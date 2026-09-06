pub struct GridOrchestrator {
    pub bpm: f64,
    pub numerator: i32,
    pub denominator: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapMode {
    Off,
    Grid,
    RelativeGrid,
    ZeroCross,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSettings {
    pub mode: SnapMode,
    pub resolution_beats: f32,
    pub threshold_samples: u32,
}
impl SnapSettings {
    pub fn validate(&self) -> bool {
        self.resolution_beats.is_finite()
            && self.resolution_beats > 0.0
            && self.resolution_beats <= 64.0
    }
}
impl SnapSettings {
    pub fn snap(
        &self,
        input: f64,
        sample_rate: f64,
        bpm: f64,
        anchor: f64,
        zero_cross: Option<f64>,
    ) -> f64 {
        if !self.validate() || !input.is_finite() {
            return input;
        }
        match self.mode {
            SnapMode::Off => input,
            SnapMode::ZeroCross => zero_cross
                .filter(|v| v.is_finite() && (input - *v).abs() <= self.threshold_samples as f64)
                .unwrap_or(input),
            SnapMode::Grid => {
                let step = sample_rate * 60.0 / bpm * self.resolution_beats as f64;
                if step.is_finite() && step > 0.0 {
                    (input / step).round() * step
                } else {
                    input
                }
            }
            SnapMode::RelativeGrid => {
                let step = sample_rate * 60.0 / bpm * self.resolution_beats as f64;
                if step.is_finite() && step > 0.0 {
                    anchor + ((input - anchor) / step).round() * step
                } else {
                    input
                }
            }
        }
    }
}

impl Default for GridOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl GridOrchestrator {
    pub fn new() -> Self {
        Self {
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
        }
    }

    /// INDUSTRIAL: Calculates the snapped sample position with absolute temporal precision and grid sovereignty.
    pub fn get_snapped_samples(
        &self,
        input_samples: f64,
        resolution: f32,
        sample_rate: f64,
    ) -> f64 {
        if !input_samples.is_finite()
            || !resolution.is_finite()
            || resolution <= 0.0
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !self.bpm.is_finite()
            || self.bpm <= 0.0
        {
            return input_samples;
        }
        let input_beats = input_samples * self.bpm / (60.0 * sample_rate);
        let snapped_beats = (input_beats / resolution as f64).round() * resolution as f64;
        self.beats_to_samples(snapped_beats, sample_rate).max(0.0)
    }

    /// INDUSTRIAL: Converts beats to samples with absolute temporal precision and grid sovereignty.
    pub fn beats_to_samples(&self, beats: f64, sample_rate: f64) -> f64 {
        // INDUSTRIAL: Implementation of high-performance time conversion.
        if !beats.is_finite()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !self.bpm.is_finite()
            || self.bpm <= 0.0
        {
            return 0.0;
        }
        (beats * 60.0 / self.bpm) * sample_rate
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide temporal mapping state.
    pub fn audit_grid_system(&self) -> bool {
        self.bpm.is_finite()
            && (20.0..=999.0).contains(&self.bpm)
            && self.numerator > 0
            && self.numerator <= 64
            && self.denominator > 0
            && self.denominator <= 64
            && (self.denominator as u32).is_power_of_two()
    }
}
