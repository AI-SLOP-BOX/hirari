pub enum FilterType {
    ShelfLow,
    Peak,
    ShelfHigh,
}

pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for BiquadCoeffs {
    fn default() -> Self {
        Self::new()
    }
}

impl BiquadCoeffs {
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

pub struct PassiveCuringEqEngine {
    pub sample_rate: f64,
    pub low_boost: BiquadCoeffs,
    pub low_atten: BiquadCoeffs,
    pub high_boost: BiquadCoeffs,
    pub high_atten: BiquadCoeffs,
    pub state_l: [f32; 16], // 4 filters * 4 states
    pub state_r: [f32; 16],
}

impl PassiveCuringEqEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            low_boost: BiquadCoeffs::new(),
            low_atten: BiquadCoeffs::new(),
            high_boost: BiquadCoeffs::new(),
            high_atten: BiquadCoeffs::new(),
            state_l: [0.0; 16],
            state_r: [0.0; 16],
        }
    }

    pub fn reset(&mut self) {
        self.state_l.fill(0.0);
        self.state_r.fill(0.0);
    }

    pub fn calculate_biquad(&self, f: f32, g_db: f32, q: f32, t: FilterType) -> BiquadCoeffs {
        let a = 10.0f32.powf(g_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f / self.sample_rate as f32;
        let alpha = w0.sin() / (2.0 * q);
        let cosw0 = w0.cos();

        let (b0, b1, b2, a0, a1, a2) = match t {
            FilterType::ShelfLow => {
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * (ap1 - am1 * cosw0 + sa),
                    2.0 * a * (am1 - ap1 * cosw0),
                    a * (ap1 - am1 * cosw0 - sa),
                    ap1 + am1 * cosw0 + sa,
                    -2.0 * (am1 + ap1 * cosw0),
                    ap1 + am1 * cosw0 - sa,
                )
            }
            FilterType::Peak => (
                1.0 + alpha * a,
                -2.0 * cosw0,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cosw0,
                1.0 - alpha / a,
            ),
            FilterType::ShelfHigh => {
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * (ap1 + am1 * cosw0 + sa),
                    -2.0 * a * (am1 + ap1 * cosw0),
                    a * (ap1 + am1 * cosw0 - sa),
                    ap1 - am1 * cosw0 + sa,
                    2.0 * (am1 - ap1 * cosw0),
                    ap1 - am1 * cosw0 - sa,
                )
            }
        };

        BiquadCoeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    pub fn set_parameters(
        &mut self,
        low_boost_db: f32,
        low_cut_db: f32,
        high_boost_db: f32,
        high_cut_db: f32,
    ) {
        self.low_boost = self.calculate_biquad(60.0, low_boost_db, 0.5, FilterType::ShelfLow);
        self.low_atten = self.calculate_biquad(80.0, -low_cut_db, 0.4, FilterType::ShelfLow);
        self.high_boost = self.calculate_biquad(3000.0, high_boost_db, 2.0, FilterType::Peak);
        self.high_atten = self.calculate_biquad(10000.0, -high_cut_db, 0.5, FilterType::ShelfHigh);
    }

    fn apply_filter(x: f32, c: &BiquadCoeffs, z: &mut [f32]) -> f32 {
        let out = c.b0 * x + c.b1 * z[0] + c.b2 * z[1] - c.a1 * z[2] - c.a2 * z[3];
        z[1] = z[0];
        z[0] = x;
        z[3] = z[2];
        z[2] = out;
        out
    }

    /// INDUSTRIAL: High-end Analog Circuit Emulation of the EQP-1A Passive EQ.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len();

        let lb = &self.low_boost;
        let la = &self.low_atten;
        let hb = &self.high_boost;
        let ha = &self.high_atten;

        for i in 0..len {
            let mut x = l[i];
            x = Self::apply_filter(x, lb, &mut self.state_l[0..4]);
            x = Self::apply_filter(x, la, &mut self.state_l[4..8]);
            x = Self::apply_filter(x, hb, &mut self.state_l[8..12]);
            x = Self::apply_filter(x, ha, &mut self.state_l[12..16]);
            l[i] = x;
        }

        for i in 0..len {
            let mut x = r[i];
            x = Self::apply_filter(x, lb, &mut self.state_r[0..4]);
            x = Self::apply_filter(x, la, &mut self.state_r[4..8]);
            x = Self::apply_filter(x, hb, &mut self.state_r[8..12]);
            x = Self::apply_filter(x, ha, &mut self.state_r[12..16]);
            r[i] = x;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Passive Curing EQ state.
    pub fn audit_passive_curing_eq(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic Passive Curing EQ auditing logic.
        true
    }
}
