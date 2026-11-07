pub struct VideoOrchestrator {
    pub has_video: bool,
    pub fps: f64,
    pub current_frame: u64,
}

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::video_native::{AvFoundationSource, FrameSource};

#[derive(Default)]
struct VideoFrameState {
    generation: u64,
    has_video: bool,
    duration: f64,
    fps: f64,
    pending_seconds: Option<f64>,
    stopping: bool,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    revision: u64,
}

struct VideoRuntimeShared {
    state: Mutex<VideoFrameState>,
    wake: Condvar,
    source: Arc<dyn FrameSource>,
}

/// The frame cache plus the single worker allowed to call the frame source.
pub struct VideoRuntime {
    shared: Arc<VideoRuntimeShared>,
    worker: Option<JoinHandle<()>>,
}

impl VideoRuntime {
    fn new(source: Arc<dyn FrameSource>) -> Option<Self> {
        let shared = Arc::new(VideoRuntimeShared {
            state: Mutex::new(VideoFrameState {
                fps: 24.0,
                ..VideoFrameState::default()
            }),
            wake: Condvar::new(),
            source,
        });
        let worker_state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("hirari-video-frame".into())
            .spawn(move || video_worker_loop(worker_state))
            .ok()?;
        Some(Self { shared, worker: Some(worker) })
    }

    /// Drops the current asset and returns the generation new work must carry.
    fn clear(&self) -> u64 {
        let mut state = self.state();
        state.generation = state.generation.wrapping_add(1);
        state.has_video = false;
        state.duration = 0.0;
        state.fps = 24.0;
        state.pending_seconds = None;
        state.pixels.clear();
        state.width = 0;
        state.height = 0;
        state.revision = 0;
        state.generation
    }

    fn load(&self, path: &str) -> bool {
        let generation = self.clear();
        let Some((duration, fps)) = self.shared.source.open(path, generation) else {
            return false;
        };
        if !(fps > 0.0) { return false; }
        let mut state = self.state();
        // Loading is synchronous, so a second load can already have taken the
        // generation while this one was reading the file header.
        if state.stopping || state.generation != generation { return false; }
        state.duration = duration;
        state.fps = fps;
        state.has_video = true;
        true
    }

    fn request_frame(&self, seconds: f64) -> bool {
        let mut state = self.state();
        if !state.has_video || state.stopping || !seconds.is_finite() || seconds < 0.0 {
            return false;
        }
        let duration = if state.duration.is_nan() { 0.0 } else { state.duration.max(0.0) };
        state.pending_seconds = Some(seconds.clamp(0.0, duration));
        drop(state);
        self.shared.wake.notify_one();
        true
    }

    fn revision(&self) -> u64 {
        self.state().revision
    }

    fn has_video(&self) -> bool {
        self.state().has_video
    }

    /// Little-endian width, height, then packed RGBA8.
    fn frame_packet(&self) -> Vec<u8> {
        let state = self.state();
        if state.pixels.is_empty() { return Vec::new(); }
        let mut packet = Vec::with_capacity(8 + state.pixels.len());
        packet.extend_from_slice(&state.width.to_le_bytes());
        packet.extend_from_slice(&state.height.to_le_bytes());
        packet.extend_from_slice(&state.pixels);
        packet
    }

    fn state(&self) -> std::sync::MutexGuard<'_, VideoFrameState> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for VideoRuntime {
    fn drop(&mut self) {
        {
            let mut state = self.state();
            state.stopping = true;
            state.pending_seconds = None;
        }
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
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

        let Some(frame) = shared.source.decode(seconds, generation) else { continue };

        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.stopping && state.has_video && state.generation == generation {
            state.pixels = frame.pixels;
            state.width = frame.width;
            state.height = frame.height;
            state.revision = state.revision.wrapping_add(1);
        }
    }
}

/// Process-wide video preview service: the engine owns one asset at a time and
/// the UI polls it from the render loop.
pub struct VideoService {
    runtime: Option<VideoRuntime>,
}

static VIDEO_SERVICE: OnceLock<VideoService> = OnceLock::new();

pub fn video_service() -> &'static VideoService {
    VIDEO_SERVICE.get_or_init(|| VideoService {
        runtime: VideoRuntime::new(Arc::new(AvFoundationSource::new())),
    })
}

impl VideoService {
    pub fn load_video(&self, path: &str) -> bool {
        if path.is_empty() { return false; }
        self.runtime.as_ref().is_some_and(|runtime| runtime.load(path))
    }

    pub fn request_frame(&self, seconds: f64) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.request_frame(seconds))
    }

    pub fn frame_revision(&self) -> u64 {
        self.runtime.as_ref().map_or(0, VideoRuntime::revision)
    }

    pub fn has_video(&self) -> bool {
        self.runtime.as_ref().map_or(false, VideoRuntime::has_video)
    }

    pub fn frame_packet(&self) -> Vec<u8> {
        self.runtime.as_ref().map_or_else(Vec::new, VideoRuntime::frame_packet)
    }
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
    use super::{VideoRuntime, video_service};
    use crate::video_native::{DecodedFrame, FrameSource};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Frame source scripted by the tests: it replays queued frames for the
    /// runtime's worker and records every request that reaches it.
    #[derive(Default)]
    struct ScriptedSource {
        metadata: Mutex<Option<(f64, f64)>>,
        queued: Mutex<VecDeque<DecodedFrame>>,
        requests: Mutex<Vec<(f64, u64)>>,
        completed: AtomicU64,
        stall: AtomicBool,
    }

    impl ScriptedSource {
        fn with_metadata(&self, duration: f64, fps: f64) {
            *self.metadata.lock().unwrap() = Some((duration, fps));
        }

        fn queue(&self, frame: DecodedFrame) {
            self.queued.lock().unwrap().push_back(frame);
        }

        /// Waits until the worker has entered `decode`, which is only observable
        /// once the request is recorded.
        fn await_request(&self, expected: (f64, u64)) -> bool {
            wait_until(|| self.requests.lock().unwrap().contains(&expected))
        }
    }

    impl FrameSource for ScriptedSource {
        fn open(&self, _path: &str, _generation: u64) -> Option<(f64, f64)> {
            *self.metadata.lock().unwrap()
        }

        fn decode(&self, seconds: f64, generation: u64) -> Option<DecodedFrame> {
            self.requests.lock().unwrap().push((seconds, generation));
            while self.stall.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let frame = self.queued.lock().unwrap().pop_front();
            self.completed.fetch_add(1, Ordering::SeqCst);
            frame
        }
    }

    fn wait_until<F: FnMut() -> bool>(mut predicate: F) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if predicate() { return true; }
            if Instant::now() >= deadline { return false; }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn frame(width: u32, height: u32, fill: u8) -> DecodedFrame {
        DecodedFrame {
            pixels: vec![fill; (width as usize) * (height as usize) * 4],
            width,
            height,
        }
    }

    fn runtime_with(source: &Arc<ScriptedSource>, duration: f64, fps: f64) -> VideoRuntime {
        source.with_metadata(duration, fps);
        let runtime = VideoRuntime::new(Arc::clone(source) as Arc<dyn FrameSource>).unwrap();
        assert!(runtime.load("clip.mp4"));
        runtime
    }

    #[test]
    fn load_fails_closed_without_asset_metadata() {
        let source = Arc::new(ScriptedSource::default());
        let runtime = VideoRuntime::new(source.clone()).unwrap();
        assert!(!runtime.load("silent.mp4"));
        assert!(!runtime.has_video());
        assert!(!runtime.request_frame(0.0));
    }

    #[test]
    fn loaded_video_accepts_requests_and_clamps_them() {
        let source = Arc::new(ScriptedSource::default());
        let runtime = runtime_with(&source, 10.0, 25.0);
        assert!(runtime.has_video());
        assert_eq!(runtime.revision(), 0);

        assert!(runtime.request_frame(99.0));
        assert!(source.await_request((10.0, 1)));

        assert!(!runtime.request_frame(f64::NAN));
        assert!(!runtime.request_frame(-1.0));
        assert_eq!(source.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn decoded_frame_publishes_as_a_sized_packet() {
        let source = Arc::new(ScriptedSource::default());
        let runtime = runtime_with(&source, 4.0, 24.0);
        source.queue(frame(2, 1, 7));

        assert!(runtime.request_frame(1.0));
        assert!(wait_until(|| runtime.revision() == 1));

        let packet = runtime.frame_packet();
        assert_eq!(&packet[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(packet[8..], vec![7u8; 8]);
    }

    #[test]
    fn a_frame_for_a_superseded_generation_is_dropped() {
        let source = Arc::new(ScriptedSource::default());
        let runtime = runtime_with(&source, 4.0, 24.0);
        source.stall.store(true, Ordering::SeqCst);

        assert!(runtime.request_frame(1.0));
        assert!(source.await_request((1.0, 1)));
        source.queue(frame(4, 4, 9));

        // Reloading takes the generation while the first decode is still in
        // flight; its frame belongs to an asset the user already replaced.
        source.with_metadata(8.0, 30.0);
        assert!(runtime.load("other.mp4"));
        source.stall.store(false, Ordering::SeqCst);

        assert!(wait_until(|| source.completed.load(Ordering::SeqCst) == 1));
        assert_eq!(runtime.revision(), 0);
        assert!(runtime.frame_packet().is_empty());
    }

    #[test]
    fn reloading_clears_the_cached_frame() {
        let source = Arc::new(ScriptedSource::default());
        let runtime = runtime_with(&source, 4.0, 24.0);
        source.queue(frame(1, 1, 3));
        assert!(runtime.request_frame(0.5));
        assert!(wait_until(|| runtime.revision() == 1));

        *source.metadata.lock().unwrap() = None;
        assert!(!runtime.load("clip.mp4"));
        assert_eq!(runtime.revision(), 0);
        assert!(runtime.frame_packet().is_empty());
        assert!(!runtime.has_video());
    }

    #[test]
    fn service_rejects_an_empty_path_and_an_invalid_position() {
        let service = video_service();
        assert!(!service.load_video(""));
        assert!(!service.request_frame(f64::NAN));
        assert!(!service.request_frame(-0.5));
    }

    /// Exercises the AVFoundation source itself. It needs a real clip, so the
    /// run is opt-in through `HIRARI_VIDEO_FIXTURE`; the scripted source above
    /// covers the runtime in every configuration.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_source_decodes_a_real_clip() {
        use super::AvFoundationSource;

        let Ok(path) = std::env::var("HIRARI_VIDEO_FIXTURE") else { return };
        let source = AvFoundationSource::new();
        let (duration, fps) = source.open(&path, 1).expect("fixture must expose a video track");
        assert!(fps > 0.0 && duration > 0.0, "fps={fps} duration={duration}");

        let frame = source.decode(duration / 2.0, 1).expect("fixture frame");
        assert!(frame.width > 0 && frame.height > 0);
        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4
        );

        // A superseded generation must not produce frames any more.
        assert!(source.decode(duration / 2.0, 2).is_none());

        // The runtime drives the same source through its worker thread, which
        // is how the UI preview reaches AVFoundation.
        let runtime = VideoRuntime::new(Arc::new(AvFoundationSource::new())).unwrap();
        assert!(runtime.load(&path));
        assert!(runtime.request_frame(duration / 2.0));
        assert!(wait_until(|| runtime.revision() == 1), "worker published no frame");
        let packet = runtime.frame_packet();
        let width = u32::from_le_bytes(packet[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(packet[4..8].try_into().unwrap());
        assert_eq!((width, height), (frame.width, frame.height));
        assert_eq!(packet.len(), 8 + width as usize * height as usize * 4);
    }

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
