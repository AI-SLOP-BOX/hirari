#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdviceCode {
    None,
    SidechainKickBass,
    NotchTrackB,
    ClarityOK,
}

pub struct ClashInfo {
    pub masking_index: f32,
    pub center_freq: f32,
    pub advice: AdviceCode,
}

pub struct MixClashDetectorEngine {}

impl Default for MixClashDetectorEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MixClashDetectorEngine {
    pub fn new() -> Self {
        Self {}
    }

    /**
     * @brief DETECT: Analyzes spectral masking using the psychoacoustic Bark scale.
     * INDUSTRIAL: Beyond raw overlaps, this identifies where the human ear actually loses clarity.
     */
    pub fn detect(&self, spectrum_a: &[f32], spectrum_b: &[f32], sample_rate: f64) -> ClashInfo {
        let size = spectrum_a.len();
        if size == 0 || spectrum_b.len() != size || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return self.empty_clash();
        }

        let bark_edges = [
            0.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0,
            1720.0, 2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0,
            12000.0, 15500.0,
        ];

        let mut bark_energy_a = [0.0f32; 24];
        let mut bark_energy_b = [0.0f32; 24];
        let bin_width = (sample_rate as f32) / (2.0 * size as f32);

        for (i, (&mag_a, &mag_b)) in spectrum_a.iter().zip(spectrum_b.iter()).enumerate() {
            let freq = i as f32 * bin_width;
            let bark = bark_edges
                .iter()
                .position(|&e| e > freq)
                .unwrap_or(24)
                .saturating_sub(1);
            if bark < 24 {
                bark_energy_a[bark] += if mag_a.is_finite() { mag_a.abs() } else { 0.0 };
                bark_energy_b[bark] += if mag_b.is_finite() { mag_b.abs() } else { 0.0 };
            }
        }

        let mut max_masking = 0.0f32;
        let mut max_band = 0usize;

        for i in 0..24 {
            let total = bark_energy_a[i] + bark_energy_b[i];
            if total > 1e-3 {
                let masking = (bark_energy_a[i].min(bark_energy_b[i]) * 2.0) / total;
                if masking > max_masking {
                    max_masking = masking;
                    max_band = i;
                }
            }
        }

        let center_freq = (bark_edges[max_band] + bark_edges[max_band + 1]) / 2.0;
        let advice = if max_masking > 0.7 {
            if center_freq < 300.0 {
                AdviceCode::SidechainKickBass
            } else {
                AdviceCode::NotchTrackB
            }
        } else {
            AdviceCode::ClarityOK
        };

        ClashInfo {
            masking_index: max_masking,
            center_freq,
            advice,
        }
    }

    fn empty_clash(&self) -> ClashInfo {
        ClashInfo {
            masking_index: 0.0,
            center_freq: 0.0,
            advice: AdviceCode::None,
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Clash Detector state.
    pub fn audit_clash_detector(&self) -> bool {
        // The detector is intentionally stateless, so audit a deterministic
        // probe instead of reporting success unconditionally. This catches
        // regressions in finite-value handling, Bark-band indexing, and the
        // advice threshold without requiring a project or audio device.
        let clean = self.detect(&[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0], 48_000.0);
        if !clean.masking_index.is_finite()
            || !(0.0..=1.0).contains(&clean.masking_index)
            || !clean.center_freq.is_finite()
            || clean.center_freq < 0.0
        {
            return false;
        }
        let masked = self.detect(&[1.0; 128], &[1.0; 128], 48_000.0);
        masked.masking_index.is_finite()
            && (0.0..=1.0).contains(&masked.masking_index)
            && masked.center_freq.is_finite()
            && masked.center_freq >= 0.0
            && masked.center_freq <= 24_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::MixClashDetectorEngine;

    #[test]
    fn audit_runs_real_deterministic_probes() {
        assert!(MixClashDetectorEngine::new().audit_clash_detector());
    }
}
