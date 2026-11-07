use crate::delay_line::DelayLineEngine;

pub struct TapeMachineEngine {
    pub sample_rate: f64,
    pub delay_l: DelayLineEngine,
    pub delay_r: DelayLineEngine,
    pub lfo_phase: f32,
    pub flutter_phase: f32,
    pub drive: f32,
    pub flutter: f32,
    pub noise: f32,
    pub noise_state: u32,
}

impl TapeMachineEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            delay_l: DelayLineEngine::new(8192),
            delay_r: DelayLineEngine::new(8192),
            lfo_phase: 0.0,
            flutter_phase: 0.0,
            drive: 0.0,
            flutter: 0.01,
            noise: 0.001,
            noise_state: 12345, // Seed for simple LCG
        }
    }

    pub fn reset(&mut self) {
        self.delay_l.reset();
        self.delay_r.reset();
        self.lfo_phase = 0.0;
        self.flutter_phase = 0.0;
    }

    pub fn set_drive(&mut self, db: f32) {
        self.drive = db;
    }

    pub fn set_flutter(&mut self, f: f32) {
        self.flutter = f.clamp(0.0, 1.0);
    }

    pub fn set_noise(&mut self, n: f32) {
        self.noise = n.clamp(0.0, 0.01);
    }

    fn next_noise(&mut self) -> f32 {
        // Simple LCG for noise generation to avoid external dependencies
        self.noise_state = self
            .noise_state
            .wrapping_mul(1103515245)
            .wrapping_add(12345)
            & 0x7fffffff;
        (self.noise_state as f32 / 0x7fffffff as f32) - 0.5
    }

    /// INDUSTRIAL: Applies magnetic character and speed instability.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite() || self.sample_rate <= 100.0 {
            return;
        }
        self.drive = if self.drive.is_finite() {
            self.drive.clamp(-60.0, 24.0)
        } else {
            0.0
        };
        self.noise = if self.noise.is_finite() {
            self.noise.clamp(0.0, 0.01)
        } else {
            0.0
        };
        let drive_lin = 10.0f32.powf(self.drive / 20.0);

        for s in 0..len {
            // 1. Wow & Flutter (Slow/Fast time modulation)
            self.lfo_phase += 0.5 / self.sample_rate as f32; // Wow (0.5Hz)
            self.flutter_phase += 5.0 / self.sample_rate as f32; // Flutter (5.0Hz)
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }
            if self.flutter_phase >= 1.0 {
                self.flutter_phase -= 1.0;
            }

            let mod_val = (self.flutter * 0.5)
                * (2.0 * std::f32::consts::PI * self.lfo_phase).sin()
                + (self.flutter * 0.2) * (2.0 * std::f32::consts::PI * self.flutter_phase).sin();

            let delay_samps = 4.0 + mod_val * 400.0;

            // Left
            let in_l = (if l[s].is_finite() { l[s] } else { 0.0 }) * drive_lin;
            let saturated_l = if in_l > 0.0 {
                in_l / (1.0 + in_l)
            } else {
                in_l / (1.0 - in_l)
            };
            let fluttered_l = self.delay_l.process(saturated_l, delay_samps);
            let hiss_l = self.next_noise() * self.noise;
            l[s] = fluttered_l + hiss_l;

            // Right
            let in_r = (if r[s].is_finite() { r[s] } else { 0.0 }) * drive_lin;
            let saturated_r = if in_r > 0.0 {
                in_r / (1.0 + in_r)
            } else {
                in_r / (1.0 - in_r)
            };
            let fluttered_r = self.delay_r.process(saturated_r, delay_samps);
            let hiss_r = self.next_noise() * self.noise;
            r[s] = (fluttered_r + hiss_r).clamp(-4.0, 4.0);
            l[s] = l[s].clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Tape Machine state.
    pub fn audit_tape_machine(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.lfo_phase.is_finite()
            && self.flutter_phase.is_finite()
            && self.drive.is_finite()
            && self.flutter.is_finite()
            && (0.0..=1.0).contains(&self.flutter)
            && self.noise.is_finite()
            && (0.0..=0.01).contains(&self.noise)
            && self.noise_state != 0
            && self.delay_l.audit_delay_line()
            && self.delay_r.audit_delay_line()
    }
}
