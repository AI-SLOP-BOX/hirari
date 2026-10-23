pub struct VideoOrchestrator {
    pub has_video: bool,
    pub fps: f64,
    pub current_frame: u64,
}

use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

type DecodeNativeFrame = unsafe extern "C" fn(*mut c_void, f64, u64, *mut *mut c_void) -> bool;
type ReadNativeFrame = unsafe extern "C" fn(
    *mut c_void, *mut u32, *mut u32, *mut f64, *mut *const u8, *mut usize,
) -> bool;
type DestroyNativeFrame = unsafe extern "C" fn(*mut c_void);
type VisitPixels = unsafe extern "C" fn(*mut c_void, *const u8, usize);

#[derive(Default)]
struct VideoFrameState {
    generation: u64,
    has_video: bool,
    duration: f64,
    fps: f64,
    requested_seconds: f64,
    pending_seconds: Option<f64>,
    stopping: bool,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    seconds: f64,
    revision: u64,
}

struct VideoRuntimeShared {
    state: Mutex<VideoFrameState>,
    wake: Condvar,
    context: usize,
    decode: DecodeNativeFrame,
    read: ReadNativeFrame,
    destroy: DestroyNativeFrame,
}

struct VideoRuntime {
    shared: Arc<VideoRuntimeShared>,
    worker: Option<JoinHandle<()>>,
}

impl VideoRuntime {
    fn new(context: *mut c_void, decode: DecodeNativeFrame,
           read: ReadNativeFrame, destroy: DestroyNativeFrame) -> Option<Self> {
        let shared = Arc::new(VideoRuntimeShared {
            state: Mutex::new(VideoFrameState { fps: 24.0, requested_seconds: -1.0,
                seconds: -1.0, ..VideoFrameState::default() }),
            wake: Condvar::new(), context: context as usize, decode, read, destroy,
        });
        let worker_state = Arc::clone(&shared);
        let worker = thread::Builder::new().name("hirari-video-frame".into())
            .spawn(move || video_worker_loop(worker_state)).ok()?;
        Some(Self { shared, worker: Some(worker) })
    }
}

impl Drop for VideoRuntime {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.stopping = true;
            state.pending_seconds = None;
        }
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn video_worker_loop(shared: Arc<VideoRuntimeShared>) {
    loop {
        let (seconds, generation) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while !state.stopping && state.pending_seconds.is_none() {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.stopping { return; }
            (state.pending_seconds.take().unwrap(), state.generation)
        };

        let mut native_frame = std::ptr::null_mut();
        let decoded = unsafe {
            (shared.decode)(shared.context as *mut c_void, seconds, generation, &mut native_frame)
        };
        let mut pixels = Vec::new();
        let mut width = 0;
        let mut height = 0;
        let mut actual_seconds = seconds;
        if decoded && !native_frame.is_null() {
            let mut data = std::ptr::null();
            let mut length = 0;
            let valid = unsafe {
                (shared.read)(native_frame, &mut width, &mut height,
                    &mut actual_seconds, &mut data, &mut length)
            };
            let expected = (width as usize).checked_mul(height as usize)
                .and_then(|area| area.checked_mul(4));
            if valid && width > 0 && height > 0 && width <= 8192 && height <= 8192
                && expected == Some(length) && (length == 0 || !data.is_null())
            {
                pixels.extend_from_slice(unsafe { std::slice::from_raw_parts(data, length) });
            }
        }
        if !native_frame.is_null() { unsafe { (shared.destroy)(native_frame) }; }
        if pixels.is_empty() { continue; }

        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.stopping && state.has_video && state.generation == generation {
            state.pixels = pixels;
            state.width = width;
            state.height = height;
            state.seconds = actual_seconds;
            state.revision = state.revision.wrapping_add(1);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_create(
    context: *mut c_void,
    decode: Option<DecodeNativeFrame>,
    read: Option<ReadNativeFrame>,
    destroy: Option<DestroyNativeFrame>,
) -> *mut c_void {
    if context.is_null() { return std::ptr::null_mut(); }
    let (Some(decode), Some(read), Some(destroy)) = (decode, read, destroy) else {
        return std::ptr::null_mut();
    };
    VideoRuntime::new(context, decode, read, destroy)
        .map_or(std::ptr::null_mut(), |runtime| Box::into_raw(Box::new(runtime)).cast())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_destroy(runtime: *mut c_void) {
    if !runtime.is_null() { drop(unsafe { Box::from_raw(runtime.cast::<VideoRuntime>()) }); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_clear(runtime: *mut c_void) -> u64 {
    let Some(runtime) = (unsafe { runtime.cast::<VideoRuntime>().as_ref() }) else { return 0 };
    let mut state = runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner());
    state.generation = state.generation.wrapping_add(1);
    state.has_video = false;
    state.duration = 0.0;
    state.fps = 24.0;
    state.requested_seconds = -1.0;
    state.pending_seconds = None;
    state.pixels.clear();
    state.width = 0;
    state.height = 0;
    state.seconds = -1.0;
    state.revision = 0;
    state.generation
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_set_metadata(
    runtime: *mut c_void, duration: f64, fps: f64,
) -> bool {
    let Some(runtime) = (unsafe { runtime.cast::<VideoRuntime>().as_ref() }) else { return false };
    if fps <= 0.0 { return false; }
    let mut state = runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.stopping { return false; }
    state.duration = duration;
    state.fps = fps;
    state.has_video = true;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_update(
    runtime: *const c_void, sample: u64, sample_rate: f64,
) {
    let Some(runtime) = (unsafe { runtime.cast::<VideoRuntime>().as_ref() }) else { return };
    if !sample_rate.is_finite() || sample_rate <= 0.0 { return; }
    let mut state = runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if !state.has_video { return; }
    let seconds = sample as f64 / sample_rate;
    let duration = if state.duration.is_nan() { 0.0 } else { state.duration.max(0.0) };
    state.requested_seconds = seconds.clamp(0.0, duration);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_request_frame(
    runtime: *mut c_void, seconds: f64,
) -> bool {
    let Some(runtime) = (unsafe { runtime.cast::<VideoRuntime>().as_ref() }) else { return false };
    let mut state = runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if !state.has_video || state.stopping || !seconds.is_finite() { return false; }
    let duration = if state.duration.is_nan() { 0.0 } else { state.duration.max(0.0) };
    state.pending_seconds = Some(seconds.clamp(0.0, duration));
    drop(state);
    runtime.shared.wake.notify_one();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_has_video(runtime: *const c_void) -> bool {
    unsafe { runtime.cast::<VideoRuntime>().as_ref() }
        .is_some_and(|runtime| runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner()).has_video)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_fps(runtime: *const c_void) -> f64 {
    unsafe { runtime.cast::<VideoRuntime>().as_ref() }
        .map_or(0.0, |runtime| runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner()).fps)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_duration(runtime: *const c_void) -> f64 {
    unsafe { runtime.cast::<VideoRuntime>().as_ref() }
        .map_or(0.0, |runtime| runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner()).duration)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_requested_seconds(runtime: *const c_void) -> f64 {
    unsafe { runtime.cast::<VideoRuntime>().as_ref() }
        .map_or(-1.0, |runtime| runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner()).requested_seconds)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_revision(runtime: *const c_void) -> u64 {
    unsafe { runtime.cast::<VideoRuntime>().as_ref() }
        .map_or(0, |runtime| runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner()).revision)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_video_runtime_copy_frame(
    runtime: *const c_void, context: *mut c_void, visit: Option<VisitPixels>,
    width: *mut u32, height: *mut u32, seconds: *mut f64,
) -> bool {
    let Some(runtime) = (unsafe { runtime.cast::<VideoRuntime>().as_ref() }) else { return false };
    let Some(visit) = visit else { return false };
    if width.is_null() || height.is_null() || seconds.is_null() { return false; }
    let state = runtime.shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.pixels.is_empty() { return false; }
    unsafe {
        *width = state.width;
        *height = state.height;
        *seconds = state.seconds;
        visit(context, state.pixels.as_ptr(), state.pixels.len());
    }
    true
}

impl Default for VideoOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoOrchestrator {
    pub fn new() -> Self {
        Self {
            has_video: false,
            fps: 24.0,
            current_frame: 0,
        }
    }
    pub fn set_video_present(&mut self, present: bool) { self.has_video = present; if !present { self.current_frame = 0; } }
    pub fn set_fps(&mut self, fps: f64) -> bool { if !fps.is_finite() || !(1.0..=120.0).contains(&fps) { return false; } self.fps = fps; true }
    pub fn seek_frame(&mut self, frame: u64) -> bool { if !self.has_video || !self.audit_video_system() { return false; } self.current_frame = frame; true }
    pub fn seek_timecode(&mut self, timecode: &str) -> bool { self.frame_from_timecode(timecode).is_some_and(|frame| self.seek_frame(frame)) }
    pub fn frame_for_samples(&self, samples: u64, sample_rate: f64) -> Option<u64> { if !sample_rate.is_finite() || sample_rate <= 0.0 || !self.fps.is_finite() || self.fps <= 0.0 { return None; } let frame = samples as f64 / sample_rate * self.fps; (frame.is_finite() && frame >= 0.0 && frame <= u64::MAX as f64).then_some(frame.round() as u64) }
    pub fn samples_for_frame(&self, frame: u64, sample_rate: f64) -> Option<u64> { if !sample_rate.is_finite() || sample_rate <= 0.0 || !self.fps.is_finite() || self.fps <= 0.0 { return None; } let samples = frame as f64 * sample_rate / self.fps; (samples.is_finite() && samples >= 0.0 && samples <= u64::MAX as f64).then_some(samples.round() as u64) }
    pub fn samples_for_timecode(&self, timecode: &str, sample_rate: f64) -> Option<u64> { self.frame_from_timecode(timecode).and_then(|frame| self.samples_for_frame(frame, sample_rate)) }

    /**
     * @brief UPDATE: Calculates the current video frame with absolute temporal precision.
     * INDUSTRIAL: Ensures sample-accurate lock between the audio playhead and the video stream.
     */
    pub fn update_video_sync(&mut self, audio_sample_pos: u64, sample_rate: f64) {
        if !self.has_video
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !self.fps.is_finite()
            || self.fps <= 0.0
        {
            return;
        }
        let seconds = audio_sample_pos as f64 / sample_rate;
        let frame = seconds * self.fps;
        if frame.is_finite() && frame >= 0.0 && frame <= u64::MAX as f64 {
            self.current_frame = frame.round() as u64;
        }
    }

    /**
     * @brief TIMECODE: Returns the current SMPTE timecode string.
     * INDUSTRIAL: Format: HH:MM:SS:FF for professional post-production workflows.
     */
    pub fn get_smpte_timecode(&self) -> String {
        if !self.fps.is_finite() || self.fps <= 0.0 {
            return "00:00:00:00".to_owned();
        }
        let total_seconds = self.current_frame as f64 / self.fps;
        let hours = (total_seconds / 3600.0) as u32;
        let minutes = ((total_seconds / 60.0) % 60.0) as u32;
        let seconds = (total_seconds % 60.0) as u32;
        let frames_per_second = self.fps.round().max(1.0) as u64;
        let frames = (self.current_frame % frames_per_second) as u32;

        format!("{:02}:{:02}:{:02}:{:02}", hours, minutes, seconds, frames)
    }
    pub fn get_drop_frame_timecode(&self) -> String {
        if !self.fps.is_finite() || (self.fps - 29.97).abs() > 0.01 { return self.get_smpte_timecode(); }
        let nominal = 30u64; let frames = self.current_frame; let dropped = 2u64;
        let d = frames / 17982; let m = frames % 17982; let extra = dropped * 9 * d + dropped * ((m.saturating_sub(2)) / 1798);
        let total = frames + extra; let h = total / (nominal * 3600); let min = (total / (nominal * 60)) % 60; let sec = (total / nominal) % 60; let fr = total % nominal;
        format!("{:02}:{:02}:{:02};{:02}", h, min, sec, fr)
    }
    pub fn frame_from_timecode(&self, timecode: &str) -> Option<u64> {
        let drop_frame = timecode.contains(';');
        let mut parts = timecode.split([':', ';']).map(|part| part.parse::<u64>().ok());
        let (h, m, s, f) = (parts.next()??, parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() || m >= 60 || s >= 60 || !self.fps.is_finite() || self.fps <= 0.0 { return None; }
        let nominal = self.fps.round() as u64;
        if nominal == 0 || f >= nominal { return None; }
        if drop_frame && nominal == 30 && m % 10 != 0 && s == 0 && f < 2 { return None; }
        let nominal_frames = h.checked_mul(3600 * nominal)?.checked_add(m.checked_mul(60 * nominal)?)?.checked_add(s.checked_mul(nominal)?)?.checked_add(f)?;
        if drop_frame && nominal == 30 { let dropped = 2u64 * (m + 60 * h - (m + 60 * h) / 10); return nominal_frames.checked_sub(dropped); }
        Some(nominal_frames)
    }

    pub fn audit_video_system(&self) -> bool {
        self.fps.is_finite() && (1.0..=120.0).contains(&self.fps)
    }
}

#[cfg(test)]
mod tests {
    use super::VideoOrchestrator;

    #[test]
    fn invalid_sample_rate_does_not_change_video_frame() {
        let mut video = VideoOrchestrator::new();
        video.has_video = true;
        video.current_frame = 12;
        video.update_video_sync(48_000, 0.0);
        assert_eq!(video.current_frame, 12);
        video.update_video_sync(48_000, f64::NAN);
        assert_eq!(video.current_frame, 12);
    }

    #[test]
    fn invalid_fps_timecode_fails_closed() {
        let mut video = VideoOrchestrator::new();
        video.fps = 0.0;
        assert_eq!(video.get_smpte_timecode(), "00:00:00:00");
        assert!(!video.audit_video_system());
    }
    #[test]
    fn drop_frame_timecode_uses_semicolon_for_ntsc() {
        let mut video = VideoOrchestrator::new();
        assert!(video.set_fps(29.97));
        video.current_frame = 30;
        assert!(video.get_drop_frame_timecode().contains(';'));
    }
    #[test]
    fn frame_timecode_round_trip_is_bounded() {
        let mut video = VideoOrchestrator::new();
        video.set_fps(25.0);
        let frame = video.frame_from_timecode("01:02:03:04").unwrap();
        assert_eq!(video.frame_for_samples(video.samples_for_frame(frame, 48_000.0).unwrap(), 48_000.0), Some(frame));
        assert!(video.frame_from_timecode("01:60:00:00").is_none());
    }
    #[test]
    fn seek_timecode_requires_loaded_video() {
        let mut video = VideoOrchestrator::new();
        assert!(!video.seek_timecode("00:00:01:00"));
        video.set_video_present(true);
        assert!(video.seek_timecode("00:00:01:00"));
        assert_eq!(video.current_frame, 24);
        video.set_video_present(false);
        assert_eq!(video.current_frame, 0);
    }
    #[test]
    fn maximum_supported_frame_rate_remains_operational() {
        let mut video = VideoOrchestrator::new();
        assert!(video.set_fps(120.0));
        video.set_video_present(true);
        assert!(video.seek_frame(240));
        assert!(video.audit_video_system());
    }
    #[test]
    fn drop_frame_parser_rejects_invalid_frame_field() {
        let mut video = VideoOrchestrator::new();
        video.set_fps(29.97);
        assert!(video.frame_from_timecode("00:00:00;30").is_none());
        assert!(video.frame_from_timecode("00:00:00;00").is_some());
    }
}
