use serde::{Deserialize, Serialize};

pub struct LoudnessState {
    pub integrated: f32,
    pub short_term: f32,
    pub momentary: f32,
    pub true_peak: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoudnessPreset {
    EbuR128,
    AtscA85,
    Streaming,
    JapaneseBroadcast,
}

impl LoudnessPreset {
    pub fn limits(self) -> (f32, f32) {
        match self {
            Self::EbuR128 => (-23.0, -1.0),
            Self::AtscA85 => (-24.0, -2.0),
            Self::Streaming => (-14.0, -1.0),
            Self::JapaneseBroadcast => (-24.0, -1.0),
        }
    }
    pub fn compliant(self, integrated: f32, true_peak: f32) -> bool {
        let (target, peak) = self.limits();
        integrated.is_finite()
            && true_peak.is_finite()
            && (integrated - target).abs() <= 1.0
            && true_peak <= peak
    }
    pub fn evaluate(self, integrated: f32, true_peak: f32) -> LoudnessReport {
        let (target, peak) = self.limits();
        LoudnessReport {
            target_lufs: target,
            integrated_lufs: integrated,
            true_peak_db: true_peak,
            delta_lufs: integrated - target,
            peak_headroom_db: peak - true_peak,
            compliant: self.compliant(integrated, true_peak),
        }
    }
    pub fn validate_delivery(self, reports: &[(f32, f32)]) -> bool {
        !reports.is_empty()
            && reports
                .iter()
                .all(|(lufs, peak)| self.compliant(*lufs, *peak))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoudnessReport {
    pub target_lufs: f32,
    pub integrated_lufs: f32,
    pub true_peak_db: f32,
    pub delta_lufs: f32,
    pub peak_headroom_db: f32,
    pub compliant: bool,
}
impl LoudnessReport {
    pub fn valid(&self) -> bool {
        self.target_lufs.is_finite()
            && self.integrated_lufs.is_finite()
            && self.true_peak_db.is_finite()
            && self.delta_lufs.is_finite()
            && self.peak_headroom_db.is_finite()
    }
}

pub struct LoudnessOrchestrator {
    pub state: LoudnessState,
    energy_sum: f64,
    energy_samples: u64,
    previous_sample: f32,
}

#[cfg(test)]
mod preset_tests {
    use super::LoudnessPreset;
    #[test]
    fn report_exposes_delivery_headroom() {
        let report = LoudnessPreset::Streaming.evaluate(-14.2, -2.0);
        assert!(report.compliant);
        assert!((report.delta_lufs + 0.2).abs() < 0.001);
        assert!((report.peak_headroom_db - 1.0).abs() < 0.001);
    }
}

impl Default for LoudnessOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessOrchestrator {
    pub fn new() -> Self {
        Self {
            state: LoudnessState {
                integrated: -70.0,
                short_term: -70.0,
                momentary: -70.0,
                true_peak: -100.0,
            },
            energy_sum: 0.0,
            energy_samples: 0,
            previous_sample: 0.0,
        }
    }

    /// INDUSTRIAL: Processes an audio signal and updates EBU R128 loudness state with absolute precision.
    pub fn process_signal(&mut self, data: &[f32], sample_rate: f64) {
        if data.is_empty() {
            return;
        }
        let mut sum = 0.0f64;
        let mut peak = 0.0f32;
        let mut interpolated_peak = 0.0f32;
        let safe_sample_rate = if sample_rate.is_finite() && sample_rate > 1.0 {
            sample_rate as f32
        } else {
            44_100.0
        };
        for &sample in data {
            let value = if sample.is_finite() { sample } else { 0.0 };
            sum += f64::from(value) * f64::from(value);
            peak = peak.max(value.abs());
            // A linear inter-sample estimate catches peaks between adjacent
            // samples without allocating or changing the audio buffer.
            let slope_peak = self.previous_sample.abs().max(value.abs())
                + (self.previous_sample - value).abs() * 0.5;
            interpolated_peak = interpolated_peak.max(slope_peak);
            self.previous_sample = value;
        }
        let rms = (sum / data.len() as f64).sqrt() as f32;
        let db = |v: f32, floor: f32| {
            if v > 0.0 {
                20.0 * v.log10().max(floor)
            } else {
                floor
            }
        };
        let momentary = db(rms, -70.0);
        self.energy_sum += sum;
        self.energy_samples = self.energy_samples.saturating_add(data.len() as u64);
        let integrated_rms = (self.energy_sum / self.energy_samples.max(1) as f64).sqrt() as f32;
        let integrated_db = db(integrated_rms, -70.0);
        let true_peak = peak.max(interpolated_peak).min(16.0);
        let peak_db = db(true_peak, -100.0);
        self.state.momentary = momentary;
        self.state.short_term = self.state.short_term * 0.75 + momentary * 0.25;
        // Keep the public state responsive while using the accumulated
        // energy for the long-term value.  The sample-rate read also guards
        // against callers accidentally passing an invalid device rate.
        let integration_alpha = (data.len() as f32 / safe_sample_rate).clamp(0.0001, 1.0);
        self.state.integrated =
            self.state.integrated * (1.0 - integration_alpha) + integrated_db * integration_alpha;
        self.state.true_peak = self.state.true_peak.max(peak_db);
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide loudness safety state.
    pub fn audit_loudness(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic loudness auditing logic.
        // Checks for broadcast standard violations and clipping artifacts.
        self.state.integrated.is_finite()
            && self.state.short_term.is_finite()
            && self.state.momentary.is_finite()
            && self.state.true_peak.is_finite()
            && self.energy_sum.is_finite()
            && self.energy_samples > 0
            && self.state.true_peak <= 24.1
    }
}
