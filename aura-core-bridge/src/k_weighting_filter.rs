pub struct KWeightingFilterEngine {
    pub sample_rate: f64,
    pub z1_l_stage1: f32,
    pub z2_l_stage1: f32,
    pub z1_r_stage1: f32,
    pub z2_r_stage1: f32,
    pub z1_l_stage2: f32,
    pub z2_l_stage2: f32,
    pub z1_r_stage2: f32,
    pub z2_r_stage2: f32,

    pub b0_stage1: f32,
    pub b1_stage1: f32,
    pub b2_stage1: f32,
    pub a1_stage1: f32,
    pub a2_stage1: f32,

    pub b0_stage2: f32,
    pub b1_stage2: f32,
    pub b2_stage2: f32,
    pub a1_stage2: f32,
    pub a2_stage2: f32,
}

impl KWeightingFilterEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1.0 {
            sample_rate
        } else {
            48_000.0
        };
        let mut engine = Self {
            sample_rate,
            z1_l_stage1: 0.0,
            z2_l_stage1: 0.0,
            z1_r_stage1: 0.0,
            z2_r_stage1: 0.0,
            z1_l_stage2: 0.0,
            z2_l_stage2: 0.0,
            z1_r_stage2: 0.0,
            z2_r_stage2: 0.0,
            b0_stage1: 0.0,
            b1_stage1: 0.0,
            b2_stage1: 0.0,
            a1_stage1: 0.0,
            a2_stage1: 0.0,
            b0_stage2: 0.0,
            b1_stage2: 0.0,
            b2_stage2: 0.0,
            a1_stage2: 0.0,
            a2_stage2: 0.0,
        };
        engine.setup_coefficients();
        engine
    }

    pub fn set_sample_rate(&mut self, sr: f64) {
        self.sample_rate = if sr.is_finite() && sr > 1.0 {
            sr
        } else {
            48_000.0
        };
        self.reset_state();
        self.setup_coefficients();
    }

    pub fn reset_state(&mut self) {
        self.z1_l_stage1 = 0.0;
        self.z2_l_stage1 = 0.0;
        self.z1_r_stage1 = 0.0;
        self.z2_r_stage1 = 0.0;
        self.z1_l_stage2 = 0.0;
        self.z2_l_stage2 = 0.0;
        self.z1_r_stage2 = 0.0;
        self.z2_r_stage2 = 0.0;
    }

    pub fn setup_coefficients(&mut self) {
        let fs = self.sample_rate;
        let vh: f64 = 3.999843853973347;
        let qh = 0.7071752369554193;
        let fh = 1681.974450955531;

        let k = (std::f64::consts::PI * fh / fs).tan();
        let common = 1.0 + k / qh + k * k;

        self.b0_stage1 = ((vh + vh.sqrt() * k / qh + k * k) / common) as f32;
        self.b1_stage1 = (2.0 * (k * k - vh) / common) as f32;
        self.b2_stage1 = ((vh - vh.sqrt() * k / qh + k * k) / common) as f32;
        self.a1_stage1 = (2.0 * (k * k - 1.0) / common) as f32;
        self.a2_stage1 = ((1.0 - k / qh + k * k) / common) as f32;

        // Stage 2: RLB (High-pass)
        let f0 = 38.13547087613982;
        let q0 = 0.5003270373253953;
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let common = 1.0 + k / q0 + k * k;

        self.b0_stage2 = (1.0 / common) as f32;
        self.b1_stage2 = (-2.0 / common) as f32;
        self.b2_stage2 = (1.0 / common) as f32;
        self.a1_stage2 = (2.0 * (k * k - 1.0) / common) as f32;
        self.a2_stage2 = ((1.0 - k / q0 + k * k) / common) as f32;
    }

    /// INDUSTRIAL: Processes a stereo sample with K-Weighting.
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        fn stage(
            input: f32,
            b0: f32,
            b1: f32,
            b2: f32,
            a1: f32,
            a2: f32,
            z1: &mut f32,
            z2: &mut f32,
        ) -> f32 {
            // Transposed Direct Form II: z1/z2 are delayed state values,
            // not input history. This keeps the recursive denominator stable.
            let output = b0 * input + *z1;
            let next_z1 = b1 * input - a1 * output + *z2;
            let next_z2 = b2 * input - a2 * output;
            *z1 = if next_z1.is_finite() { next_z1 } else { 0.0 };
            *z2 = if next_z2.is_finite() { next_z2 } else { 0.0 };
            if output.is_finite() {
                output
            } else {
                0.0
            }
        }

        let v_l1 = stage(
            l,
            self.b0_stage1,
            self.b1_stage1,
            self.b2_stage1,
            self.a1_stage1,
            self.a2_stage1,
            &mut self.z1_l_stage1,
            &mut self.z2_l_stage1,
        );
        let v_r1 = stage(
            r,
            self.b0_stage1,
            self.b1_stage1,
            self.b2_stage1,
            self.a1_stage1,
            self.a2_stage1,
            &mut self.z1_r_stage1,
            &mut self.z2_r_stage1,
        );
        let out_l = stage(
            v_l1,
            self.b0_stage2,
            self.b1_stage2,
            self.b2_stage2,
            self.a1_stage2,
            self.a2_stage2,
            &mut self.z1_l_stage2,
            &mut self.z2_l_stage2,
        );
        let out_r = stage(
            v_r1,
            self.b0_stage2,
            self.b1_stage2,
            self.b2_stage2,
            self.a1_stage2,
            self.a2_stage2,
            &mut self.z1_r_stage2,
            &mut self.z2_r_stage2,
        );
        (out_l, out_r)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide K-Weighting Filter state.
    pub fn audit_k_weighting_filter(&self) -> bool {
        let finite = [
            self.sample_rate,
            self.b0_stage1 as f64,
            self.b1_stage1 as f64,
            self.b2_stage1 as f64,
            self.a1_stage1 as f64,
            self.a2_stage1 as f64,
            self.b0_stage2 as f64,
            self.b1_stage2 as f64,
            self.b2_stage2 as f64,
            self.a1_stage2 as f64,
            self.a2_stage2 as f64,
        ]
        .iter()
        .all(|v| v.is_finite())
            && self.sample_rate > 1.0;
        let stable = |a1: f32, a2: f32| {
            // Jury conditions for a(z)=1+a1*z^-1+a2*z^-2.
            a2.abs() < 1.0 && 1.0 + a1 + a2 > 0.0 && 1.0 - a1 + a2 > 0.0
        };
        finite && stable(self.a1_stage1, self.a2_stage1) && stable(self.a1_stage2, self.a2_stage2)
    }
}

#[cfg(test)]
mod tests {
    use super::KWeightingFilterEngine;

    #[test]
    fn transposed_df2_remains_finite_for_impulse_and_silence() {
        let mut filter = KWeightingFilterEngine::new(48_000.0);
        for i in 0..4096 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = filter.process(input, input);
            assert!(l.is_finite() && r.is_finite());
        }
        assert!(filter.audit_k_weighting_filter());
    }

    #[test]
    fn audit_rejects_unstable_denominator() {
        let mut filter = KWeightingFilterEngine::new(48_000.0);
        filter.a2_stage1 = 1.0;
        assert!(!filter.audit_k_weighting_filter());
    }

    #[test]
    fn sample_rate_change_clears_old_filter_history() {
        let mut filter = KWeightingFilterEngine::new(48_000.0);
        let _ = filter.process(1.0, -1.0);
        filter.set_sample_rate(96_000.0);
        assert_eq!(filter.z1_l_stage1, 0.0);
        assert_eq!(filter.z2_r_stage2, 0.0);
        assert_eq!(filter.sample_rate, 96_000.0);
    }
}
