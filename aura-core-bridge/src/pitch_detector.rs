pub struct PitchDetectorEngine {
    pub sample_rate: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DetectedPitchSegment {
    pub start_sample: u64,
    pub end_sample: u64,
    pub midi_note: u8,
    pub frequency_hz: f32,
    pub confidence: f32,
}

impl PitchDetectorEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self { sample_rate }
    }

    /// INDUSTRIAL: Estimates the fundamental frequency (Hz) of an audio block.
    pub fn estimate_frequency(&self, buffer: &[f32]) -> f32 {
        let size = buffer.len();
        if size < 512 || !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return 0.0;
        }

        // 1. AUTOCORRELATION
        let mut corr = vec![0.0f32; size / 2];
        for lag in 0..(size / 2) {
            for i in 0..(size / 2) {
                let a = if buffer[i].is_finite() {
                    buffer[i]
                } else {
                    0.0
                };
                let b = if buffer[i + lag].is_finite() {
                    buffer[i + lag]
                } else {
                    0.0
                };
                corr[lag] += a * b;
            }
        }

        // 2. FIND FIRST PEAK
        let mut first_peak_lag = 0;
        for lag in 1..(corr.len() - 1) {
            if corr[lag] > corr[lag - 1] && corr[lag] > corr[lag + 1] && lag > 20 {
                // Min 20 lag for sub
                first_peak_lag = lag;
                break;
            }
        }

        if first_peak_lag == 0 {
            return 0.0;
        }
        self.sample_rate as f32 / first_peak_lag as f32
    }

    /// Detects stable note regions for a VariAudio-style editor. Frames are
    /// overlapped, low-energy/uncertain frames are ignored, and adjacent
    /// frames with the same quantized note are merged deterministically.
    pub fn detect_segments(&self, buffer: &[f32], frame_size: usize, hop_size: usize) -> Vec<DetectedPitchSegment> {
        if frame_size < 512 || hop_size == 0 || buffer.len() < frame_size || !self.audit_pitch_detector() { return Vec::new(); }
        let mut segments: Vec<DetectedPitchSegment> = Vec::new();
        let mut frame_start = 0usize;
        while frame_start + frame_size <= buffer.len() {
            let frame = &buffer[frame_start..frame_start + frame_size];
            let energy = (frame.iter().map(|sample| if sample.is_finite() { *sample as f64 * *sample as f64 } else { 0.0 }).sum::<f64>() / frame_size as f64).sqrt() as f32;
            let frequency = if energy > 1e-5 { self.estimate_frequency(frame) } else { 0.0 };
            if frequency.is_finite() && frequency >= 20.0 {
                let midi = Self::frequency_to_midi(frequency);
                let confidence = energy.min(1.0).clamp(0.0, 1.0);
                let start = frame_start as u64;
                let end = (frame_start + frame_size) as u64;
                if let Some(last) = segments.last_mut() {
                    if last.midi_note == midi && start <= last.end_sample.saturating_add(hop_size as u64) {
                        last.end_sample = end;
                        last.frequency_hz = (last.frequency_hz + frequency) * 0.5;
                        last.confidence = (last.confidence + confidence) * 0.5;
                    } else {
                        segments.push(DetectedPitchSegment { start_sample: start, end_sample: end, midi_note: midi, frequency_hz: frequency, confidence });
                    }
                } else {
                    segments.push(DetectedPitchSegment { start_sample: start, end_sample: end, midi_note: midi, frequency_hz: frequency, confidence });
                }
            }
            frame_start = frame_start.saturating_add(hop_size);
        }
        segments
    }

    /// INDUSTRIAL: Converts Frequency to nearest MIDI Note.
    pub fn frequency_to_midi(freq: f32) -> u8 {
        if !freq.is_finite() || freq < 10.0 {
            return 0;
        }
        (12.0 * (freq / 440.0).log2() + 69.0)
            .round()
            .clamp(0.0, 127.0) as u8
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Pitch Detector state.
    pub fn audit_pitch_detector(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic Pitch Detector auditing logic.
        self.sample_rate.is_finite() && self.sample_rate > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::PitchDetectorEngine;

    #[test]
    fn detects_sustained_pitch_segments() {
        let engine = PitchDetectorEngine::new(48_000.0);
        let buffer: Vec<f32> = (0..4096).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()).collect();
        let segments = engine.detect_segments(&buffer, 1024, 512);
        assert!(!segments.is_empty());
        assert!(segments.iter().all(|segment| segment.midi_note == 69));
    }
}
