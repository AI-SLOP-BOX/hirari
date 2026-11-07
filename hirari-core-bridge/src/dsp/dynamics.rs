pub struct DynamicsParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

pub struct DynamicsOrchestrator;

impl DynamicsOrchestrator {
    /**
     * @brief SUGGEST: Analyzes crest factor and recommends optimal compressor settings.
     * INDUSTRIAL: Now includes transient-density detection to distinguish drums
     * (fast attack needed) from sustained sources (slow attack preferred).
     */
    pub fn suggest_parameters(&self, data: &[f32]) -> DynamicsParams {
        if data.is_empty() {
            return DynamicsParams {
                threshold_db: -18.0,
                ratio: 2.0,
                attack_ms: 10.0,
                release_ms: 80.0,
            };
        }

        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f32;
        let mut transients = 0u32;
        let mut prev_abs = 0.0f32;

        for &s in data {
            let abs_s = if s.is_finite() { s.abs() } else { 0.0 };
            peak = peak.max(abs_s);
            sum_sq += abs_s * abs_s;
            // Transient = sudden rise ≥ 6 dB in amplitude
            if abs_s > prev_abs * 2.0 && abs_s > 0.02 {
                transients += 1;
            }
            prev_abs = abs_s;
        }

        let rms = (sum_sq / data.len() as f32).sqrt();
        let crest_db = 20.0 * (peak + 1e-9).log10() - 20.0 * (rms + 1e-9).log10();
        let transient_rate = transients as f32 / data.len() as f32 * 100.0; // per 100 samples

        // --- Recommendation matrix ---
        match (crest_db as u32, transient_rate as u32) {
            (c, t) if c > 15 && t > 3 =>
            // Drums: fast attack, fast release
            {
                DynamicsParams {
                    threshold_db: -20.0,
                    ratio: 5.0,
                    attack_ms: 1.0,
                    release_ms: 40.0,
                }
            }
            (c, _) if c > 12 =>
            // Percussive melodic: medium attack
            {
                DynamicsParams {
                    threshold_db: -16.0,
                    ratio: 3.5,
                    attack_ms: 8.0,
                    release_ms: 60.0,
                }
            }
            (c, _) if c > 8 =>
            // Guitars/bass: moderate
            {
                DynamicsParams {
                    threshold_db: -12.0,
                    ratio: 2.5,
                    attack_ms: 15.0,
                    release_ms: 80.0,
                }
            }
            _ =>
            // Sustained/pads: gentle glue
            {
                DynamicsParams {
                    threshold_db: -6.0,
                    ratio: 1.5,
                    attack_ms: 30.0,
                    release_ms: 200.0,
                }
            }
        }
    }

    /**
     * @brief NOISE_GATE: Suggests an expander threshold from the signal's noise floor.
     * INDUSTRIAL: Uses the bottom-5th-percentile amplitude as the noise floor estimate.
     */
    pub fn suggest_gate_threshold(&self, data: &[f32]) -> f32 {
        if data.is_empty() {
            return -60.0;
        }
        let mut sorted: Vec<f32> = data
            .iter()
            .map(|&s| if s.is_finite() { s.abs() } else { 0.0 })
            .collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let noise_floor = sorted[sorted.len() / 20]; // 5th percentile
        20.0 * (noise_floor + 1e-9).log10() + 6.0 // +6 dB headroom above floor
    }

    pub fn audit_dynamics(&self) -> bool {
        let params = self.suggest_parameters(&[]);
        params.threshold_db.is_finite()
            && params.ratio.is_finite()
            && params.ratio >= 1.0
            && params.attack_ms.is_finite()
            && params.attack_ms > 0.0
            && params.release_ms.is_finite()
            && params.release_ms > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::DynamicsOrchestrator;

    #[test]
    fn compressor_and_gate_recommendations_cover_empty_and_signal_paths() {
        let orchestrator = DynamicsOrchestrator;
        let empty = orchestrator.suggest_parameters(&[]);
        assert_eq!(empty.threshold_db, -18.0);
        assert_eq!(empty.ratio, 2.0);
        assert_eq!(empty.attack_ms, 10.0);
        assert_eq!(empty.release_ms, 80.0);
        assert_eq!(orchestrator.suggest_gate_threshold(&[]), -60.0);

        let sustained = orchestrator.suggest_parameters(&[0.25; 128]);
        assert!(sustained.ratio >= 1.0);
        assert!(sustained.attack_ms > 0.0 && sustained.release_ms > 0.0);

        let gate = orchestrator.suggest_gate_threshold(&[0.001, 0.01, 0.1, f32::NAN]);
        assert!(gate.is_finite());
        assert!(gate < 0.0);
    }
}
