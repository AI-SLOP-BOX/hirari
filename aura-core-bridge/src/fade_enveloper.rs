#[derive(Debug, Clone, Copy)]
pub enum FadeCurve {
    Linear,
    EqualPower,
    EaseInOut,
    Bezier,
}

pub struct FadeOrchestrator {
    pub eq_power_lut: Vec<f32>,
    pub ease_in_out_lut: Vec<f32>,
}

impl Default for FadeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl FadeOrchestrator {
    pub fn new() -> Self {
        let mut eq_power_lut = vec![0.0f32; 1024];
        let mut ease_in_out_lut = vec![0.0f32; 1024];

        for i in 0..1024 {
            let x = i as f32 / 1023.0;
            eq_power_lut[i] = x.sqrt();
            ease_in_out_lut[i] = x * x * (3.0 - 2.0 * x);
        }

        Self {
            eq_power_lut,
            ease_in_out_lut,
        }
    }

    /// INDUSTRIAL: High-Precision Gain Calculation for Fades
    pub fn get_fade_factor(
        &self,
        pos: usize,
        length: usize,
        is_fade_in: bool,
        curve_type: FadeCurve,
        curvature: f32,
    ) -> f32 {
        if length == 0 || pos >= length {
            return if is_fade_in { 1.0 } else { 0.0 };
        }

        let mut x = pos as f32 / length as f32;
        if !is_fade_in {
            x = 1.0 - x;
        }

        match curve_type {
            FadeCurve::Linear => x,
            FadeCurve::EqualPower => {
                let index_f = x * 1023.0;
                let idx = index_f as usize;
                let frac = index_f - idx as f32;
                let v1 = self.eq_power_lut[idx];
                let v2 = self.eq_power_lut[std::cmp::min(idx + 1, 1023)];
                v1 + frac * (v2 - v1)
            }
            FadeCurve::EaseInOut => {
                let index_f = x * 1023.0;
                let idx = index_f as usize;
                let frac = index_f - idx as f32;
                let v1 = self.ease_in_out_lut[idx];
                let v2 = self.ease_in_out_lut[std::cmp::min(idx + 1, 1023)];
                v1 + frac * (v2 - v1)
            }
            FadeCurve::Bezier => {
                let c = curvature.clamp(0.01, 0.99);
                let t = x;
                let one_minus_t = 1.0 - t;

                let p1y = if c < 0.5 { 0.0 } else { (c - 0.5) * 2.0 };
                let p2y = if c > 0.5 { 1.0 } else { c * 2.0 };

                let y = 3.0 * one_minus_t * one_minus_t * t * p1y
                    + 3.0 * one_minus_t * t * t * p2y
                    + t * t * t * 1.0;

                y.clamp(0.0, 1.0)
            }
        }
    }

    /// INDUSTRIAL: Applies a crossfade between two buffers.
    pub fn apply(&self, out: &mut [f32], in1: &[f32], in2: &[f32]) {
        // Crossfades are fed by regions with independently edited lengths.
        // Never index past the shorter source and avoid 0/0 for empty edits.
        let num_frames = out.len().min(in1.len()).min(in2.len());
        if num_frames == 0 {
            out.fill(0.0);
            return;
        }
        for i in 0..num_frames {
            let t = if num_frames == 1 {
                1.0
            } else {
                i as f32 / (num_frames - 1) as f32
            };
            let gain1 = (t * std::f32::consts::PI * 0.5).cos();
            let gain2 = (t * std::f32::consts::PI * 0.5).sin();
            out[i] = (in1[i] * gain1) + (in2[i] * gain2);
        }
        out[num_frames..].fill(0.0);
    }

    /// INDUSTRIAL: Automatically prevents clicks at region boundaries.
    pub fn apply_micro_fade(&self, buffer: &mut [f32], is_fade_in: bool) {
        let num_frames = buffer.len();
        let fade_len = std::cmp::min(num_frames, 256); // ~5ms at 44.1k
        for i in 0..fade_len {
            let mut g = i as f32 / fade_len as f32;
            if !is_fade_in {
                g = 1.0 - g;
            }

            let idx = if is_fade_in { i } else { num_frames - 1 - i };
            buffer[idx] *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FadeOrchestrator;

    #[test]
    fn crossfade_handles_empty_and_short_sources() {
        let fades = FadeOrchestrator::new();
        let mut empty = [1.0f32];
        fades.apply(&mut empty, &[], &[]);
        assert_eq!(empty, [0.0]);

        let mut output = [9.0f32; 4];
        fades.apply(&mut output, &[1.0, 1.0], &[0.0, 0.0]);
        assert!(output[0].is_finite());
        assert!(output[1].is_finite());
        assert_eq!(&output[2..], &[0.0, 0.0]);
    }

    #[test]
    fn single_sample_crossfade_is_finite() {
        let fades = FadeOrchestrator::new();
        let mut output = [0.0f32];
        fades.apply(&mut output, &[1.0], &[1.0]);
        assert!(output[0].is_finite());
    }
}
