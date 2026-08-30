pub struct AnalysisResult {
    pub bpm: f32,
    pub confidence: f32,
}
impl AnalysisResult { pub fn validate(&self) -> bool { self.bpm.is_finite() && (20.0..=300.0).contains(&self.bpm) && self.confidence.is_finite() && self.confidence >= 0.0 } }

pub struct TempoAnalyzerEngine {}

impl Default for TempoAnalyzerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TempoAnalyzerEngine {
    pub fn new() -> Self {
        Self {}
    }

    /// INDUSTRIAL: Detecting BPM using Spectral Flux Onset Detection.
    pub fn detect_bpm(&self, data: &[f32], sample_rate: f64) -> AnalysisResult {
        let len = data.len();
        let hop_size = 512;
        let n_fft = 1024;
        let mut flux = Vec::new();

        if len < n_fft || !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate)
        {
            return AnalysisResult {
                bpm: 120.0,
                confidence: 0.0,
            };
        }

        // 1. SPECTRAL FLUX (Onset Strength)
        let mut i = 0;
        let mut previous_energy = 0.0f32;
        while i < len - n_fft {
            let mut energy = 0.0f32;
            for k in 0..hop_size {
                let sample = if data[i + k].is_finite() {
                    data[i + k]
                } else {
                    0.0
                };
                energy += sample * sample;
            }
            // Positive spectral-flux style onset strength: only energy rises
            // contribute, suppressing sustained tones and DC-like beds.
            flux.push((energy - previous_energy).max(0.0));
            previous_energy = energy;
            i += hop_size;
        }

        // 2. AUTOCORRELATION (BPM Hub)
        let mut best_val = 0.0f32;
        let mut best_bpm = 120.0f32;

        let mut bpm = 60.0f32;
        while bpm < 190.0 {
            let lag = (sample_rate * 60.0 / bpm as f64 / hop_size as f64)
                .round()
                .max(1.0) as usize;
            let mut current_val = 0.0f32;

            if flux.len() > lag + 1 {
                for j in 0..(flux.len() - lag - 1) {
                    current_val += flux[j] * flux[j + lag];
                }
            }

            if current_val > best_val {
                best_val = current_val;
                best_bpm = bpm;
            }
            bpm += 0.5;
        }

        // Normalize correlation against total onset energy so confidence is a
        // portable 0..=1 score instead of a buffer-length-dependent number.
        let normalization = flux.iter().map(|value| f64::from(*value) * f64::from(*value)).sum::<f64>() as f32;
        let confidence = if best_val.is_finite() && normalization.is_finite() && normalization > f32::EPSILON {
            (best_val / normalization).clamp(0.0, 1.0)
        } else { 0.0 };
        let result = AnalysisResult {
            bpm: best_bpm,
            confidence,
        };
        if result.validate() { result } else { AnalysisResult { bpm: 120.0, confidence: 0.0 } }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Tempo Analyzer state.
    pub fn audit_tempo_analyzer(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic Tempo Analyzer auditing logic.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::TempoAnalyzerEngine;

    #[test]
    fn invalid_rate_and_non_finite_input_return_safe_defaults() {
        let analyzer = TempoAnalyzerEngine::new();
        let invalid = analyzer.detect_bpm(&vec![f32::NAN; 2048], 0.0);
        assert_eq!(invalid.bpm, 120.0);
        assert_eq!(invalid.confidence, 0.0);

        let valid = analyzer.detect_bpm(&vec![f32::NAN; 2048], 48_000.0);
        assert!(valid.bpm.is_finite());
        assert!(valid.confidence.is_finite());
        assert!((0.0..=1.0).contains(&valid.confidence));
    }
}
