use std::ffi::c_void;

#[allow(dead_code)]
struct ChromaGlow {
    sample_rate: f64,
    gain: f32,
    mix: f32,
    mode: u32,
}

impl ChromaGlow {
    fn valid_rate(rate: f64) -> bool {
        rate.is_finite() && (8_000.0..=384_000.0).contains(&rate)
    }

    fn prepare(&mut self, rate: f64) {
        self.sample_rate = if Self::valid_rate(rate) { rate } else { 44_100.0 };
    }

    fn set_params(&mut self, drive_db: f32, character: f32, mode: u32) {
        let db = (if drive_db.is_finite() { drive_db } else { 0.0 }).clamp(-24.0, 36.0);
        self.gain = 10.0f32.powf(db / 20.0);
        self.mix = (if character.is_finite() { character } else { 0.0 }).clamp(0.0, 1.0);
        self.mode = mode;
    }

    fn set_parameter(&mut self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        if id == 0 {
            self.gain = 10.0f32.powf((-24.0 + value.clamp(0.0, 1.0) * 60.0) / 20.0);
        } else if id == 1 {
            self.mix = value.clamp(0.0, 1.0);
        } else if id == 2 {
            self.mode = ((value * 2.0).round() as i32).clamp(0, 2) as u32;
        }
    }

    fn get_parameter(&self, id: u32) -> f32 {
        if id == 0 {
            return ((20.0 * self.gain.max(1.0e-6).log10() + 24.0) / 60.0).clamp(0.0, 1.0);
        }
        if id == 1 {
            return self.mix;
        }
        if id == 2 {
            return self.mode as f32 / 2.0;
        }
        0.0
    }

    fn saturate(&self, x: f32) -> f32 {
        match self.mode {
            0 => {
                if x > 0.0 {
                    fast_tanh(x)
                } else {
                    x / (1.0 + x.abs())
                }
            }
            1 => fast_tanh(x),
            2 => ((1.5 * x) * (1.0 - (x * x) / 3.0)).clamp(-1.0, 1.0),
            _ => x,
        }
    }

    unsafe fn process(&self, left: *mut f32, right: *mut f32, frames: u32) {
        if left.is_null() || right.is_null() || frames == 0 {
            return;
        }
        let local_gain = if self.gain.is_finite() { self.gain } else { 1.0 };
        let local_mix = (if self.mix.is_finite() { self.mix } else { 0.0 }).clamp(0.0, 1.0);
        let mono = left == right;
        for index in 0..frames as usize {
            let dry_l = {
                let value = *left.add(index);
                if value.is_finite() { value } else { 0.0 }
            };
            let dry_r = {
                let value = *right.add(index);
                if value.is_finite() { value } else { 0.0 }
            };
            let wet_l = self.saturate(dry_l * local_gain);
            let wet_r = self.saturate(dry_r * local_gain);
            let out_l = dry_l + local_mix * (wet_l - dry_l);
            let out_r = dry_r + local_mix * (wet_r - dry_r);
            if mono {
                let mixed = 0.5 * (out_l + out_r);
                *left.add(index) = if mixed.is_finite() {
                    mixed.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
            } else {
                *left.add(index) = if out_l.is_finite() {
                    out_l.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
                *right.add(index) = if out_r.is_finite() {
                    out_r.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
            }
        }
    }
}

fn fast_tanh(x: f32) -> f32 {
    if x > 3.0 {
        return 1.0;
    }
    if x < -3.0 {
        return -1.0;
    }
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

