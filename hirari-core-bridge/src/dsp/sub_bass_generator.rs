use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct SubBassGeneratorEngine {
    sample_rate: f64,
    env: f32,
    env_attack: f32,
    env_release: f32,
    lpf_state: f32,
    last_lpf: f32,
    dc_block_state: f32,
    lpf_coeff: f32,
    mix: AtomicU32,
    target_freq: f32,
    curr_freq: f32,
    phase: f64,
    zc_count: u32,
    is_positive: bool,
}

impl SubBassGeneratorEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            env: 0.0,
            env_attack: 0.995,
            env_release: 0.9998,
            lpf_state: 0.0,
            last_lpf: 0.0,
            dc_block_state: 0.0,
            lpf_coeff: 0.9775,
            mix: AtomicU32::new(0.5_f32.to_bits()),
            target_freq: 50.0,
            curr_freq: 50.0,
            phase: 0.0,
            zc_count: 0,
            is_positive: false,
        };
        engine.prepare_to_play(sample_rate);
        engine
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite() {
            sample_rate.clamp(1_000.0, 384_000.0)
        } else {
            44_100.0
        };
        self.env_attack = (-1.0 / (0.005 * self.sample_rate)).exp() as f32;
        self.env_release = (-1.0 / (0.1 * self.sample_rate)).exp() as f32;
        self.lpf_coeff = (-1.0 / (0.001 * self.sample_rate)).exp() as f32;
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.phase = 0.0;
        self.lpf_state = 0.0;
        self.last_lpf = 0.0;
        self.dc_block_state = 0.0;
        self.zc_count = 0;
        self.target_freq = 50.0;
        self.curr_freq = 50.0;
    }

    pub fn set_mix(&self, mix: f32) {
        let mix = if mix.is_finite() { mix } else { 0.0 };
        self.mix
            .store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn mix(&self) -> f32 {
        f32::from_bits(self.mix.load(Ordering::Relaxed))
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if id == 0 {
            self.set_mix(value);
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        if id == 0 {
            self.mix()
        } else {
            0.0
        }
    }

    pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        let mix = self.mix();
        let sr = self.sample_rate as f32;

        for frame in 0..frames {
            let in_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let in_r = right.as_ref().map_or(in_l, |channel| {
                if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    0.0
                }
            });
            let mid = 0.5 * (in_l + in_r);

            self.lpf_state += (1.0 - self.lpf_coeff) * (mid - self.lpf_state);
            let dc = self.lpf_state - self.last_lpf + 0.995 * self.dc_block_state;
            self.last_lpf = self.lpf_state;
            self.dc_block_state = if dc.is_finite() { dc } else { 0.0 };

            let magnitude = mid.abs();
            if magnitude > self.env {
                self.env = self.env_attack * self.env + (1.0 - self.env_attack) * magnitude;
            } else {
                self.env *= self.env_release;
            }
            self.env = if self.env.is_finite() { self.env } else { 0.0 }.clamp(0.0, 4.0);

            let mut crossing = false;
            if self.dc_block_state > 0.02 && !self.is_positive {
                self.is_positive = true;
                crossing = true;
            } else if self.dc_block_state < -0.02 {
                self.is_positive = false;
            }
            if crossing {
                let period = self.zc_count as f32;
                if period > 10.0 {
                    self.target_freq = ((sr / period) * 0.5).clamp(20.0, 90.0_f32.min(sr * 0.24));
                }
                self.zc_count = 0;
            }
            self.zc_count = (self.zc_count + 1).min(10_000);

            let max_freq = 90.0_f32.min(sr * 0.24);
            self.target_freq = (if self.target_freq.is_finite() {
                self.target_freq
            } else {
                50.0
            })
            .clamp(20.0, max_freq);
            self.curr_freq += (self.target_freq - self.curr_freq) * 0.05;
            self.curr_freq = (if self.curr_freq.is_finite() {
                self.curr_freq
            } else {
                50.0
            })
            .clamp(20.0, max_freq);
            self.phase += self.curr_freq as f64 / self.sample_rate;
            self.phase -= self.phase.floor();

            let sub = (2.0 * std::f64::consts::PI * self.phase).sin() as f32 * self.env * mix;
            left[frame] = if (in_l + sub).is_finite() {
                in_l + sub
            } else {
                in_l
            };
            if let Some(channel) = right.as_deref_mut() {
                channel[frame] = if (in_r + sub).is_finite() {
                    in_r + sub
                } else {
                    in_r
                };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_sub_bass_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(SubBassGeneratorEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SubBassGeneratorEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<SubBassGeneratorEngine>().as_mut() {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<SubBassGeneratorEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_set_parameter(state: *const c_void, id: u32, value: f32) {
    if let Some(state) = state.cast::<SubBassGeneratorEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_get_parameter(state: *const c_void, id: u32) -> f32 {
    state
        .cast::<SubBassGeneratorEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sub_bass_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() {
        return;
    }
    let Some(state) = state.cast::<SubBassGeneratorEngine>().as_mut() else {
        return;
    };
    let left = std::slice::from_raw_parts_mut(left, frames);
    if right.is_null() {
        state.process(left, None);
    } else {
        state.process(left, Some(std::slice::from_raw_parts_mut(right, frames)));
    }
}
