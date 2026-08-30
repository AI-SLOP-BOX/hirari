pub struct SimpleBP {
    pub z1: f32,
    pub z2: f32,
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for SimpleBP {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleBP {
    pub fn new() -> Self {
        Self {
            z1: 0.0,
            z2: 0.0,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    pub fn process(&mut self, in_val: f32) -> f32 {
        let out = self.b0 * in_val + self.b1 * self.z1 + self.b2 * self.z2
            - self.a1 * self.z1
            - self.a2 * self.z2;
        self.z2 = self.z1;
        self.z1 = out;
        out
    }
}

// 2nd-order Butterworth filter state
pub struct BiquadFilter {
    pub z1: f32,
    pub z2: f32,
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for BiquadFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl BiquadFilter {
    pub fn new() -> Self {
        Self {
            z1: 0.0,
            z2: 0.0,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.z1 + self.b2 * self.z2
            - self.a1 * self.z1
            - self.a2 * self.z2;
        self.z2 = self.z1;
        self.z1 = y;
        y
    }
}

pub struct DeEsserEngine {
    pub sample_rate: f64,
    pub threshold: f32,
    pub intensity: f32,
    pub env: f32,
    pub current_gain: f32,
    pub sc_filter: SimpleBP,

    // Split-band crossover filters at 4000 Hz
    pub lpf_l: BiquadFilter,
    pub lpf_r: BiquadFilter,
    pub hpf_l: BiquadFilter,
    pub hpf_r: BiquadFilter,
}

impl DeEsserEngine {
    pub fn new(sr: f64) -> Self {
        let mut engine = Self {
            sample_rate: sr,
            threshold: 0.5,
            intensity: 0.8,
            env: 0.0,
            current_gain: 1.0,
            sc_filter: SimpleBP::new(),
            lpf_l: BiquadFilter::new(),
            lpf_r: BiquadFilter::new(),
            hpf_l: BiquadFilter::new(),
            hpf_r: BiquadFilter::new(),
        };
        engine.update_filters();
        engine
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.current_gain = 1.0;
        self.sc_filter.reset();
        self.lpf_l.reset();
        self.lpf_r.reset();
        self.hpf_l.reset();
        self.hpf_r.reset();
    }

    pub fn update_filters(&mut self) {
        // Keep all design frequencies below Nyquist.  The margin also keeps
        // the trigonometric terms well away from the singular point.
        let valid_sr = self.sample_rate.is_finite() && self.sample_rate > 0.0;
        let max_freq = if valid_sr {
            self.sample_rate * 0.49
        } else {
            0.0
        };
        let design_freq = |freq: f64| {
            if max_freq > 0.0 {
                freq.min(max_freq)
            } else {
                0.0
            }
        };

        // 1. Professional 6kHz Bandpass Sidechain (Q=1.0)
        let w0 = if valid_sr {
            2.0 * std::f64::consts::PI * design_freq(6000.0) / self.sample_rate
        } else {
            0.0
        };
        let alpha = w0.sin() / 2.0; // Q = 1.0
        let a0 = 1.0 + alpha;

        self.sc_filter.b0 = (alpha / a0) as f32;
        self.sc_filter.b1 = 0.0;
        self.sc_filter.b2 = (-alpha / a0) as f32;
        self.sc_filter.a1 = (-2.0 * w0.cos() / a0) as f32;
        self.sc_filter.a2 = ((1.0 - alpha) / a0) as f32;

        // 2. 2nd-order Butterworth Crossover Filters at 4000 Hz
        let fc = design_freq(4000.0);
        let w0_c = if valid_sr {
            2.0 * std::f64::consts::PI * fc / self.sample_rate
        } else {
            0.0
        };
        let cos_w0 = w0_c.cos();
        let sin_w0 = w0_c.sin();
        let q = std::f64::consts::FRAC_1_SQRT_2; // 0.7071 (Butterworth)
        let alpha_c = sin_w0 / (2.0 * q);
        let a0_c = 1.0 + alpha_c;

        // Low-pass coefficients
        let l_b0 = ((1.0 - cos_w0) * 0.5 / a0_c) as f32;
        let l_b1 = ((1.0 - cos_w0) / a0_c) as f32;
        let l_b2 = l_b0;
        let l_a1 = (-2.0 * cos_w0 / a0_c) as f32;
        let l_a2 = ((1.0 - alpha_c) / a0_c) as f32;

        // High-pass coefficients
        let h_b0 = ((1.0 + cos_w0) * 0.5 / a0_c) as f32;
        let h_b1 = (-(1.0 + cos_w0) / a0_c) as f32;
        let h_b2 = h_b0;
        let h_a1 = l_a1;
        let h_a2 = l_a2;

        let apply_coeffs = |f: &mut BiquadFilter, b0, b1, b2, a1, a2| {
            f.b0 = b0;
            f.b1 = b1;
            f.b2 = b2;
            f.a1 = a1;
            f.a2 = a2;
        };

        apply_coeffs(&mut self.lpf_l, l_b0, l_b1, l_b2, l_a1, l_a2);
        apply_coeffs(&mut self.lpf_r, l_b0, l_b1, l_b2, l_a1, l_a2);
        apply_coeffs(&mut self.hpf_l, h_b0, h_b1, h_b2, h_a1, h_a2);
        apply_coeffs(&mut self.hpf_r, h_b0, h_b1, h_b2, h_a1, h_a2);
    }

    /**
     * @brief PROCESS: Splits input into Low/High bands, detects sibilance,
     * attenuates ONLY the High band, and recombines the results.
     * INDUSTRIAL: Resolves broadband pumping artifacts completely.
     */
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());

        for s in 0..len {
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            let mid = (in_l + in_r) * 0.5;

            // 1. Detect Sibilance (Bandpass Filter 6kHz)
            let sibilance = self.sc_filter.process(mid);
            let peak = sibilance.abs();

            // 2. Ballistics (Fast Attack, Moderate Release)
            if peak > self.env {
                self.env = 0.9 * self.env + 0.1 * peak;
            } else {
                self.env *= 0.998; // Smooth, realistic release decay
            }

            // 3. Calculate Gain Reduction factor
            let mut target_gain = 1.0;
            if self.env > self.threshold {
                target_gain = 1.0 - (self.env - self.threshold) * self.intensity;
                target_gain = target_gain.max(0.1); // Max 20 dB reduction to keep vocal clear
            }
            self.current_gain = 0.95 * self.current_gain + 0.05 * target_gain;

            // 4. Split-Band Processing (LPF and HPF)
            let low_l = self.lpf_l.process(in_l);
            let low_r = self.lpf_r.process(in_r);
            let high_l = self.hpf_l.process(in_l);
            let high_r = self.hpf_r.process(in_r);

            // 5. Recombine: Low band untouched + ducked High band
            l[s] = (low_l + high_l * self.current_gain).clamp(-1.0, 1.0);
            r[s] = (low_r + high_r * self.current_gain).clamp(-1.0, 1.0);
        }
    }

    pub fn audit_deesser(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.threshold.is_finite()
            && self.intensity.is_finite()
            && self.env.is_finite()
            && self.current_gain.is_finite()
            && self.current_gain >= 0.0
            && self.current_gain <= 1.0
    }
}
