//! Drum voice engine used by HirariUltimate and offline bounce.

use std::ffi::c_void;

const MIDI_EVENT_BYTES: usize = 256;

#[repr(C)]
pub struct MidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; MIDI_EVENT_BYTES],
    articulation_id: u8,
}

struct VirtuosoDrumSynth {
    sample_rate: f64,
    rand_state: u32,
    kick_env: f32,
    kick_click_env: f32,
    kick_phase: f32,
    drive: f32,
    decay: f32,
    snare_env: f32,
    snare_phase1: f32,
    snare_phase2: f32,
    snare_lowpass: f32,
    hat_env: f32,
    hat_z1: f32,
    hat_color: f32,
    active_hat_note: u8,
    hat_phases: [f32; 6],
}

impl VirtuosoDrumSynth {
    fn new(sample_rate: f64) -> Self {
        let mut synth = Self {
            sample_rate: valid_sample_rate(sample_rate),
            rand_state: 0xACE1,
            kick_env: 0.0,
            kick_click_env: 0.0,
            kick_phase: 0.0,
            drive: 0.1,
            decay: 0.5,
            snare_env: 0.0,
            snare_phase1: 0.0,
            snare_phase2: 0.0,
            snare_lowpass: 0.0,
            hat_env: 0.0,
            hat_z1: 0.0,
            hat_color: 0.5,
            active_hat_note: 0,
            hat_phases: [0.0; 6],
        };
        synth.reset();
        synth
    }

    fn reset(&mut self) {
        self.kick_env = 0.0;
        self.snare_env = 0.0;
        self.hat_env = 0.0;
        self.kick_phase = 0.0;
        self.snare_phase1 = 0.0;
        self.snare_phase2 = 0.0;
        self.hat_phases.fill(0.0);
    }

    fn trigger_drum(&mut self, note: u8, velocity: f32) {
        match note {
            36 => {
                self.kick_env = velocity;
                self.kick_click_env = velocity;
                self.kick_phase = 0.0;
            }
            38 | 40 => {
                self.snare_env = velocity;
                self.snare_phase1 = 0.0;
                self.snare_phase2 = 0.0;
            }
            42 | 44 | 46 => {
                self.hat_env = velocity * 0.5;
                self.active_hat_note = note;
            }
            _ => {}
        }
    }

    fn fast_rand(&mut self) -> u32 {
        self.rand_state ^= self.rand_state << 13;
        self.rand_state ^= self.rand_state >> 17;
        self.rand_state ^= self.rand_state << 5;
        self.rand_state
    }

    fn noise(&mut self) -> f32 {
        (self.fast_rand() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn process(&mut self, events: &[MidiEvent], left: &mut [f32], right: &mut [f32]) {
        for event in events {
            if event.size < 3 {
                continue;
            }
            let status = event.data[0] & 0xf0;
            if status == 0x90 && event.data[2] > 0 {
                self.trigger_drum(event.data[1], event.data[2] as f32 / 127.0);
            } else if status == 0xb0 {
                let value = event.data[2] as f32 / 127.0;
                match event.data[1] {
                    1 => self.drive = value,
                    74 => self.hat_color = value,
                    75 => self.decay = value,
                    _ => {}
                }
            }
        }

        let frames = left.len().min(right.len());
        let sample_rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44_100.0
        };
        const HAT_FREQS: [f32; 6] = [367.5, 459.0, 552.0, 625.5, 784.5, 988.5];

        for frame in 0..frames {
            let mut output = 0.0;
            if self.kick_env > 0.0001 {
                let sweep_freq = 35.0 + 160.0 * self.kick_env.powi(3);
                self.kick_phase +=
                    ((std::f32::consts::TAU * sweep_freq) as f64 / sample_rate) as f32;
                let fm_mod = (self.kick_phase * 0.5).sin() * (self.kick_env * 0.5);
                let body = (self.kick_phase + fm_mod).sin();
                output += (body * (1.0 + self.drive * 2.0)).tanh() * self.kick_env * 0.8;
                self.kick_click_env *= 0.995;
                output += self.noise() * 0.1 * self.kick_click_env;
                self.kick_env *= 0.999 - (1.0 - self.decay) * 0.005;
            }

            if self.snare_env > 0.0001 {
                self.snare_phase1 +=
                    ((std::f32::consts::TAU * 180.0f32) as f64 / sample_rate) as f32;
                self.snare_phase2 +=
                    ((std::f32::consts::TAU * 330.0f32) as f64 / sample_rate) as f32;
                let tone = self.snare_phase1.sin() * 0.6 + self.snare_phase2.sin() * 0.4;
                let noise = self.noise();
                self.snare_lowpass = 0.4 * noise + 0.6 * self.snare_lowpass;
                output += (tone * 0.3 + self.snare_lowpass * 0.7) * self.snare_env;
                self.snare_env *= 0.9992;
            }

            if self.hat_env > 0.0001 {
                let mut cluster = 0.0;
                for (phase, frequency) in self.hat_phases.iter_mut().zip(HAT_FREQS) {
                    *phase += ((std::f32::consts::TAU * frequency) as f64 / sample_rate) as f32;
                    cluster += if phase.sin() > 0.0 { 1.0 } else { -1.0 };
                }
                let hp_frequency = 0.85 + self.hat_color * 0.1;
                self.hat_z1 = cluster * 0.2 - self.hat_z1 * hp_frequency;
                output += self.hat_z1 * self.hat_env * 0.6;
                self.hat_env *= if self.active_hat_note == 42 {
                    0.9982
                } else {
                    0.9998
                };
            }

            left[frame] += output;
            right[frame] += output;
        }
    }
}

fn valid_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_drum_synth_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoDrumSynth::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_drum_synth_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<VirtuosoDrumSynth>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_drum_synth_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<VirtuosoDrumSynth>().as_mut() } {
        state.sample_rate = valid_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_drum_synth_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<VirtuosoDrumSynth>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_drum_synth_process(
    state: *mut c_void,
    events: *const MidiEvent,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<VirtuosoDrumSynth>().as_mut() }) else {
        return;
    };
    if left.is_null() || right.is_null() || (event_count > 0 && events.is_null()) {
        return;
    }
    let events = if event_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(events, event_count) }
    };
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
    state.process(events, left, right);
}
