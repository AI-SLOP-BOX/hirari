pub struct VideoOrchestrator {
    pub has_video: bool,
    pub fps: f64,
    pub current_frame: u64,
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
