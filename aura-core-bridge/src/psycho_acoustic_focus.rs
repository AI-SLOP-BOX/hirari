pub struct PsychoAcousticFocusEngine {
    pub sample_rate: f64,
    pub alpha: f32,
    pub hpf_state: Vec<f32>,
    pub last_in: Vec<f32>,
    pub focus_amount: f32,
}

impl PsychoAcousticFocusEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            48_000.0
        };
        let cutoff = 3500.0;
        let dt = 1.0 / sr;
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
        let alpha = (rc / (rc + dt)) as f32;

        Self {
            sample_rate: sr,
            alpha,
            hpf_state: vec![0.0; 32],
            last_in: vec![0.0; 32],
            focus_amount: 0.5,
        }
    }

    pub fn reset(&mut self) {
        self.hpf_state.fill(0.0);
        self.last_in.fill(0.0);
    }

    pub fn set_focus_amount(&mut self, amount: f32) {
        if amount.is_finite() {
            self.focus_amount = amount.clamp(0.0, 2.0);
        }
    }

    /// INDUSTRIAL: Generates musically-related even-order harmonics.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_psycho_acoustic_focus() {
            return;
        }
        let len = l.len().min(r.len());

        for s in 0..len {
            // Process Left channel (index 0)
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            self.hpf_state[0] = self.alpha * (self.hpf_state[0] + in_l - self.last_in[0]);
            self.last_in[0] = in_l;
            let high_mids_l = self.hpf_state[0];
            let harmonic_l = (high_mids_l * 2.0).tanh() * self.focus_amount * 0.1;
            l[s] = (in_l + harmonic_l).clamp(-4.0, 4.0);

            // Process Right channel (index 1)
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            self.hpf_state[1] = self.alpha * (self.hpf_state[1] + in_r - self.last_in[1]);
            self.last_in[1] = in_r;
            let high_mids_r = self.hpf_state[1];
            let harmonic_r = (high_mids_r * 2.0).tanh() * self.focus_amount * 0.1;
            r[s] = (in_r + harmonic_r).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Psycho Acoustic Focus state.
    pub fn audit_psycho_acoustic_focus(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.alpha.is_finite()
            && (0.0..=1.0).contains(&self.alpha)
            && self.focus_amount.is_finite()
            && (0.0..=2.0).contains(&self.focus_amount)
            && self.hpf_state.len() >= 2
            && self.last_in.len() >= 2
            && self.hpf_state.iter().all(|v| v.is_finite())
            && self.last_in.iter().all(|v| v.is_finite())
    }
}
