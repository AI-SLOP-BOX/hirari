pub struct TubeSaturationEngine {
    pub drive: f32,
    pub bias: f32,
    pub dry_wet: f32,
}

impl Default for TubeSaturationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TubeSaturationEngine {
    pub fn new() -> Self {
        Self {
            drive: 0.0,
            bias: 0.0,
            dry_wet: 1.0,
        }
    }

    /// Restore the parameter defaults. The processor is stateless between
    /// blocks, so reset intentionally does not clear an audio history buffer.
    pub fn reset(&mut self) {
        self.drive = 0.0;
        self.bias = 0.0;
        self.dry_wet = 1.0;
    }

    pub fn set_drive(&mut self, db: f32) {
        if db.is_finite() { self.drive = db.clamp(-60.0, 24.0); }
    }

    pub fn set_bias(&mut self, b: f32) {
        if b.is_finite() { self.bias = b.clamp(-1.0, 1.0); }
    }

    pub fn set_dry_wet(&mut self, mix: f32) {
        if mix.is_finite() { self.dry_wet = mix.clamp(0.0, 1.0); }
    }

    /// INDUSTRIAL: Applies the non-linear transfer function.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_tube_saturation() { return; }
        let len = l.len().min(r.len());
        let drive_lin = 10.0f32.powf(self.drive / 20.0);

        for s in 0..len {
            // Left
            let in_l = (if l[s].is_finite() { l[s] } else { 0.0 }) * drive_lin + self.bias;
            let saturated_l = if in_l > 0.0 {
                in_l / (1.0 + in_l)
            } else {
                in_l / (1.0 - in_l)
            };
            let compensated_l = saturated_l - self.bias * 0.5;
            l[s] = ((if l[s].is_finite() { l[s] } else { 0.0 }) * (1.0 - self.dry_wet) + compensated_l * self.dry_wet).clamp(-2.0, 2.0);

            // Right
            let in_r = (if r[s].is_finite() { r[s] } else { 0.0 }) * drive_lin + self.bias;
            let saturated_r = if in_r > 0.0 {
                in_r / (1.0 + in_r)
            } else {
                in_r / (1.0 - in_r)
            };
            let compensated_r = saturated_r - self.bias * 0.5;
            r[s] = ((if r[s].is_finite() { r[s] } else { 0.0 }) * (1.0 - self.dry_wet) + compensated_r * self.dry_wet).clamp(-2.0, 2.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Tube Saturation state.
    pub fn audit_tube_saturation(&self) -> bool {
        self.drive.is_finite() && (-60.0..=24.0).contains(&self.drive)
            && self.bias.is_finite() && (-1.0..=1.0).contains(&self.bias)
            && self.dry_wet.is_finite() && (0.0..=1.0).contains(&self.dry_wet)
    }
}

#[cfg(test)]
mod tests {
    use super::TubeSaturationEngine;

    #[test]
    fn reset_restores_parameter_defaults() {
        let mut engine = TubeSaturationEngine::new();
        engine.set_drive(12.0);
        engine.set_bias(0.5);
        engine.set_dry_wet(0.25);
        engine.reset();
        assert_eq!(engine.drive, 0.0);
        assert_eq!(engine.bias, 0.0);
        assert_eq!(engine.dry_wet, 1.0);
    }
}
