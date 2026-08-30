pub struct BitcrusherEngine {
    pub bits: f32,
    pub downsample: f32,
    pub hold_sample_l: f32,
    pub hold_sample_r: f32,
    pub sample_counter: f64,
}

impl Default for BitcrusherEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BitcrusherEngine {
    pub fn new() -> Self {
        Self {
            bits: 16.0,
            downsample: 1.0,
            hold_sample_l: 0.0,
            hold_sample_r: 0.0,
            sample_counter: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.hold_sample_l = 0.0;
        self.hold_sample_r = 0.0;
        self.sample_counter = 0.0;
    }

    pub fn set_bits(&mut self, b: f32) {
        self.bits = if b.is_finite() {
            b.clamp(1.0, 24.0)
        } else {
            16.0
        };
    }

    pub fn set_downsample(&mut self, d: f32) {
        self.downsample = if d.is_finite() {
            d.clamp(1.0, 64.0)
        } else {
            1.0
        };
    }

    /// INDUSTRIAL: Processes an audio block with quantization and downsampling.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        let levels = 2.0f32.powf(self.bits);
        let downsample = self.downsample as f64;

        for s in 0..len {
            self.sample_counter += 1.0;

            if self.sample_counter >= downsample {
                self.sample_counter -= downsample;

                // 1. Quantization (Bit Reduction)
                let input_l = if l[s].is_finite() {
                    l[s].clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                let input_r = if r[s].is_finite() {
                    r[s].clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                self.hold_sample_l = (input_l * levels).round() / levels;
                self.hold_sample_r = (input_r * levels).round() / levels;
            }

            l[s] = self.hold_sample_l;
            r[s] = self.hold_sample_r;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Bitcrusher state.
    pub fn audit_bitcrusher(&self) -> bool {
        self.bits.is_finite()
            && (1.0..=24.0).contains(&self.bits)
            && self.downsample.is_finite()
            && (1.0..=64.0).contains(&self.downsample)
            && self.sample_counter.is_finite()
            && self.hold_sample_l.is_finite()
            && self.hold_sample_r.is_finite()
    }
}
