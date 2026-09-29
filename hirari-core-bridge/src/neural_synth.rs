use std::ffi::c_void;

const VOICES: usize = 32;
const OSCILLATORS: usize = 4;
const TABLE_SIZE: usize = 2048;

struct Voice {
    active: bool,
    note: u8,
    velocity: f32,
    phase: [f64; OSCILLATORS],
    envelope: f32,
    envelope_state: u8,
}

impl Voice {
    fn idle() -> Self {
        Self {
            active: false,
            note: 0,
            velocity: 0.0,
            phase: [0.0; OSCILLATORS],
            envelope: 0.0,
            envelope_state: 0,
        }
    }
}

struct NeuralSynth {
    sine: [f32; TABLE_SIZE],
    grit: [f32; TABLE_SIZE],
    voices: [Voice; VOICES],
}

impl NeuralSynth {
    fn new() -> Self {
        let mut sine = [0.0; TABLE_SIZE];
        let mut grit = [0.0; TABLE_SIZE];
        for index in 0..TABLE_SIZE {
            let phase = index as f32 / TABLE_SIZE as f32;
            sine[index] = (phase * 2.0 * std::f32::consts::PI).sin();
            grit[index] = if phase < 0.5 { 1.0 } else { -1.0 };
        }
        Self {
            sine,
            grit,
            voices: std::array::from_fn(|_| Voice::idle()),
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            return;
        }
        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
            voice.active = true;
            voice.note = note;
            voice.velocity = velocity as f32 / 127.0;
            voice.phase = [0.0; OSCILLATORS];
            voice.envelope = 0.0;
            voice.envelope_state = 1;
        }
    }

    fn note_off(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.active && voice.note == note {
                voice.envelope_state = 4;
            }
        }
    }

    fn process(
        &mut self,
        left: &mut [f32],
        mut right: Option<&mut [f32]>,
        sample_rate: f64,
        morph: f32,
    ) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        let right_output = right
            .as_deref_mut()
            .map_or(std::ptr::null_mut(), |channel| channel.as_mut_ptr());
        let sample_rate =
            if sample_rate.is_finite() && sample_rate >= 8_000.0 && sample_rate <= 384_000.0 {
                sample_rate
            } else {
                44_100.0
            };
        let morph = if morph.is_finite() {
            morph.clamp(0.0, 1.0)
        } else {
            0.0
        };
        for frame in 0..frames {
            let mut output = 0.0f32;
            for voice in &mut self.voices {
                if !voice.active {
                    continue;
                }
                let frequency = 440.0f64 * 2.0f64.powf((voice.note as f64 - 69.0) / 12.0);
                match voice.envelope_state {
                    1 => {
                        voice.envelope += 0.001;
                        if voice.envelope >= 1.0 {
                            voice.envelope = 1.0;
                            voice.envelope_state = 3;
                        }
                    }
                    4 => {
                        voice.envelope -= 0.0005;
                        if voice.envelope <= 0.0 {
                            voice.envelope = 0.0;
                            voice.active = false;
                        }
                    }
                    _ => {}
                }
                let mut signal = 0.0;
                for phase in &mut voice.phase {
                    *phase += frequency / sample_rate;
                    if *phase >= 1.0 {
                        *phase -= 1.0;
                    }
                    let index = ((*phase * TABLE_SIZE as f64) as usize) % TABLE_SIZE;
                    signal += self.sine[index] * (1.0 - morph) + self.grit[index] * morph;
                }
                output += signal * voice.envelope * voice.velocity;
            }
            let contribution = output * 0.25;
            if contribution.is_finite() {
                left[frame] += contribution;
            }
            if !right_output.is_null() {
                if contribution.is_finite() {
                    unsafe {
                        *right_output.add(frame) += contribution;
                    }
                }
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_neural_synth_create() -> *mut c_void {
    Box::into_raw(Box::new(NeuralSynth::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_neural_synth_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<NeuralSynth>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_neural_synth_note_on(state: *mut c_void, note: u8, velocity: u8) {
    if !state.is_null() {
        (*state.cast::<NeuralSynth>()).note_on(note, velocity);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_neural_synth_note_off(state: *mut c_void, note: u8) {
    if !state.is_null() {
        (*state.cast::<NeuralSynth>()).note_off(note);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_neural_synth_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    sample_rate: f64,
    morph: f32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let synth = &mut *state.cast::<NeuralSynth>();
    let left = std::slice::from_raw_parts_mut(left, frames as usize);
    let right = if right.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts_mut(right, frames as usize))
    };
    synth.process(left, right, sample_rate, morph);
}
