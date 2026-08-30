use std::sync::atomic::{AtomicU64, Ordering};

pub struct MetronomeOrchestrator {
    pub total_samples: AtomicU64,
    sample_rate: f64,
    bpm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetronomeConfigError {
    InvalidSampleRate,
    InvalidBpm,
}

impl Default for MetronomeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MetronomeOrchestrator {
    pub fn new() -> Self {
        // Keep the infallible default constructor panic-free.  The constants
        // are validated by `try_new`'s contract and are intentionally kept
        // explicit here so a future validation rule cannot turn startup into
        // an unexpected panic path.
        Self {
            total_samples: AtomicU64::new(0),
            sample_rate: 44_100.0,
            bpm: 120.0,
        }
    }

    pub fn try_new(sample_rate: f64, bpm: f64) -> Result<Self, MetronomeConfigError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(MetronomeConfigError::InvalidSampleRate);
        }
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(MetronomeConfigError::InvalidBpm);
        }
        Ok(Self {
            total_samples: AtomicU64::new(0),
            sample_rate,
            bpm,
        })
    }

    pub fn new_with_config(sample_rate: f64, bpm: f64) -> Result<Self, MetronomeConfigError> {
        Self::try_new(sample_rate, bpm)
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    fn advance_samples(&self, frames: usize) -> u64 {
        self.total_samples
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(frames as u64))
            })
            .unwrap_or_else(|current| current)
    }

    pub fn process(&self, left: &mut [f32], right: &mut [f32], num_frames: usize) {
        let frames = num_frames.min(left.len()).min(right.len());
        if frames == 0 {
            return;
        }

        let samples_per_beat = (60.0 / self.bpm) * self.sample_rate;
        let click_duration = (self.sample_rate * 0.08).max(1.0).min(u64::MAX as f64) as u64;
        let start_sample = self.advance_samples(frames);

        for i in 0..frames {
            let abs_sample = start_sample.saturating_add(i as u64);
            let beat = (abs_sample as f64 / samples_per_beat).floor() as u64;
            let beat_sample = (beat as f64 * samples_per_beat).min(u64::MAX as f64) as u64;
            let offset = abs_sample.saturating_sub(beat_sample);
            if offset < click_duration {
                let t = offset as f64 / self.sample_rate;
                let freq = if beat.is_multiple_of(4) {
                    1200.0
                } else {
                    800.0
                };
                let env = (-65.0 * t).exp() as f32;
                let wave = (2.0 * std::f64::consts::PI * freq * t).sin() as f32 * env * 0.2;
                left[i] =
                    ((if left[i].is_finite() { left[i] } else { 0.0 }) + wave).clamp(-1.0, 1.0);
                right[i] =
                    ((if right[i].is_finite() { right[i] } else { 0.0 }) + wave).clamp(-1.0, 1.0);
            }
        }
    }

    pub fn audit_metronome_engine(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.bpm.is_finite()
            && self.bpm > 0.0
            && (60.0 / self.bpm * self.sample_rate).is_finite()
    }
}
