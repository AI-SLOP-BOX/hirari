use crate::lfo::{LfoEngine, Waveform};
use crate::wavetable_oscillator::WavetableOscillatorEngine;

pub struct Voice {
    pub active: bool,
    pub releasing: bool,
    pub note: u8,
    pub velocity: f32,
    pub env: f32,
    pub osc: WavetableOscillatorEngine,
    pub lfo: LfoEngine,
}

pub struct WavetableSynthEngine {
    pub sample_rate: f64,
    pub voices: Vec<Voice>,
}

impl WavetableSynthEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1.0 { sample_rate } else { 48_000.0 };
        let mut voices = Vec::with_capacity(16);
        for _ in 0..16 {
            let mut lfo = LfoEngine::new(sample_rate as f32);
            lfo.set_frequency(5.0); // 5Hz Default
            let mut osc = WavetableOscillatorEngine::new(44_100.0);
            osc.set_sample_rate(sample_rate);
            voices.push(Voice {
                active: false,
                releasing: false,
                note: 0,
                velocity: 0.0,
                env: 0.0,
                osc,
                lfo,
            });
        }
        Self {
            sample_rate,
            voices,
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        for v in &mut self.voices {
            if !v.active {
                v.active = true;
                v.releasing = false;
                v.note = note;
                v.velocity = velocity as f32 / 127.0;
                v.env = 0.0;
                v.osc
                    .set_frequency(440.0 * 2.0f64.powf((note as f64 - 69.0) / 12.0));
                return;
            }
        }
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.active && v.note == note {
                v.releasing = true;
            }
        }
    }

    /// INDUSTRIAL: Processes an audio block with LFO-modulated wavetable synthesis.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let events = [];
        self.process_with_midi(l, r, &events);
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 1000.0 { return; }
        self.sample_rate = sample_rate;
        for voice in &mut self.voices {
            voice.osc.set_sample_rate(sample_rate);
            voice.lfo.set_sample_rate(sample_rate as f32);
        }
    }

    pub fn process_with_midi(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        events: &[crate::hirari_wavetable_synth::HirariWavetableMidiEvent],
    ) {
        let len = l.len().min(r.len());
        unsafe { self.process_raw(l.as_mut_ptr(), r.as_mut_ptr(), len, events); }
    }

    pub unsafe fn process_raw(
        &mut self,
        left: *mut f32,
        right: *mut f32,
        len: usize,
        events: &[crate::hirari_wavetable_synth::HirariWavetableMidiEvent],
    ) {
        let mut event_index = 0;
        for sample in 0..len {
            while event_index < events.len() && events[event_index].sample_offset <= sample as u64 {
                let event = &events[event_index];
                event_index += 1;
                if event.size < 3 { continue; }
                match event.data[0] & 0xf0 {
                    0x90 if event.data[2] != 0 => self.note_on(event.data[1], event.data[2]),
                    0x80 | 0x90 => self.note_off(event.data[1]),
                    _ => {}
                }
            }
            let mut output = 0.0f32;
            for voice in &mut self.voices {
                if !voice.active { continue; }
                let attack = (1.0 / (self.sample_rate * 0.005).max(1.0)) as f32;
                let release = (1.0 / (self.sample_rate * 0.08).max(1.0)) as f32;
                voice.env = if voice.releasing { voice.env - release } else { voice.env + attack };
                if voice.releasing && voice.env <= 0.0 {
                    voice.env = 0.0;
                    voice.active = false;
                    continue;
                }
                voice.env = voice.env.clamp(0.0, 1.0);
                let lfo = voice.lfo.process(Waveform::Sine) * 0.0025;
                output += voice.osc.process(0.5 + lfo) * voice.velocity * voice.env;
            }
            output = (output * 0.25).clamp(-1.0, 1.0);
            if left == right {
                *left.add(sample) += output;
            } else {
                *left.add(sample) += output;
                *right.add(sample) += output;
            }
        }
    }

    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
            voice.releasing = false;
            voice.env = 0.0;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Wavetable Synth state.
    pub fn audit_wavetable_synth(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate > 1.0
            && self.voices.iter().all(|voice| voice.velocity.is_finite()
                && (0.0..=1.0).contains(&voice.velocity)
                && voice.env.is_finite() && (0.0..=1.0).contains(&voice.env))
    }
}

#[no_mangle]
pub extern "C" fn hirari_ws_create(sample_rate: f64) -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(WavetableSynthEngine::new(sample_rate))).cast()
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<WavetableSynthEngine>())); }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_prepare(state: *mut std::ffi::c_void, sample_rate: f64) {
    if let Some(engine) = state.cast::<WavetableSynthEngine>().as_mut() { engine.prepare(sample_rate); }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_reset(state: *mut std::ffi::c_void) {
    if let Some(engine) = state.cast::<WavetableSynthEngine>().as_mut() { engine.reset(); }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_note_on(state: *mut std::ffi::c_void, note: u8, velocity: u8) {
    if let Some(engine) = state.cast::<WavetableSynthEngine>().as_mut() { engine.note_on(note, velocity); }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_note_off(state: *mut std::ffi::c_void, note: u8) {
    if let Some(engine) = state.cast::<WavetableSynthEngine>().as_mut() { engine.note_off(note); }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_ws_process(
    state: *mut std::ffi::c_void,
    events: *const crate::hirari_wavetable_synth::HirariWavetableMidiEvent,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    let Some(engine) = state.cast::<WavetableSynthEngine>().as_mut() else { return; };
    if left.is_null() || right.is_null() || event_count > 1024
        || (event_count > 0 && events.is_null()) { return; }
    let event_slice = if event_count == 0 { &[] } else {
        std::slice::from_raw_parts(events, event_count)
    };
    engine.process_raw(left, right, frames as usize, event_slice);
}
