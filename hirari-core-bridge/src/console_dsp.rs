pub struct DivineConsoleEngine {
    pub sample_rate: f64,
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub gain_env: f32,
    pub drive: f32,
}

impl DivineConsoleEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                48_000.0
            },
            threshold: 0.5, // -6dB ballpark
            ratio: 4.0,
            attack: 0.01,
            release: 0.1,
            gain_env: 1.0,
            drive: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.gain_env = 1.0;
    }

    pub fn set_params(&mut self, threshold_db: f32, ratio: f32) {
        if threshold_db.is_finite()
            && (-120.0..=0.0).contains(&threshold_db)
            && ratio.is_finite()
            && (1.0..=100.0).contains(&ratio)
        {
            self.threshold = 10.0f32.powf(threshold_db / 20.0);
            self.ratio = ratio;
        }
    }

    pub fn set_drive(&mut self, drive: f32) {
        if drive.is_finite() && (0.0..=8.0).contains(&drive) {
            self.drive = 1.0 + drive;
        }
    }

    fn soft_clip(&self, x: f32) -> f32 {
        let driven = x * self.drive;
        let abs_x = driven.abs();
        if abs_x < 0.8 {
            return driven;
        }
        let sign = if driven > 0.0 { 1.0 } else { -1.0 };
        sign * (0.8 + 0.2 * ((abs_x - 0.8) / 0.2).tanh())
    }

    /// INDUSTRIAL: SSL-Inspired Master Buss Processor with VCA Compression and Harmonic Saturation.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], sidechain: Option<&[f32]>) {
        let len = l.len().min(r.len());
        if len == 0 || !self.audit_divine_console() {
            return;
        }

        for i in 0..len {
            // 1. Sidechain: Stereo RMS-ish peak detection
            let sc = if let Some(sc_buf) = sidechain {
                sc_buf.get(i).copied().unwrap_or(0.0).abs()
            } else {
                ((if l[i].is_finite() { l[i] } else { 0.0 })
                    + if r[i].is_finite() { r[i] } else { 0.0 })
                .abs()
                    * 0.5
            };

            // 2. VCA Compression Logic
            let mut reduction = 1.0;
            if sc > self.threshold {
                let db_over = 20.0 * (sc / self.threshold).log10();
                let db_reduced = db_over / self.ratio;
                reduction = 10.0f32.powf((db_reduced - db_over) / 20.0);
            }

            // Smoothing (Attack/Release)
            let coeff = if reduction < self.gain_env {
                self.attack
            } else {
                self.release
            };
            self.gain_env += (reduction - self.gain_env) * coeff;

            // 3. Apply Gain + Glue
            let mut out_l = (if l[i].is_finite() { l[i] } else { 0.0 }) * self.gain_env;
            let mut out_r = (if r[i].is_finite() { r[i] } else { 0.0 }) * self.gain_env;

            // 4. Harmonic Saturation (Divine Warmth) - Sovereign Soft Clipper
            out_l = self.soft_clip(out_l);
            out_r = self.soft_clip(out_r);

            l[i] = if out_l.is_finite() {
                out_l.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            r[i] = if out_r.is_finite() {
                out_r.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Divine Console state.
    pub fn audit_divine_console(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.threshold.is_finite()
            && (1.0e-6..=1.0).contains(&self.threshold)
            && self.ratio.is_finite()
            && (1.0..=100.0).contains(&self.ratio)
            && self.attack.is_finite()
            && (0.0..=1.0).contains(&self.attack)
            && self.release.is_finite()
            && (0.0..=1.0).contains(&self.release)
            && self.gain_env.is_finite()
            && (0.0..=1.0).contains(&self.gain_env)
            && self.drive.is_finite()
            && (1.0..=9.0).contains(&self.drive)
    }
}

#[cfg(test)]
mod tests {
    use super::DivineConsoleEngine;

    #[test]
    fn console_process_is_safe_for_short_sidechain_and_nonfinite_audio() {
        let mut console = DivineConsoleEngine::new(48_000.0);
        let mut left = vec![f32::NAN, 0.5, 2.0];
        let mut right = vec![0.25, 0.25];
        console.process(&mut left, &mut right, Some(&[0.8]));
        assert!(left[..2]
            .iter()
            .chain(right.iter())
            .all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(console.audit_divine_console());
    }
}
