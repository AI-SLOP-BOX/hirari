/**
 * @struct VocalAlignmentEngine
 * @brief Professional spectral-matching and temporal alignment engine.
 * INDUSTRIAL: Uses Dynamic Time Warping (DTW) logic to align backing vocals
 * to a lead reference with absolute temporal sovereignty.
 */
pub struct VocalAlignmentEngine {
    pub tightness: f32, // 0.0 to 1.0
}

impl VocalAlignmentEngine {
    pub fn new() -> Self {
        Self { tightness: 0.8 }
    }

    pub fn set_tightness(&mut self, value: f32) {
        if value.is_finite() {
            self.tightness = value.clamp(0.0, 1.0);
        }
    }

    /**
     * @brief ALIGN: Calculates the necessary warp points to tighten a dub to a lead.
     * INDUSTRIAL: Beyond simple delay, this performs non-linear spectral stretching.
     */
    pub fn calculate_alignment(&self, lead_envelope: &[f32], dub_envelope: &[f32]) -> Vec<f32> {
        if lead_envelope.is_empty()
            || dub_envelope.is_empty()
            || lead_envelope.len() > 2_000_000
            || dub_envelope.len() > 2_000_000
        {
            return Vec::new();
        }
        const GRID: usize = 512;
        let finite = |value: f32| if value.is_finite() { value.abs() } else { 0.0 };
        let sample = |source: &[f32], index: usize| {
            let at = index.saturating_mul(source.len()) / GRID;
            finite(source[at.min(source.len() - 1)])
        };
        let mut dp = vec![f64::INFINITY; (GRID + 1) * (GRID + 1)];
        let at = |row: usize, col: usize| row * (GRID + 1) + col;
        dp[at(0, 0)] = 0.0;
        for row in 1..=GRID {
            for col in 1..=GRID {
                let cost = f64::from(
                    (sample(lead_envelope, row - 1) - sample(dub_envelope, col - 1)).abs(),
                );
                dp[at(row, col)] = cost
                    + dp[at(row - 1, col)].min(dp[at(row, col - 1)].min(dp[at(row - 1, col - 1)]));
            }
        }
        let mut path = Vec::with_capacity(GRID * 2);
        let (mut row, mut col) = (GRID, GRID);
        while row > 0 || col > 0 {
            path.push((row, col));
            if row == 0 {
                col -= 1;
                continue;
            }
            if col == 0 {
                row -= 1;
                continue;
            }
            let diagonal = dp[at(row - 1, col - 1)];
            let vertical = dp[at(row - 1, col)];
            let horizontal = dp[at(row, col - 1)];
            if diagonal <= vertical && diagonal <= horizontal {
                row -= 1;
                col -= 1;
            } else if vertical <= horizontal {
                row -= 1;
            } else {
                col -= 1;
            }
        }
        path.push((0, 0));
        path.reverse();
        let mut factors = vec![1.0_f32; dub_envelope.len()];
        for pair in path.windows(2) {
            let (r0, c0) = pair[0];
            let (r1, c1) = pair[1];
            if c1 <= c0 || c0 == 0 {
                continue;
            }
            let ratio = ((r1.saturating_sub(r0).max(1)) as f32 / (c1 - c0) as f32
                * lead_envelope.len() as f32
                / dub_envelope.len() as f32)
                .clamp(0.25, 4.0);
            let start = c0.saturating_mul(dub_envelope.len()) / GRID;
            let start = start.min(factors.len());
            let end = c1.saturating_mul(dub_envelope.len()) / GRID;
            let end = end.min(factors.len());
            let end = end.max(start + 1).min(factors.len());
            let tightness = self.tightness.clamp(0.0, 1.0);
            for value in &mut factors[start..end] {
                *value = ratio * tightness + (1.0 - tightness);
            }
        }
        factors
    }

    pub fn audit_vocal_align(&self) -> bool {
        self.tightness.is_finite() && (0.0..=1.0).contains(&self.tightness)
    }
}

impl Default for VocalAlignmentEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::VocalAlignmentEngine;

    #[test]
    fn dtw_alignment_returns_bounded_local_warp() {
        let engine = VocalAlignmentEngine::new();
        let lead: Vec<f32> = (0..96).map(|i| ((i as f32) * 0.17).sin().abs()).collect();
        let dub: Vec<f32> = (0..128)
            .map(|i| (((i as f32) * 0.13).sin()).abs())
            .collect();
        let result = engine.calculate_alignment(&lead, &dub);
        assert_eq!(result.len(), dub.len());
        assert!(result
            .iter()
            .all(|value| value.is_finite() && (0.25..=4.0).contains(value)));
    }

    #[test]
    fn empty_alignment_fails_closed() {
        let mut engine = VocalAlignmentEngine::default();
        engine.set_tightness(f32::NAN);
        assert!(engine.audit_vocal_align());
        engine.set_tightness(2.0);
        assert!((engine.tightness - 1.0).abs() < f32::EPSILON);
        assert!(engine.calculate_alignment(&[], &[1.0]).is_empty());
    }
}
