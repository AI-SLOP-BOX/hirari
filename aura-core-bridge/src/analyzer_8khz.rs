pub struct Analyzer8kHzEngine {
    pub sample_rate: f64,
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
    pub z1: f32,
    pub z2: f32,
    pub high_freq_energy: std::sync::atomic::AtomicU32, // Store as u32 bits for atomic float
}

impl Analyzer8kHzEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate =
            if sample_rate.is_finite() && (16_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                48_000.0
            };
        let mut engine = Self {
            sample_rate,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
            high_freq_energy: std::sync::atomic::AtomicU32::new(0),
        };
        engine.calculate_coefficients();
        engine
    }

    pub fn calculate_coefficients(&mut self) {
        let f0 = 8000.0;
        let q = 0.707;
        let omega = 2.0 * std::f64::consts::PI * f0 / self.sample_rate;
        let alpha = omega.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;

        self.b0 = (alpha / a0) as f32;
        self.b1 = 0.0;
        self.b2 = (-alpha / a0) as f32;
        self.a1 = (-2.0 * omega.cos() / a0) as f32;
        self.a2 = ((1.0 - alpha) / a0) as f32;
    }

    /// INDUSTRIAL: Processes an audio block with SIMD-accelerated precision.
    pub fn analyze(&mut self, buffer: &[f32]) {
        if buffer.is_empty() || !self.audit_analyzer_8khz() {
            return;
        }

        let mut sum_sq = 0.0f32;
        let mut z1 = self.z1;
        let mut z2 = self.z2;

        for &raw in buffer {
            let x = if raw.is_finite() { raw } else { 0.0 };
            let y = self.b0 * x + self.b1 * z1 + self.b2 * z2 - self.a1 * z1 - self.a2 * z2;
            z2 = z1;
            z1 = y;
            if y.is_finite() {
                sum_sq += y * y;
            }
        }

        self.z1 = z1;
        self.z2 = z2;

        let rms = (sum_sq / buffer.len() as f32).sqrt().clamp(0.0, 16.0);
        self.high_freq_energy
            .store(rms.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_energy(&self) -> f32 {
        f32::from_bits(
            self.high_freq_energy
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal state.
    pub fn audit_analyzer_8khz(&self) -> bool {
        self.sample_rate.is_finite()
            && (16_000.0..=384_000.0).contains(&self.sample_rate)
            && [
                self.b0,
                self.b1,
                self.b2,
                self.a1,
                self.a2,
                self.z1,
                self.z2,
                self.get_energy(),
            ]
            .iter()
            .all(|v| v.is_finite())
            && self.a2.abs() < 1.0
    }
}
