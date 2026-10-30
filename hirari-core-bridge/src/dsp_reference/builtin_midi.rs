use std::ffi::c_void;

#[repr(C)]
struct MidiEventView {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    held: bool,
    pitch: u8,
    channel: u8,
    velocity: f32,
    envelope: f32,
    phase: f64,
    frequency: f64,
    started_order: u64,
}

struct Reference {
    voices: [Voice; 32],
    order: u64,
}

#[no_mangle]
pub extern "C" fn hirari_builtin_midi_reference_create() -> *mut c_void {
    Box::into_raw(Box::new(Reference {
        voices: [Voice::default(); 32],
        order: 0,
    })) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_midi_reference_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut Reference));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_midi_reference_process(
    state: *mut c_void,
    events: *const MidiEventView,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    sample_rate: f64,
) {
    let state = &mut *(state as *mut Reference);
    let sample_rate = if !sample_rate.is_finite() || sample_rate <= 1.0 {
        44100.0
    } else {
        sample_rate
    };
    let mut event_index = 0usize;
    for frame in 0..frames {
        while event_index < event_count && (*events.add(event_index)).sample_offset <= frame as u64 {
            let event = &*events.add(event_index);
            event_index += 1;
            if event.sample_offset != frame as u64 || event.size < 3 {
                continue;
            }
            let status = event.data[0] & 0xf0;
            let channel = event.data[0] & 0x0f;
            let pitch = event.data[1] & 0x7f;
            if status == 0x90 && event.data[2] > 0 {
                let mut voice_index = 0usize;
                for index in 0..state.voices.len() {
                    if !state.voices[index].active {
                        voice_index = index;
                        break;
                    }
                    if state.voices[index].started_order < state.voices[voice_index].started_order {
                        voice_index = index;
                    }
                }
                state.order += 1;
                let order = state.order;
                state.voices[voice_index] = Voice {
                    active: true,
                    held: true,
                    pitch,
                    channel,
                    velocity: event.data[2] as f32 / 127.0,
                    envelope: 0.0,
                    phase: 0.0,
                    frequency: 440.0 * 2.0f64.powf((pitch as i32 - 69) as f64 / 12.0),
                    started_order: order,
                };
            } else if status == 0x80 || (status == 0x90 && event.data[2] == 0) {
                let mut oldest: Option<usize> = None;
                for index in 0..state.voices.len() {
                    let voice = &state.voices[index];
                    if !voice.active || !voice.held || voice.pitch != pitch || voice.channel != channel {
                        continue;
                    }
                    if oldest
                        .map(|current| voice.started_order < state.voices[current].started_order)
                        .unwrap_or(true)
                    {
                        oldest = Some(index);
                    }
                }
                if let Some(index) = oldest {
                    state.voices[index].held = false;
                }
            } else if status == 0xb0 && event.data[1] == 123 {
                for voice in state.voices.iter_mut() {
                    if voice.active && voice.channel == channel {
                        voice.held = false;
                    }
                }
            }
        }
        let mut mixed = 0.0f32;
        for voice in state.voices.iter_mut() {
            if !voice.active {
                continue;
            }
            let attack = (1.0 / (sample_rate * 0.005)) as f32;
            let release = (1.0 / (sample_rate * 0.12)) as f32;
            voice.envelope = if voice.held {
                (voice.envelope + attack).min(1.0)
            } else {
                (voice.envelope - release).max(0.0)
            };
            if !voice.held && voice.envelope <= 0.0 {
                voice.active = false;
                continue;
            }
            mixed += voice.phase.sin() as f32 * voice.velocity * voice.envelope * 0.2;
            voice.phase += 2.0 * std::f64::consts::PI * voice.frequency / sample_rate;
            if voice.phase >= 2.0 * std::f64::consts::PI {
                voice.phase %= 2.0 * std::f64::consts::PI;
            }
        }
        *left.add(frame as usize) += mixed;
        *right.add(frame as usize) += mixed;
    }
}
