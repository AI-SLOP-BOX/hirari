//! Lock-free automation recording rings shared by the native engine adapters.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const RECORDER_CAPACITY: usize = 65_536;
const TRACK_CAPTURE_CAPACITY: usize = 16_384;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct NativeAutomationEvent {
    pub track_id: u32,
    pub param_id: u32,
    pub pos: u64,
    pub val: f32,
}

struct RecorderSlot {
    version: AtomicU64,
    track_id: AtomicU32,
    param_id: AtomicU32,
    pos: AtomicU64,
    value: AtomicU32,
}

impl RecorderSlot {
    fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            track_id: AtomicU32::new(0),
            param_id: AtomicU32::new(0),
            pos: AtomicU64::new(0),
            value: AtomicU32::new(0),
        }
    }

    fn write(&self, ticket: u64, event: NativeAutomationEvent) {
        self.version
            .store(ticket.wrapping_mul(2).wrapping_add(1), Ordering::Release);
        self.track_id.store(event.track_id, Ordering::Relaxed);
        self.param_id.store(event.param_id, Ordering::Relaxed);
        self.pos.store(event.pos, Ordering::Relaxed);
        self.value.store(event.val.to_bits(), Ordering::Relaxed);
        self.version
            .store(ticket.wrapping_mul(2).wrapping_add(2), Ordering::Release);
    }

    fn read(&self, ticket: u64) -> Option<NativeAutomationEvent> {
        let expected = ticket.wrapping_mul(2).wrapping_add(2);
        if self.version.load(Ordering::Acquire) != expected {
            return None;
        }
        let event = NativeAutomationEvent {
            track_id: self.track_id.load(Ordering::Relaxed),
            param_id: self.param_id.load(Ordering::Relaxed),
            pos: self.pos.load(Ordering::Relaxed),
            val: f32::from_bits(self.value.load(Ordering::Relaxed)),
        };
        (self.version.load(Ordering::Acquire) == expected).then_some(event)
    }
}

struct AutomationRecorderState {
    mode: AtomicU32,
    punch_start: AtomicU64,
    punch_end: AtomicU64,
    head: AtomicU64,
    oldest_visible: AtomicU64,
    slots: Box<[RecorderSlot]>,
}

impl AutomationRecorderState {
    fn new() -> Self {
        let slots = (0..RECORDER_CAPACITY)
            .map(|_| RecorderSlot::new())
            .collect();
        Self {
            mode: AtomicU32::new(0),
            punch_start: AtomicU64::new(0),
            punch_end: AtomicU64::new(0),
            head: AtomicU64::new(0),
            oldest_visible: AtomicU64::new(0),
            slots,
        }
    }

    fn record_value(&self, track_id: u32, param_id: u32, value: f32, timestamp: u64) {
        let mode = self.mode.load(Ordering::Acquire);
        if mode == 0 || !value.is_finite() {
            return;
        }
        if mode == 4
            && (timestamp < self.punch_start.load(Ordering::Relaxed)
                || timestamp >= self.punch_end.load(Ordering::Relaxed))
        {
            return;
        }
        let ticket = self.head.fetch_add(1, Ordering::Relaxed);
        self.slots[ticket as usize % RECORDER_CAPACITY].write(
            ticket,
            NativeAutomationEvent {
                track_id,
                param_id,
                pos: timestamp,
                val: value,
            },
        );
        let min_visible = ticket
            .saturating_add(1)
            .saturating_sub(RECORDER_CAPACITY as u64);
        self.oldest_visible
            .fetch_max(min_visible, Ordering::Release);
    }

    fn snapshot(&self) -> Vec<NativeAutomationEvent> {
        let end = self.head.load(Ordering::Acquire);
        let start = self
            .oldest_visible
            .load(Ordering::Acquire)
            .max(end.saturating_sub(RECORDER_CAPACITY as u64));
        (start..end)
            .filter_map(|ticket| self.slots[ticket as usize % RECORDER_CAPACITY].read(ticket))
            .collect()
    }

    fn flush(&self) {
        self.oldest_visible
            .store(self.head.load(Ordering::Acquire), Ordering::Release);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TrackAutomationCapturePoint {
    pub time: f64,
    pub volume: f32,
    pub pan: f32,
}

struct TrackCaptureInner {
    points: Box<[UnsafeCell<TrackAutomationCapturePoint>]>,
    write: AtomicU64,
    read: AtomicU64,
    drops: AtomicU32,
}

// One serialized audio producer publishes points to one control-thread consumer.
unsafe impl Sync for TrackCaptureInner {}

impl TrackCaptureInner {
    fn new() -> Self {
        let points = (0..TRACK_CAPTURE_CAPACITY)
            .map(|_| UnsafeCell::new(TrackAutomationCapturePoint::default()))
            .collect();
        Self {
            points,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            drops: AtomicU32::new(0),
        }
    }

    fn record(
        &self,
        track_id: u32,
        sample_position: u64,
        volume: f32,
        pan: f32,
        recorder: &AutomationRecorderState,
    ) -> bool {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.saturating_sub(read) >= TRACK_CAPTURE_CAPACITY as u64 {
            self.drops.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let point = TrackAutomationCapturePoint {
            time: sample_position as f64,
            volume,
            pan: pan.clamp(-1.0, 1.0),
        };
        unsafe {
            *self.points[write as usize % TRACK_CAPTURE_CAPACITY].get() = point;
        }
        recorder.record_value(track_id, 0, point.volume, sample_position);
        recorder.record_value(track_id, 1, point.pan, sample_position);
        self.write.store(write + 1, Ordering::Release);
        true
    }

    fn flush_into(&self, output: &mut [TrackAutomationCapturePoint]) -> Option<usize> {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Relaxed);
        let count = write
            .saturating_sub(read)
            .min(TRACK_CAPTURE_CAPACITY as u64) as usize;
        if output.len() < count {
            return None;
        }
        for (index, destination) in output.iter_mut().take(count).enumerate() {
            *destination = unsafe {
                *self.points[(read + index as u64) as usize % TRACK_CAPTURE_CAPACITY].get()
            };
        }
        self.read.store(read + count as u64, Ordering::Release);
        Some(count)
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_recorder_create() -> *mut c_void {
    Box::into_raw(Box::new(AutomationRecorderState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<AutomationRecorderState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_set_mode(state: *const c_void, mode: u32) {
    if let Some(state) = unsafe { state.cast::<AutomationRecorderState>().as_ref() } {
        state.mode.store(mode.min(4), Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_get_mode(state: *const c_void) -> u32 {
    unsafe { state.cast::<AutomationRecorderState>().as_ref() }
        .map_or(0, |state| state.mode.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_set_punch_range(
    state: *const c_void,
    start: u64,
    end: u64,
) {
    if let Some(state) = unsafe { state.cast::<AutomationRecorderState>().as_ref() } {
        state.punch_start.store(start, Ordering::Release);
        state.punch_end.store(end, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_record_value(
    state: *const c_void,
    track_id: u32,
    param_id: u32,
    value: f32,
    timestamp: u64,
) {
    if let Some(state) = unsafe { state.cast::<AutomationRecorderState>().as_ref() } {
        state.record_value(track_id, param_id, value, timestamp);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_flush(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<AutomationRecorderState>().as_ref() } {
        state.flush();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_recorder_snapshot(
    state: *const c_void,
    output: *mut NativeAutomationEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<AutomationRecorderState>().as_ref() }) else {
        return 0;
    };
    let events = state.snapshot();
    if output.is_null() || capacity < events.len() {
        return events.len();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(events.as_ptr(), output, events.len());
    }
    events.len()
}

#[no_mangle]
pub extern "C" fn hirari_track_automation_capture_create() -> *mut c_void {
    Box::into_raw(Box::new(TrackCaptureInner::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_capture_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<TrackCaptureInner>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_capture_record(
    state: *const c_void,
    recorder: *const c_void,
    track_id: u32,
    sample_position: u64,
    volume: f32,
    pan: f32,
) -> bool {
    let (Some(state), Some(recorder)) = (
        unsafe { state.cast::<TrackCaptureInner>().as_ref() },
        unsafe { recorder.cast::<AutomationRecorderState>().as_ref() },
    ) else {
        return false;
    };
    state.record(track_id, sample_position, volume, pan, recorder)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_capture_flush(
    state: *const c_void,
    output: *mut TrackAutomationCapturePoint,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<TrackCaptureInner>().as_ref() }) else {
        return 0;
    };
    let write = state.write.load(Ordering::Acquire);
    let read = state.read.load(Ordering::Relaxed);
    let count = write
        .saturating_sub(read)
        .min(TRACK_CAPTURE_CAPACITY as u64) as usize;
    if output.is_null() || capacity < count {
        return count;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    state.flush_into(output).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_modes_and_punch_range_gate_events() {
        let state = AutomationRecorderState::new();
        state.record_value(1, 1, 0.5, 10);
        assert!(state.snapshot().is_empty());
        state.mode.store(4, Ordering::Release);
        state.punch_start.store(20, Ordering::Relaxed);
        state.punch_end.store(30, Ordering::Relaxed);
        state.record_value(1, 1, 0.2, 19);
        state.record_value(1, 1, 0.3, 20);
        state.record_value(1, 1, f32::NAN, 21);
        state.record_value(1, 1, 0.4, 30);
        assert_eq!(
            state.snapshot(),
            vec![NativeAutomationEvent {
                track_id: 1,
                param_id: 1,
                pos: 20,
                val: 0.3
            }]
        );
    }

    #[test]
    fn track_capture_preserves_volume_pan_and_drains_as_spsc() {
        let capture = TrackCaptureInner::new();
        let recorder = AutomationRecorderState::new();
        recorder.mode.store(1, Ordering::Release);
        assert!(capture.record(7, 123, 0.75, 1.5, &recorder));
        assert_eq!(recorder.snapshot().len(), 2);
        let mut output = [TrackAutomationCapturePoint::default(); 1];
        assert_eq!(capture.flush_into(&mut output), Some(1));
        assert_eq!(output[0].time, 123.0);
        assert_eq!(output[0].volume, 0.75);
        assert_eq!(output[0].pan, 1.0);
        assert_eq!(capture.read.load(Ordering::Acquire), 1);
    }

    #[test]
    fn recorder_ring_keeps_the_newest_bounded_history_and_flush_hides_old_events() {
        let state = AutomationRecorderState::new();
        state.mode.store(1, Ordering::Release);
        for index in 0..(RECORDER_CAPACITY as u64 + 2) {
            state.record_value(1, 0, (index % 100) as f32, index);
        }
        let events = state.snapshot();
        assert_eq!(events.len(), RECORDER_CAPACITY);
        assert_eq!(events[0].pos, 2);
        state.flush();
        assert!(state.snapshot().is_empty());
        state.record_value(1, 0, 1.0, 100_000);
        assert_eq!(state.snapshot()[0].pos, 100_000);
    }
}
