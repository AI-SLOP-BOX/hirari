pub struct DynamicsSuiteEngine {
    pub gate_thresh_db: f32,
    pub sidechain_thresh_db: f32,
}

impl Default for DynamicsSuiteEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicsSuiteEngine {
    pub fn new() -> Self {
        Self {
            gate_thresh_db: -60.0,
            sidechain_thresh_db: -20.0,
        }
    }

    /// INDUSTRIAL: Processes an audio block with gate, ducking, and limiting.
    pub fn process(&self, l: &mut [f32], r: &mut [f32], sidechain: Option<&[f32]>) {
        if !self.audit_dynamics_suite() {
            return;
        }
        let num_frames = l.len().min(r.len());
        let gate_linear = 10.0f32.powf(self.gate_thresh_db / 20.0);
        let sc_linear = 10.0f32.powf(self.sidechain_thresh_db / 20.0);

        for i in 0..num_frames {
            let left = if l[i].is_finite() { l[i] } else { 0.0 };
            let right = if r[i].is_finite() { r[i] } else { 0.0 };
            let energy = (left.abs() + right.abs()) * 0.5;
            l[i] = left;
            r[i] = right;

            // 1. GATE
            if energy < gate_linear {
                l[i] = 0.0;
                r[i] = 0.0;
            }

            // 2. SIDECHAIN DUCKING
            if let Some(sc) = sidechain {
                if i < sc.len() {
                    let sc_energy = if sc[i].is_finite() { sc[i].abs() } else { 0.0 };
                    if sc_energy > sc_linear {
                        let att = 0.5; // Fixed ducking
                        l[i] *= att;
                        r[i] *= att;
                    }
                }
            }

            // 3. MASTER LIMITER
            let peak = l[i].abs().max(r[i].abs());
            if peak > 0.99 {
                let scale = 0.99 / peak;
                l[i] *= scale;
                r[i] *= scale;
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide dynamics state.
    pub fn audit_dynamics_suite(&self) -> bool {
        self.gate_thresh_db.is_finite()
            && (-120.0..=0.0).contains(&self.gate_thresh_db)
            && self.sidechain_thresh_db.is_finite()
            && (-120.0..=0.0).contains(&self.sidechain_thresh_db)
    }
}
