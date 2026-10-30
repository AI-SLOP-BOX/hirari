use std::ffi::c_void;

#[allow(dead_code)]
struct TransientShaper {
    sample_rate: f64,
    attack: f32,
    sustain: f32,
    attack_env: f32,
    sustain_env: f32,
    attack_alpha: f32,
    sustain_alpha: f32,
    current_gain: f32,
}

impl TransientShaper {
    fn new() -> Self {
        Self {
            sample_rate: 44_100.0,
            attack: 0.0,
            sustain: 0.0,
            attack_env: 0.0,
            sustain_env: 0.0,
            attack_alpha: 0.9,
            sustain_alpha: 0.99,
            current_gain: 1.0,
        }
    }

    fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.attack_alpha = (-1.0f32 / (sample_rate as f32 * 0.005)).exp();
        self.sustain_alpha = (-1.0f32 / (sample_rate as f32 * 0.050)).exp();
    }

    fn reset(&mut self) {
        self.attack_env = 0.0;
        self.sustain_env = 0.0;
        self.current_gain = 1.0;
    }

    unsafe fn process(&mut self, left: *mut f32, right: *mut f32, frames: u32) {
        if left.is_null() || frames == 0 {
            return;
        }
        let attack_gain = (1.0 + self.attack).clamp(0.0, 2.0);
        let sustain_gain = (1.0 + self.sustain).clamp(0.0, 2.0);
        for frame in 0..frames as usize {
            let in_l = *left.add(frame);
            let in_r = if right.is_null() {
                in_l
            } else {
                *right.add(frame)
            };
            let level = in_l.abs().max(in_r.abs());
            self.attack_env = self.attack_alpha * self.attack_env + (1.0 - self.attack_alpha) * level;
            self.sustain_env =
                self.sustain_alpha * self.sustain_env + (1.0 - self.sustain_alpha) * level;
            let transient = (self.attack_env - self.sustain_env).clamp(-1.0, 1.0);
            let body = self.sustain_env.clamp(0.0, 1.0);
            let gain = (1.0 + transient * (attack_gain - 1.0) + body * (sustain_gain - 1.0))
                .clamp(0.0, 3.0);
            *left.add(frame) = in_l * gain;
            if !right.is_null() {
                *right.add(frame) = in_r * gain;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_transient_shaper_reference_create(_sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(TransientShaper::new())) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reference_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut TransientShaper));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reference_prepare(
    state: *mut c_void,
    sample_rate: f64,
) {
    if !state.is_null() {
        (*(state as *mut TransientShaper)).prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reference_reset(state: *mut c_void) {
    if !state.is_null() {
        (*(state as *mut TransientShaper)).reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reference_set_parameters(
    state: *mut c_void,
    attack: f32,
    sustain: f32,
) {
    if state.is_null() {
        return;
    }
    let reference = &mut *(state as *mut TransientShaper);
    reference.attack = attack;
    reference.sustain = sustain;
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reference_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if !state.is_null() {
        (*(state as *mut TransientShaper)).process(left, right, frames);
    }
}
