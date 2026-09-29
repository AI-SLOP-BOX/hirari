//! Realtime fallback synth for MIDI notes that have no target track.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const VOICE_COUNT: usize = 16;
const MAX_FRAMES: usize = 65_536;
const MAX_EVENTS: usize = 1024;
const TWO_PI: f64 = std::f64::consts::TAU;

#[repr(C)]
struct MidiEventView {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

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

struct PreviewSynth {
    voices: [Voice; VOICE_COUNT],
    filter_state: f32,
    engine: AtomicU32,
}

impl PreviewSynth {
    fn new() -> Self {
        Self {
            voices: [Voice::default(); VOICE_COUNT],
            filter_state: 0.0,
            engine: AtomicU32::new(0),
        }
    }

    fn reset(&mut self) {
        self.voices.fill(Voice::default());
        self.filter_state = 0.0;
    }

    unsafe fn process(
        &mut self,
        events: *const MidiEventView,
        event_count: usize,
        left: *mut f32,
        right: *mut f32,
        frames: usize,
        playhead: u64,
        sample_rate: f64,
        morph: f32,
        detune_ratio: f64,
        drive: f32,
        cutoff: f32,
        resonance: f32,
        output_gain: f32,
    ) {
        if left.is_null()
            || right.is_null()
            || frames == 0
            || frames > MAX_FRAMES
            || event_count > MAX_EVENTS
        {
            return;
        }
        let events = if event_count == 0 {
            &[][..]
        } else if events.is_null() {
            return;
        } else {
            unsafe { std::slice::from_raw_parts(events, event_count) }
        };
        let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
        let engine = self.engine.load(Ordering::Acquire);
        let mut event_index = 0;
        for frame in 0..frames {
            let absolute_sample = playhead.saturating_add(frame as u64);
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
                    let index = self
                        .voices
                        .iter()
                        .position(|voice| !voice.active)
                        .unwrap_or(0);
                    let voice = &mut self.voices[index];
                    voice.active = true;
                    voice.pitch = pitch;
                    voice.channel = channel;
                    voice.level = event.data[2] as f32 / 127.0 * 0.65;
                    voice.phase = 0.0;
                    voice.release_at = u64::MAX;
                    voice.started_at = absolute_sample;
                } else if status == 0x80 || (status == 0x90 && event.data[2] == 0) {
                    let oldest = self
                        .voices
                        .iter()
                        .enumerate()
                        .filter(|(_, voice)| {
                            voice.active
                                && voice.pitch == pitch
                                && voice.channel == channel
                                && voice.release_at == u64::MAX
                        })
                        .min_by_key(|(_, voice)| voice.started_at)
                        .map(|(index, _)| index);
                    if let Some(index) = oldest {
                        self.voices[index].release_at = absolute_sample;
                    }
                }
            }
            let mut sample = 0.0_f32;
            for voice in &mut self.voices {
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
                let frequency =
                    440.0 * 2.0_f64.powf((voice.pitch as i32 - 69) as f64 / 12.0) * detune_ratio;
                let sine = voice.phase.sin() as f32;
                let normalized_phase = ((voice.phase + TWO_PI) % TWO_PI) as f32 / TWO_PI as f32;
                let saw = normalized_phase * 2.0 - 1.0;
                let oscillator = if engine == 1 {
                    sine * (1.0 - morph) + saw * morph
                } else if engine == 2 {
                    let triangle = 1.0 - 4.0 * (normalized_phase - 0.5).abs();
                    triangle * (1.0 - morph) + saw * morph
                } else {
                    sine
                };
                sample += oscillator * voice.level;
                voice.phase += TWO_PI * frequency / sample_rate;
                if voice.phase >= TWO_PI {
                    voice.phase %= TWO_PI;
                }
            }
            let driven = (sample * drive).tanh();
            self.filter_state += cutoff * (driven - self.filter_state);
            let output =
                (self.filter_state + resonance * (driven - self.filter_state)) * output_gain;
            left[frame] += output;
            right[frame] += output;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_preview_synth_create() -> *mut c_void {
    Box::into_raw(Box::new(PreviewSynth::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<PreviewSynth>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_set_engine(state: *mut c_void, engine: u32) {
    let Some(state) = (unsafe { state.cast::<PreviewSynth>().as_ref() }) else {
        return;
    };
    state.engine.store(engine.min(2), Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_reset(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<PreviewSynth>().as_mut() }) else {
        return;
    };
    state.reset();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_synth_process(
    state: *mut c_void,
    events: *const c_void,
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
    let Some(state) = (unsafe { state.cast::<PreviewSynth>().as_mut() }) else {
        return;
    };
    unsafe {
        state.process(
            events.cast(),
            event_count,
            left,
            right,
            frames as usize,
            playhead,
            sample_rate,
            morph,
            detune_ratio,
            drive,
            cutoff,
            resonance,
            output_gain,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_preview_synth_reference_create() -> *mut c_void;
        fn hirari_preview_synth_reference_destroy(state: *mut c_void);
        fn hirari_preview_synth_reference_set_engine(state: *mut c_void, engine: u32);
        fn hirari_preview_synth_reference_reset(state: *mut c_void);
        fn hirari_preview_synth_reference_process(
            state: *mut c_void,
            events: *const MidiEventView,
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
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn fallback_voice_matches_cpp_across_waveforms_blocks_and_note_overlap() {
        let events = [
            midi(0, [0x90, 60, 100]),
            midi(13, [0x91, 67, 72]),
            midi(40, [0x90, 60, 80]),
            midi(75, [0x80, 60, 0]),
            midi(120, [0x81, 67, 0]),
            midi(180, [0x90, 72, 110]),
            midi(300, [0x80, 72, 0]),
        ];
        for engine in 0..=2 {
            let mut rust = PreviewSynth::new();
            rust.engine.store(engine, Ordering::Release);
            let cpp = unsafe { hirari_preview_synth_reference_create() };
            assert!(!cpp.is_null());
            unsafe {
                hirari_preview_synth_reference_set_engine(cpp, engine);
            }
            let mut rust_left = [0.0_f32; 384];
            let mut rust_right = [0.0_f32; 384];
            let mut cpp_left = [0.0_f32; 384];
            let mut cpp_right = [0.0_f32; 384];
            for (start, end) in [(0usize, 64usize), (64, 160), (160, 256), (256, 384)] {
                let block_events: Vec<_> = events
                    .iter()
                    .filter_map(|event| {
                        (event.sample_offset >= start as u64 && event.sample_offset < end as u64)
                            .then(|| MidiEventView {
                                sample_offset: event.sample_offset - start as u64,
                                size: 3,
                                data: event.data,
                                articulation_id: 0,
                            })
                    })
                    .collect();
                unsafe {
                    rust.process(
                        block_events.as_ptr(),
                        block_events.len(),
                        rust_left[start..].as_mut_ptr(),
                        rust_right[start..].as_mut_ptr(),
                        end - start,
                        start as u64,
                        48_000.0,
                        0.37,
                        1.0004,
                        1.35,
                        0.12,
                        0.08,
                        0.91,
                    );
                    hirari_preview_synth_reference_process(
                        cpp,
                        block_events.as_ptr(),
                        block_events.len(),
                        cpp_left[start..].as_mut_ptr(),
                        cpp_right[start..].as_mut_ptr(),
                        (end - start) as u32,
                        start as u64,
                        48_000.0,
                        0.37,
                        1.0004,
                        1.35,
                        0.12,
                        0.08,
                        0.91,
                    );
                }
            }
            unsafe {
                hirari_preview_synth_reference_destroy(cpp);
            }
            for (actual, expected) in rust_left.iter().zip(cpp_left) {
                assert!(
                    (actual - expected).abs() <= 1.0e-6,
                    "engine={engine}: {actual} != {expected}"
                );
            }
            assert_eq!(rust_left, rust_right);
            assert_eq!(cpp_left, cpp_right);
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn reset_clears_active_preview_voices_and_filter_history() {
        let mut rust = PreviewSynth::new();
        let cpp = unsafe { hirari_preview_synth_reference_create() };
        let event = midi(0, [0x90, 60, 127]);
        let mut rust_out = [0.0_f32; 64];
        let mut cpp_out = [0.0_f32; 64];
        unsafe {
            rust.process(
                &event,
                1,
                rust_out.as_mut_ptr(),
                rust_out.as_mut_ptr(),
                64,
                0,
                48_000.0,
                0.5,
                1.0,
                1.0,
                0.1,
                0.0,
                1.0,
            );
            hirari_preview_synth_reference_process(
                cpp,
                &event,
                1,
                cpp_out.as_mut_ptr(),
                cpp_out.as_mut_ptr(),
                64,
                0,
                48_000.0,
                0.5,
                1.0,
                1.0,
                0.1,
                0.0,
                1.0,
            );
            hirari_preview_synth_reference_reset(cpp);
        }
        rust.reset();
        let mut rust_silence = [0.0_f32; 16];
        let mut cpp_silence = [0.0_f32; 16];
        unsafe {
            rust.process(
                std::ptr::null(),
                0,
                rust_silence.as_mut_ptr(),
                rust_silence.as_mut_ptr(),
                16,
                64,
                48_000.0,
                0.5,
                1.0,
                1.0,
                0.1,
                0.0,
                1.0,
            );
            hirari_preview_synth_reference_process(
                cpp,
                std::ptr::null(),
                0,
                cpp_silence.as_mut_ptr(),
                cpp_silence.as_mut_ptr(),
                16,
                64,
                48_000.0,
                0.5,
                1.0,
                1.0,
                0.1,
                0.0,
                1.0,
            );
            hirari_preview_synth_reference_destroy(cpp);
        }
        assert_eq!(rust_silence, cpp_silence);
    }

    #[cfg(feature = "dsp-differential-reference")]
    fn midi(sample_offset: u64, data: [u8; 3]) -> MidiEventView {
        let mut bytes = [0; 256];
        bytes[..3].copy_from_slice(&data);
        MidiEventView {
            sample_offset,
            size: 3,
            data: bytes,
            articulation_id: 0,
        }
    }
}
