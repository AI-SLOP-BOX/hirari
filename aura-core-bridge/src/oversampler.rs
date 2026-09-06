pub struct OversamplerEngine {
    pub a1: f32,
    pub a2: f32,
    pub s1_l: f32,
    pub s1_r: f32,
    pub s2_l: f32,
    pub s2_r: f32,
}

impl Default for OversamplerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OversamplerEngine {
    pub fn new() -> Self {
        Self {
            a1: 0.129_676_55,
            a2: 0.484_189_24,
            s1_l: 0.0,
            s1_r: 0.0,
            s2_l: 0.0,
            s2_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.s1_l = 0.0;
        self.s1_r = 0.0;
        self.s2_l = 0.0;
        self.s2_r = 0.0;
    }

    /// INDUSTRIAL: UPSAMPLE: 1 in -> 2 out.
    pub fn upsample(&mut self, x: f32) -> (f32, f32) {
        let x = if x.is_finite() {
            x.clamp(-4.0, 4.0)
        } else {
            0.0
        };
        self.sanitize();
        // Stage 1 (All-pass 1)
        let v1 = x - self.a1 * self.s1_l;
        let y1 = self.s1_l + self.a1 * v1;
        self.s1_l = v1;

        // Stage 2 (All-pass 2)
        let v2 = x - self.a2 * self.s2_l;
        let y2 = self.s2_l + self.a2 * v2;
        self.s2_l = v2;

        (y1, y2)
    }

    /// INDUSTRIAL: DOWNSAMPLE: 2 in -> 1 out.
    pub fn downsample(&mut self, y1: f32, y2: f32) -> f32 {
        self.sanitize();
        let y1 = if y1.is_finite() {
            y1.clamp(-4.0, 4.0)
        } else {
            0.0
        };
        let y2 = if y2.is_finite() {
            y2.clamp(-4.0, 4.0)
        } else {
            0.0
        };
        // Polyphase IIR Downsampling (Dual of upsampling)
        let v1 = y1 - self.a1 * self.s1_r;
        let out1 = self.s1_r + self.a1 * v1;
        self.s1_r = v1;

        let v2 = y2 - self.a2 * self.s2_r;
        let out2 = self.s2_r + self.a2 * v2;
        self.s2_r = v2;

        let output = (out1 + out2) * 0.5;
        if output.is_finite() {
            output.clamp(-4.0, 4.0)
        } else {
            0.0
        }
    }

    fn sanitize(&mut self) {
        if !self.a1.is_finite() || !(-1.0..=1.0).contains(&self.a1) {
            self.a1 = 0.129_676_55;
        }
        if !self.a2.is_finite() || !(-1.0..=1.0).contains(&self.a2) {
            self.a2 = 0.484_189_24;
        }
        for state in [
            &mut self.s1_l,
            &mut self.s1_r,
            &mut self.s2_l,
            &mut self.s2_r,
        ] {
            if !state.is_finite() {
                *state = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OversamplerEngine;

    #[test]
    fn oversampler_sanitizes_corrupt_state_and_audio() {
        let mut engine = OversamplerEngine::new();
        engine.a1 = f32::NAN;
        engine.s1_l = f32::INFINITY;
        let (a, b) = engine.upsample(f32::NAN);
        assert!(a.is_finite() && b.is_finite());
        let out = engine.downsample(f32::INFINITY, f32::NEG_INFINITY);
        assert!(out.is_finite() && out.abs() <= 4.0);
    }
}
