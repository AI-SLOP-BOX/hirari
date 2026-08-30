#![allow(clippy::too_many_arguments)]

pub struct StemConfig {
    pub fft_size: u32,
    pub hop_size: u32,
}

pub struct StemOrchestrator {
    pub config: StemConfig,
    // Filter states
    pub lp_z1: f32,
    pub hp_z1: f32,
}

impl Default for StemOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl StemOrchestrator {
    pub fn new() -> Self {
        Self {
            config: StemConfig {
                fft_size: 2048,
                hop_size: 1024,
            },
            lp_z1: 0.0,
            hp_z1: 0.0,
        }
    }

    /**
     * @brief SPLIT: 4-stem M/S separation engine.
     * INDUSTRIAL: Vocals reside primarily in the Mid (mono sum) component.
     * Drums occupy the full spectrum. Bass concentrates below 250 Hz.
     * "Other" = the harmonic mid-band remainder (guitars, keys, etc.)
     *
     * Method: two 1-pole IIR filters (LP ≤ 250Hz, HP ≥ 4kHz) + Mid/Side decode.
     */
    pub fn split_signal(
        &mut self,
        left: &[f32],
        right: &[f32],
        sample_rate: f64,
        drums: &mut [f32],
        bass: &mut [f32],
        vocals: &mut [f32],
        other: &mut [f32],
    ) {
        let n = left
            .len()
            .min(right.len())
            .min(drums.len())
            .min(bass.len())
            .min(vocals.len())
            .min(other.len());
        if n == 0 {
            return;
        }
        if !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate) {
            drums[..n].fill(0.0);
            bass[..n].fill(0.0);
            vocals[..n].fill(0.0);
            other[..n].fill(0.0);
            return;
        }

        // Coefficient for 1-pole IIR LP at 250 Hz
        let lp_c = 1.0 - (-2.0 * std::f32::consts::PI * 250.0 / sample_rate as f32).exp();
        // Coefficient for 1-pole IIR HP at 4000 Hz
        let hp_c = (-2.0 * std::f32::consts::PI * 4000.0 / sample_rate as f32).exp();

        for i in 0..n {
            // --- Mid/Side decode ---
            let l = if left[i].is_finite() { left[i] } else { 0.0 };
            let r = if right[i].is_finite() { right[i] } else { 0.0 };
            let mid = (l + r) * 0.5;
            let side = (l - r) * 0.5;

            // --- LP (bass content) ---
            self.lp_z1 += lp_c * (mid - self.lp_z1);
            let lp_out = self.lp_z1;

            // --- HP (drum transients, cymbals) ---
            let hp_out = hp_c * (self.hp_z1 + mid - self.hp_z1);
            self.hp_z1 = mid;

            // --- Stem assignments ---
            bass[i] = lp_out; // Sub + bass fundamentals
            drums[i] = hp_out + side * 0.5; // Highs + stereo width (cymbals/rooms)
            vocals[i] = mid - lp_out - hp_out; // Mid band of the mono center
            other[i] = side - side * 0.5; // Harmonic stereo content
        }
    }

    pub fn audit_separation(&self) -> bool {
        self.config.fft_size.is_power_of_two()
            && self.config.fft_size >= 64
            && self.config.hop_size > 0
            && self.config.hop_size <= self.config.fft_size
            && self.lp_z1.is_finite()
            && self.hp_z1.is_finite()
    }
}
