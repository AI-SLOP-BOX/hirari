//! Frozen preview-synth renderer.

#[repr(C)]
struct MidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

const _: () = assert!(std::mem::size_of::<MidiEvent>() == 272);

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    pitch: u8,
    channel: u8,
    level: f32,
    phase: f64,
    release_at: u64,
    started_at: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            active: false,
            pitch: 60,
            channel: 0,
            level: 0.0,
            phase: 0.0,
            release_at: u64::MAX,
            started_at: 0,
        }
    }
}

struct PreviewSynthReference {
    voices: [Voice; 16],
    filter_state: f32,
    engine: u32,
}

#[no_mangle]
pub extern "C" fn hirari_preview_synth_reference_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(PreviewSynthReference {
        voices: [Voice::default(); 16],
        filter_state: 0.0,
        engine: 0,
    })) as *mut std::ffi::c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_reference_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut PreviewSynthReference));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_reference_set_engine(
    state: *mut std::ffi::c_void,
    engine: u32,
) {
    if !state.is_null() {
        (*(state as *mut PreviewSynthReference)).engine = engine.min(2);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_reference_reset(state: *mut std::ffi::c_void) {
    if state.is_null() {
        return;
    }
    let synth = &mut *(state as *mut PreviewSynthReference);
    synth.voices = [Voice::default(); 16];
    synth.filter_state = 0.0;
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_reference_process(
    state: *mut std::ffi::c_void,
    events: *const MidiEvent,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    playhead: u64,
    sample_rate: f64,
    morph: f32,
    detune_ratio: f64,
    drive: f32,
    cutoff: f32,
    resonance: f32,
    output_gain: f32,
) {
    if state.is_null() || left.is_null() || right.is_null() {
        return;
    }
    let synth = &mut *(state as *mut PreviewSynthReference);
    let frames = frames as usize;
    let left = std::slice::from_raw_parts_mut(left, frames);
    let right = std::slice::from_raw_parts_mut(right, frames);
    let events = if events.is_null() || event_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(events, event_count)
    };

    let mut event_index = 0usize;
    for frame in 0..frames {
        let absolute_sample = playhead + frame as u64;
        while event_index < events.len() && events[event_index].sample_offset <= frame as u64 {
            let event = &events[event_index];
            event_index += 1;
            if event.sample_offset != frame as u64 || event.size < 3 {
                continue;
            }
            let status = event.data[0] & 0xf0;
            let channel = event.data[0] & 0x0f;
            let pitch = event.data[1] & 0x7f;
            if status == 0x90 && event.data[2] > 0 {
                let slot = synth
                    .voices
                    .iter()
                    .position(|voice| !voice.active)
                    .unwrap_or(0);
                let voice = &mut synth.voices[slot];
                voice.active = true;
                voice.pitch = pitch;
                voice.channel = channel;
                voice.level = event.data[2] as f32 / 127.0 * 0.65;
                voice.phase = 0.0;
                voice.release_at = u64::MAX;
                voice.started_at = absolute_sample;
            } else if status == 0x80 || (status == 0x90 && event.data[2] == 0) {
                let mut oldest: Option<usize> = None;
                for (index, voice) in synth.voices.iter().enumerate() {
                    if !voice.active
                        || voice.pitch != pitch
                        || voice.channel != channel
                        || voice.release_at != u64::MAX
                    {
                        continue;
                    }
                    if oldest.map_or(true, |best| {
                        voice.started_at < synth.voices[best].started_at
                    }) {
                        oldest = Some(index);
                    }
                }
                if let Some(index) = oldest {
                    synth.voices[index].release_at = absolute_sample;
                }
            }
        }

        let mut sample = 0.0f32;
        for voice in synth.voices.iter_mut() {
            if !voice.active {
                continue;
            }
            if absolute_sample >= voice.release_at {
                voice.level *= 0.995;
                if voice.level < 0.0005 {
                    voice.active = false;
                    continue;
                }
            }
            let frequency = 440.0
                * 2.0f64.powf((voice.pitch as i32 - 69) as f64 / 12.0)
                * detune_ratio;
            let sine = voice.phase.sin() as f32;
            let normalized_phase =
                ((voice.phase + 2.0 * std::f64::consts::PI) % (2.0 * std::f64::consts::PI)
                    / (2.0 * std::f64::consts::PI)) as f32;
            let saw = normalized_phase * 2.0 - 1.0;
            let mut oscillator = sine;
            if synth.engine == 1 {
                oscillator = sine * (1.0 - morph) + saw * morph;
            } else if synth.engine == 2 {
                let triangle = 1.0 - 4.0 * (normalized_phase - 0.5).abs();
                oscillator = triangle * (1.0 - morph) + saw * morph;
            }
            sample += oscillator * voice.level;
            voice.phase += 2.0 * std::f64::consts::PI * frequency / sample_rate;
            if voice.phase >= 2.0 * std::f64::consts::PI {
                voice.phase %= 2.0 * std::f64::consts::PI;
            }
        }
        let driven = (sample * drive).tanh();
        synth.filter_state += cutoff * (driven - synth.filter_state);
        sample = (synth.filter_state + resonance * (driven - synth.filter_state)) * output_gain;
        left[frame] += sample;
        right[frame] += sample;
    }
}
