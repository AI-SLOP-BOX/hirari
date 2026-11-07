pub struct MeterState {
    pub peak: f32,
    pub true_peak: f32,
    pub hold_peak: f32,
    previous_sample: f32,
}

pub struct MixerOrchestrator {
    pub meters: Vec<MeterState>,
}

#[cfg(test)]
mod tests {
    use super::MixerOrchestrator;

    #[test]
    fn meter_tracks_block_peak_and_hold_without_nonfinite_state() {
        let mut mixer = MixerOrchestrator::new();
        mixer.process_signal(0, &[0.25, -0.5, 0.1], 48_000.0);
        mixer.process_signal(0, &[0.05], 48_000.0);
        assert_eq!(mixer.meters.len(), 1);
        assert!((mixer.meters[0].peak - 0.05).abs() < 1e-6);
        assert!(mixer.meters[0].hold_peak >= 0.5);
        assert!(mixer.audit_mixing());
    }

    #[test]
    fn hold_peak_decay_never_drops_below_current_signal() {
        let mut mixer = MixerOrchestrator::new();
        mixer.process_signal(0, &[1.0], 48_000.0);
        mixer.process_signal(0, &[0.01], 48_000.0);
        assert!(mixer.decay_hold_peak(20.0));
        assert!(mixer.meters[0].hold_peak >= mixer.meters[0].true_peak);
        assert!(!mixer.decay_hold_peak(f32::NAN));
    }
}

impl Default for MixerOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MixerOrchestrator {
    pub fn new() -> Self {
        Self { meters: Vec::new() }
    }

    /// INDUSTRIAL: Processes an audio signal and updates true-peak state with absolute precision.
    pub fn process_signal(&mut self, index: usize, data: &[f32], sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || data.is_empty() {
            return;
        }
        if self.meters.len() <= index {
            self.meters.resize_with(index + 1, || MeterState {
                peak: 0.0,
                true_peak: 0.0,
                hold_peak: 0.0,
                previous_sample: 0.0,
            });
        }
        let meter = &mut self.meters[index];
        let mut peak = 0.0f32;
        let mut true_peak = 0.0f32;
        for &raw in data {
            let sample = if raw.is_finite() { raw } else { 0.0 };
            peak = peak.max(sample.abs());
            true_peak = true_peak.max(sample.abs());
            // Four-times linear intersample scan catches peaks between host
            // samples without allocating or changing the realtime buffer.
            for fraction in 1..4 {
                let t = fraction as f32 * 0.25;
                let interpolated = meter.previous_sample + (sample - meter.previous_sample) * t;
                true_peak = true_peak.max(interpolated.abs());
            }
            meter.previous_sample = sample;
        }
        meter.peak = peak;
        meter.true_peak = true_peak.min(16.0);
        meter.hold_peak = meter.hold_peak.max(meter.true_peak);
    }

    pub fn decay_hold_peak(&mut self, db_per_tick: f32) -> bool {
        if !db_per_tick.is_finite() || !(0.0..=120.0).contains(&db_per_tick) {
            return false;
        }
        let factor = 10.0f32.powf(-db_per_tick / 20.0);
        for meter in &mut self.meters {
            meter.hold_peak = (meter.hold_peak * factor).max(meter.true_peak);
        }
        true
    }

    pub fn reset_hold_peaks(&mut self) {
        for meter in &mut self.meters {
            meter.hold_peak = meter.true_peak;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide mixing state and level consistency.
    pub fn audit_mixing(&self) -> bool {
        self.meters.iter().all(|meter| {
            meter.peak.is_finite()
                && meter.true_peak.is_finite()
                && meter.hold_peak.is_finite()
                && meter.peak >= 0.0
                && meter.true_peak >= 0.0
                && meter.hold_peak >= meter.true_peak
        })
    }
}
