//! Real-time capture queue and writer lifecycle for the native audio facade.

use crate::recording_stream::StreamingRecordingWriter;
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const RING_CAPACITY: usize = 1 << 19;
const FRAMES_PER_WRITE: usize = 4096;

#[derive(Clone, Copy, Default)]
struct StereoFrame {
    left: f32,
    right: f32,
}

struct SpscFrameRing {
    slots: Box<[UnsafeCell<StereoFrame>]>,
    write: AtomicU64,
    read: AtomicU64,
}

// Exactly one audio callback produces and exactly one disk thread consumes.
unsafe impl Sync for SpscFrameRing {}

impl SpscFrameRing {
    fn new() -> Self {
        let slots = (0..RING_CAPACITY)
            .map(|_| UnsafeCell::new(StereoFrame::default()))
            .collect();
        Self {
            slots,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        self.read.store(0, Ordering::Relaxed);
        self.write.store(0, Ordering::Relaxed);
    }

    fn push(&self, frame: StereoFrame) -> bool {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= (RING_CAPACITY - 1) as u64 {
            return false;
        }
        // SAFETY: the write index is owned by the single producer. Acquire on
        // `read` ensures the consumer has released this slot before reuse.
        unsafe { *self.slots[write as usize & (RING_CAPACITY - 1)].get() = frame };
        self.write.store(write.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop_batch(&self, output: &mut [f32]) -> usize {
        let capacity = output.len() / 2;
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let count = write
            .wrapping_sub(read)
            .min(capacity as u64)
            .min((RING_CAPACITY - 1) as u64) as usize;
        for index in 0..count {
            // SAFETY: acquire on `write` observes the producer's completed
            // frame write, and the consumer exclusively owns this read slot.
            let frame = unsafe { *self.slots[(read as usize + index) & (RING_CAPACITY - 1)].get() };
            output[index * 2] = frame.left;
            output[index * 2 + 1] = frame.right;
        }
        self.read
            .store(read.wrapping_add(count as u64), Ordering::Release);
        count
    }

    fn is_empty(&self) -> bool {
        self.read.load(Ordering::Relaxed) == self.write.load(Ordering::Acquire)
    }
}

struct CaptureShared {
    ring: SpscFrameRing,
    recording: AtomicBool,
    stop_writer: AtomicBool,
    write_failed: AtomicBool,
    overflowed: AtomicBool,
    dropped_frames: AtomicU64,
    frames_written: AtomicU64,
    active_writers: AtomicU64,
}

impl CaptureShared {
    fn new() -> Self {
        Self {
            ring: SpscFrameRing::new(),
            recording: AtomicBool::new(false),
            stop_writer: AtomicBool::new(false),
            write_failed: AtomicBool::new(false),
            overflowed: AtomicBool::new(false),
            dropped_frames: AtomicU64::new(0),
            frames_written: AtomicU64::new(0),
            active_writers: AtomicU64::new(0),
        }
    }
}

struct NativeCapture {
    shared: Arc<CaptureShared>,
    lifecycle: Mutex<()>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

impl NativeCapture {
    fn new() -> Self {
        Self {
            shared: Arc::new(CaptureShared::new()),
            lifecycle: Mutex::new(()),
            writer: Mutex::new(None),
        }
    }

    fn start(&self, path: PathBuf, sample_rate: f64) -> bool {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.stop_unlocked();
        if path.as_os_str().is_empty()
            || !sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&sample_rate)
        {
            return false;
        }
        let writer = match StreamingRecordingWriter::create_float32(path, sample_rate as u32, 2) {
            Ok(writer) => writer,
            Err(_) => return false,
        };
        self.shared.ring.reset();
        self.shared.write_failed.store(false, Ordering::Relaxed);
        self.shared.overflowed.store(false, Ordering::Relaxed);
        self.shared.dropped_frames.store(0, Ordering::Relaxed);
        self.shared.frames_written.store(0, Ordering::Relaxed);
        self.shared.stop_writer.store(false, Ordering::Release);
        let shared = Arc::clone(&self.shared);
        let Ok(worker) = thread::Builder::new()
            .name("hirari-native-recording-writer".into())
            .spawn(move || writer_loop(shared, writer))
        else {
            self.shared.stop_writer.store(true, Ordering::Release);
            return false;
        };
        *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = Some(worker);
        self.shared.recording.store(true, Ordering::Release);
        true
    }

    fn stop(&self) {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.stop_unlocked();
    }

    fn stop_unlocked(&self) {
        self.shared.recording.store(false, Ordering::Release);
        self.shared.stop_writer.store(true, Ordering::Release);
        if let Some(worker) = self
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            if worker.join().is_err() {
                self.shared.write_failed.store(true, Ordering::Release);
            }
        }
    }

    fn write(&self, left: &[f32], right: &[f32]) -> bool {
        self.shared.active_writers.fetch_add(1, Ordering::AcqRel);
        if !self.shared.recording.load(Ordering::Acquire) {
            self.shared.active_writers.fetch_sub(1, Ordering::Release);
            return false;
        }
        let frames = left.len().min(right.len());
        for index in 0..frames {
            let left = left[index];
            let right = right[index];
            let frame = StereoFrame {
                left: if left.is_finite() { left } else { 0.0 },
                right: if right.is_finite() { right } else { 0.0 },
            };
            if !self.shared.ring.push(frame) {
                self.shared.overflowed.store(true, Ordering::Release);
                self.shared
                    .dropped_frames
                    .fetch_add((frames - index) as u64, Ordering::Relaxed);
                self.shared.recording.store(false, Ordering::Release);
                self.shared.stop_writer.store(true, Ordering::Release);
                self.shared.active_writers.fetch_sub(1, Ordering::Release);
                return false;
            }
        }
        self.shared.active_writers.fetch_sub(1, Ordering::Release);
        true
    }
}

impl Drop for NativeCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn writer_loop(shared: Arc<CaptureShared>, mut writer: StreamingRecordingWriter) {
    let mut interleaved = vec![0.0_f32; FRAMES_PER_WRITE * 2];
    loop {
        let frames = shared.ring.pop_batch(&mut interleaved);
        if frames != 0 {
            if writer
                .append_interleaved(&interleaved[..frames * 2])
                .is_err()
            {
                writer.preserve_partial_spool();
                shared.write_failed.store(true, Ordering::Release);
                shared.recording.store(false, Ordering::Release);
                shared.stop_writer.store(true, Ordering::Release);
                return;
            }
            shared
                .frames_written
                .fetch_add(frames as u64, Ordering::Relaxed);
        } else if shared.stop_writer.load(Ordering::Acquire)
            && shared.active_writers.load(Ordering::Acquire) == 0
        {
            break;
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }

    while !shared.ring.is_empty() {
        let frames = shared.ring.pop_batch(&mut interleaved);
        if frames == 0 {
            break;
        }
        if writer
            .append_interleaved(&interleaved[..frames * 2])
            .is_err()
        {
            writer.preserve_partial_spool();
            shared.write_failed.store(true, Ordering::Release);
            return;
        }
        shared
            .frames_written
            .fetch_add(frames as u64, Ordering::Relaxed);
    }

    if writer.finalize().is_err() {
        shared.write_failed.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_recording_capture_create() -> *mut c_void {
    Box::into_raw(Box::new(NativeCapture::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle is returned by create and destroyed once by the facade.
        drop(unsafe { Box::from_raw(state.cast::<NativeCapture>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_start(
    state: *mut c_void,
    path: *const c_char,
    sample_rate: f64,
) -> bool {
    if state.is_null() || path.is_null() {
        return false;
    }
    // SAFETY: the facade passes a live C string for the duration of this call.
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        return false;
    };
    // SAFETY: state is an opaque handle created by this module.
    unsafe { &*state.cast::<NativeCapture>() }.start(PathBuf::from(path), sample_rate)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_stop(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<NativeCapture>().as_ref() } {
        state.stop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_write(
    state: *const c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeCapture>().as_ref() }) else {
        return false;
    };
    if left.is_null() || right.is_null() {
        return false;
    }
    // SAFETY: C++ provides two valid, non-overlapping channels of `frames` samples.
    let (left, right) = unsafe {
        (
            std::slice::from_raw_parts(left, frames as usize),
            std::slice::from_raw_parts(right, frames as usize),
        )
    };
    state.write(left, right)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_is_recording(state: *const c_void) -> bool {
    unsafe { state.cast::<NativeCapture>().as_ref() }
        .is_some_and(|state| state.shared.recording.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_has_write_error(state: *const c_void) -> bool {
    unsafe { state.cast::<NativeCapture>().as_ref() }
        .is_some_and(|state| state.shared.write_failed.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_has_overflowed(state: *const c_void) -> bool {
    unsafe { state.cast::<NativeCapture>().as_ref() }
        .is_some_and(|state| state.shared.overflowed.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_recording_capture_dropped_frames(state: *const c_void) -> u64 {
    unsafe { state.cast::<NativeCapture>().as_ref() }.map_or(0, |state| {
        state.shared.dropped_frames.load(Ordering::Acquire)
    })
}

#[cfg(test)]
mod tests {
    use super::{NativeCapture, SpscFrameRing, StereoFrame, RING_CAPACITY};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn spsc_ring_preserves_stereo_order_across_wrap_and_reports_full() {
        let ring = SpscFrameRing::new();
        for index in 0..RING_CAPACITY - 1 {
            assert!(ring.push(StereoFrame {
                left: index as f32,
                right: -(index as f32),
            }));
        }
        assert!(!ring.push(StereoFrame::default()));
        let mut batch = vec![0.0; 2048 * 2];
        let mut received = 0usize;
        while !ring.is_empty() {
            let count = ring.pop_batch(&mut batch);
            assert!(count > 0);
            for index in 0..count {
                let expected = (received + index) as f32;
                assert_eq!(batch[index * 2], expected);
                assert_eq!(batch[index * 2 + 1], -expected);
            }
            received += count;
        }
        assert_eq!(received, RING_CAPACITY - 1);
        assert!(ring.push(StereoFrame {
            left: 9.0,
            right: -9.0
        }));
        let mut one = [0.0; 2];
        assert_eq!(ring.pop_batch(&mut one), 1);
        assert_eq!(one, [9.0, -9.0]);
    }

    #[test]
    fn native_capture_drains_the_stop_tail_and_publishes_float_wav() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hirari-native-capture-{}-{nonce}.wav",
            std::process::id()
        ));
        let capture = NativeCapture::new();
        assert!(capture.start(path.clone(), 48_000.0));
        let mut left = [0.125_f32; 256];
        let right = [-0.25_f32; 256];
        left[0] = f32::NAN;
        for _ in 0..17 {
            assert!(capture.write(&left, &right));
        }
        capture.stop();

        assert!(!capture
            .shared
            .write_failed
            .load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            capture
                .shared
                .dropped_frames
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert_eq!(
            capture
                .shared
                .frames_written
                .load(std::sync::atomic::Ordering::Acquire),
            17 * 256
        );
        let bytes = std::fs::read(&path).expect("published recording");
        assert_eq!(
            crate::recording_stream::recording_wav_metadata(&bytes),
            Some((2, 17 * 256 * 2 * 4))
        );
        assert_eq!(f32::from_le_bytes(bytes[80..84].try_into().unwrap()), 0.0);
        assert_eq!(f32::from_le_bytes(bytes[84..88].try_into().unwrap()), -0.25);
        std::fs::remove_file(path).expect("remove recording fixture");
    }
}
