use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

const MIN_METRONOME_SAMPLE_RATE: f64 = 8_000.0;
const MAX_METRONOME_SAMPLE_RATE: f64 = 384_000.0;

/// Stateful, allocation-free click renderer used by the realtime transport.
/// The enabled flag is the only cross-thread field; click phase belongs to the
/// audio thread, just as it did in the former C++ implementation.
pub struct Metronome {
    enabled: AtomicBool,
    audio: UnsafeCell<MetronomeAudioState>,
}

struct MetronomeAudioState {
    sample_rate: f64,
    click_sample_count: u32,
    click_phase: f32,
    click_phase_step: f32,
    click_envelope: f32,
    click_envelope_decay: f32,
    last_playhead: u64,
    has_playhead: bool,
}

impl Metronome {
    fn new(sample_rate: f64) -> Self {
        let mut audio = MetronomeAudioState {
            sample_rate: 44_100.0,
            click_sample_count: 0,
            click_phase: 0.0,
            click_phase_step: 0.0,
            click_envelope: 0.0,
            click_envelope_decay: 0.0,
            last_playhead: 0,
            has_playhead: false,
        };
        audio.set_sample_rate(sample_rate);
        Self {
            enabled: AtomicBool::new(false),
            audio: UnsafeCell::new(audio),
        }
    }

    fn set_sample_rate(&self, sample_rate: f64) {
        // SAFETY: sample-rate changes are control-thread operations and must
        // be serialized with reset/process, matching the former C++ API.
        unsafe { &mut *self.audio.get() }.set_sample_rate(sample_rate);
    }

    fn reset(&self) {
        // SAFETY: reset is serialized with process by the transport owner.
        unsafe { &mut *self.audio.get() }.reset();
    }

    fn process(
        &self,
        left: &mut [f32],
        right: &mut [f32],
        playhead: u64,
        sample_rate: f64,
        bpm: f64,
    ) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: the audio callback is the sole owner of render state while
        // running; control-thread reset/rate changes are serialized with it.
        unsafe { &mut *self.audio.get() }.process(left, right, playhead, sample_rate, bpm);
    }
}

impl MetronomeAudioState {
    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite()
            && (MIN_METRONOME_SAMPLE_RATE..=MAX_METRONOME_SAMPLE_RATE).contains(&sample_rate)
        {
            sample_rate
        } else {
            44_100.0
        };
        self.reset();
    }

    fn reset(&mut self) {
        self.click_sample_count = 0;
        self.click_phase = 0.0;
        self.click_phase_step = 0.0;
        self.click_envelope = 0.0;
        self.click_envelope_decay = 0.0;
        self.last_playhead = 0;
        self.has_playhead = false;
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playhead: u64,
        sample_rate: f64,
        bpm: f64,
    ) {
        let sample_count = left.len().min(right.len());
        if sample_count == 0
            || !bpm.is_finite()
            || bpm <= 0.0
            || !sample_rate.is_finite()
            || !(MIN_METRONOME_SAMPLE_RATE..=MAX_METRONOME_SAMPLE_RATE).contains(&sample_rate)
        {
            return;
        }

        let sample_count_u64 = sample_count as u64;
        if playhead > u64::MAX - sample_count_u64 {
            return;
        }

        let samples_per_beat = (60.0 / bpm) * sample_rate;
        let transport_jumped_back = self.has_playhead && playhead < self.last_playhead;

        for i in 0..sample_count {
            let position = playhead + i as u64;
            let current_beat = position as f64 / samples_per_beat;
            let next_beat = (position + 1) as f64 / samples_per_beat;
            let starts_at_transport_head = i == 0 && (!self.has_playhead || transport_jumped_back);

            if starts_at_transport_head || current_beat.floor() != next_beat.floor() {
                let beat_index = if starts_at_transport_head {
                    current_beat.floor() as u64
                } else {
                    next_beat.floor() as u64
                };
                let frequency = if beat_index % 4 == 0 { 1_000.0 } else { 800.0 };

                self.click_sample_count = (0.04_f32 * sample_rate as f32) as u32;
                self.click_phase = 0.0;
                self.click_phase_step =
                    (2.0 * std::f64::consts::PI * frequency / sample_rate) as f32;
                self.click_envelope = 0.4;
                self.click_envelope_decay = self.click_envelope / self.click_sample_count as f32;
            }

            if self.click_sample_count > 0 {
                let click_value = self.click_phase.sin() * self.click_envelope;
                left[i] = if left[i].is_finite() {
                    (left[i] + click_value).clamp(-16.0, 16.0)
                } else {
                    click_value
                };
                right[i] = if right[i].is_finite() {
                    (right[i] + click_value).clamp(-16.0, 16.0)
                } else {
                    click_value
                };

                self.click_phase += self.click_phase_step;
                self.click_envelope = (self.click_envelope - self.click_envelope_decay).max(0.0);
                self.click_sample_count -= 1;
            }
        }

        self.last_playhead = playhead + sample_count_u64;
        self.has_playhead = true;
    }
}

unsafe fn state_from_ptr<'a>(state: *mut c_void) -> Option<&'a Metronome> {
    if state.is_null() {
        None
    } else {
        // SAFETY: callers must pass a handle returned by `hirari_metronome_create`
        // and must not access it concurrently except for the atomic enabled flag.
        Some(unsafe { &*state.cast::<Metronome>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_metronome_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(Metronome::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle was allocated by `hirari_metronome_create` and is
        // destroyed exactly once after the audio callback has stopped using it.
        drop(unsafe { Box::from_raw(state.cast::<Metronome>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_set_sample_rate(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state_from_ptr(state) } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state_from_ptr(state) } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_set_enabled(state: *mut c_void, enabled: bool) {
    if state.is_null() {
        return;
    }
    // SAFETY: handles are created as `Metronome`; only the atomic flag is
    // accessed here so this operation is safe alongside audio processing.
    let state = unsafe { &*state.cast::<Metronome>() };
    state.enabled.store(enabled, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_is_enabled(state: *const c_void) -> bool {
    if state.is_null() {
        return false;
    }
    // SAFETY: handles are created as `Metronome`; only the atomic flag is read.
    unsafe { &*state.cast::<Metronome>() }
        .enabled
        .load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_metronome_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    sample_count: u32,
    playhead: u64,
    sample_rate: f64,
    bpm: f64,
) {
    if state.is_null() || left.is_null() || right.is_null() || sample_count == 0 {
        return;
    }
    // SAFETY: the adapter provides valid, non-overlapping channel buffers for
    // `sample_count` samples, and the audio thread exclusively owns render state.
    let (left, right) = unsafe {
        (
            std::slice::from_raw_parts_mut(left, sample_count as usize),
            std::slice::from_raw_parts_mut(right, sample_count as usize),
        )
    };
    if let Some(state) = unsafe { state_from_ptr(state) } {
        state.process(left, right, playhead, sample_rate, bpm);
    }
}

#[cfg(test)]
mod tests {
    use super::Metronome;
    use std::sync::atomic::Ordering;

    #[test]
    fn renders_the_legacy_40ms_click_and_adds_it_to_audio() {
        let metronome = Metronome::new(48_000.0);
        metronome.enabled.store(true, Ordering::Release);
        let mut left = [0.1; 64];
        let mut right = [0.1; 64];

        metronome.process(&mut left, &mut right, 0, 48_000.0, 120.0);

        assert_eq!(left[0], 0.1);
        assert_eq!(right[0], 0.1);
        assert!(left[1] > 0.1);
        assert_eq!(left, right);
        assert_eq!(
            unsafe { (*metronome.audio.get()).click_sample_count },
            (0.04_f32 * 48_000.0) as u32 - 64
        );
    }

    #[test]
    fn handles_transport_rewind_and_rejects_overflowing_blocks() {
        let metronome = Metronome::new(48_000.0);
        metronome.enabled.store(true, Ordering::Release);
        let mut left = [0.0; 16];
        let mut right = [0.0; 16];
        metronome.process(&mut left, &mut right, 10_000, 48_000.0, 120.0);
        assert_eq!(
            unsafe { (*metronome.audio.get()).click_sample_count },
            1_920 - 16
        );

        metronome.process(&mut left, &mut right, 0, 48_000.0, 120.0);
        assert_eq!(
            unsafe { (*metronome.audio.get()).click_sample_count },
            1_920 - 16
        );

        let previous = left;
        metronome.process(&mut left, &mut right, u64::MAX - 1, 48_000.0, 120.0);
        assert_eq!(left, previous);
    }

    #[test]
    fn disabled_or_invalid_inputs_leave_buffers_untouched() {
        let metronome = Metronome::new(48_000.0);
        let mut left = [f32::NAN; 8];
        let mut right = [f32::INFINITY; 8];
        metronome.process(&mut left, &mut right, 0, 48_000.0, 120.0);
        assert!(left.iter().all(|sample| sample.is_nan()));
        assert!(right.iter().all(|sample| sample.is_infinite()));

        metronome.enabled.store(true, Ordering::Release);
        metronome.process(&mut left, &mut right, 0, 48_000.0, f64::NAN);
        assert!(left.iter().all(|sample| sample.is_nan()));
        assert!(right.iter().all(|sample| sample.is_infinite()));
    }
}
