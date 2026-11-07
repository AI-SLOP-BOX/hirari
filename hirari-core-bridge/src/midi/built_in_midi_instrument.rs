//! The allocation-free sine fallback used by MIDI and instrument tracks when
//! the track has no native instrument plugin.

use std::ffi::c_void;
use std::slice;

const VOICE_COUNT: usize = 32;

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

struct Instrument {
    voices: [Voice; VOICE_COUNT],
    voice_order: u64,
}

impl Instrument {
    fn new() -> Self {
        Self {
            voices: [Voice::default(); VOICE_COUNT],
            voice_order: 0,
        }
    }

    unsafe fn process(
        &mut self,
        events: *const MidiEventView,
        event_count: usize,
        left: *mut f32,
        right: *mut f32,
        frames: usize,
        sample_rate: f64,
    ) {
        if left.is_null() || right.is_null() || frames == 0 {
            return;
        }
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1.0 {
            sample_rate
        } else {
            44_100.0
        };
        let events = if event_count == 0 {
            &[][..]
        } else if events.is_null() {
            return;
        } else {
            unsafe { slice::from_raw_parts(events, event_count) }
        };
        let left = unsafe { slice::from_raw_parts_mut(left, frames) };
        let right = unsafe { slice::from_raw_parts_mut(right, frames) };
        let mut event_index = 0;
        for frame in 0..frames {
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
                    let mut voice_index = None;
                    for (index, candidate) in self.voices.iter().enumerate() {
                        if !candidate.active {
                            voice_index = Some(index);
                            break;
                        }
                        if voice_index.is_none_or(|oldest| {
                            candidate.started_order < self.voices[oldest].started_order
                        }) {
                            voice_index = Some(index);
                        }
                    }
                    if let Some(index) = voice_index {
                        self.voice_order = self.voice_order.wrapping_add(1);
                        self.voices[index] = Voice {
                            active: true,
                            held: true,
                            pitch,
                            channel,
                            velocity: event.data[2] as f32 / 127.0,
                            phase: 0.0,
                            frequency: 440.0 * 2.0f64.powf((pitch as i32 - 69) as f64 / 12.0),
                            started_order: self.voice_order,
                            ..Voice::default()
                        };
                    }
                } else if status == 0x80 || (status == 0x90 && event.data[2] == 0) {
                    let mut oldest: Option<usize> = None;
                    for (index, voice) in self.voices.iter().enumerate() {
                        if !voice.active
                            || !voice.held
                            || voice.pitch != pitch
                            || voice.channel != channel
                        {
                            continue;
                        }
                        if oldest.is_none_or(|selected| {
                            voice.started_order < self.voices[selected].started_order
                        }) {
                            oldest = Some(index);
                        }
                    }
                    if let Some(index) = oldest {
                        self.voices[index].held = false;
                    }
                } else if status == 0xb0 && event.data[1] == 123 {
                    for voice in &mut self.voices {
                        if voice.active && voice.channel == channel {
                            voice.held = false;
                        }
                    }
                }
            }

            let mut mixed = 0.0f32;
            for voice in &mut self.voices {
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
                mixed += (voice.phase.sin() as f32) * voice.velocity * voice.envelope * 0.2;
                voice.phase += std::f64::consts::TAU * voice.frequency / sample_rate;
                if voice.phase >= std::f64::consts::TAU {
                    voice.phase %= std::f64::consts::TAU;
                }
            }
            left[frame] += mixed;
            right[frame] += mixed;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_builtin_midi_instrument_create() -> *mut c_void {
    Box::into_raw(Box::new(Instrument::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_midi_instrument_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<Instrument>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_midi_instrument_process(
    state: *mut c_void,
    events: *const c_void,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    sample_rate: f64,
) {
    if state.is_null() {
        return;
    }
    unsafe {
        (&mut *state.cast::<Instrument>()).process(
            events.cast(),
            event_count,
            left,
            right,
            frames as usize,
            sample_rate,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_builtin_midi_reference_process(
            state: *mut c_void,
            events: *const MidiEventView,
            event_count: usize,
            left: *mut f32,
            right: *mut f32,
            frames: u32,
            sample_rate: f64,
        );
        fn hirari_builtin_midi_reference_create() -> *mut c_void;
        fn hirari_builtin_midi_reference_destroy(state: *mut c_void);
    }

    #[test]
    fn midi_event_layout_matches_track_buffer_abi() {
        assert_eq!(std::mem::size_of::<MidiEventView>(), 272);
        assert_eq!(std::mem::offset_of!(MidiEventView, data), 12);
        assert_eq!(std::mem::offset_of!(MidiEventView, articulation_id), 268);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_builtin_voice_matches_frozen_cpp_reference_across_blocks() {
        let events = [
            MidiEventView {
                sample_offset: 0,
                size: 3,
                data: {
                    let mut d = [0; 256];
                    d[..3].copy_from_slice(&[0x90, 60, 96]);
                    d
                },
                articulation_id: 0,
            },
            MidiEventView {
                sample_offset: 90,
                size: 3,
                data: {
                    let mut d = [0; 256];
                    d[..3].copy_from_slice(&[0x90, 60, 80]);
                    d
                },
                articulation_id: 0,
            },
            MidiEventView {
                sample_offset: 180,
                size: 3,
                data: {
                    let mut d = [0; 256];
                    d[..3].copy_from_slice(&[0x80, 60, 0]);
                    d
                },
                articulation_id: 0,
            },
            MidiEventView {
                sample_offset: 260,
                size: 3,
                data: {
                    let mut d = [0; 256];
                    d[..3].copy_from_slice(&[0xb0, 123, 0]);
                    d
                },
                articulation_id: 0,
            },
        ];
        let mut rust = Instrument::new();
        let cpp = unsafe { hirari_builtin_midi_reference_create() };
        assert!(!cpp.is_null());
        let sample_rate = 48_000.0;
        let mut rust_left = [0.0f32; 512];
        let mut rust_right = [0.0f32; 512];
        let mut cpp_left = [0.0f32; 512];
        let mut cpp_right = [0.0f32; 512];
        for (start, end) in [(0usize, 128usize), (128, 256), (256, 384), (384, 512)] {
            let block_events: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    (event.sample_offset >= start as u64 && event.sample_offset < end as u64).then(
                        || MidiEventView {
                            sample_offset: event.sample_offset - start as u64,
                            size: event.size,
                            data: event.data,
                            articulation_id: event.articulation_id,
                        },
                    )
                })
                .collect();
            unsafe {
                rust.process(
                    block_events.as_ptr(),
                    block_events.len(),
                    rust_left[start..].as_mut_ptr(),
                    rust_right[start..].as_mut_ptr(),
                    end - start,
                    sample_rate,
                );
                hirari_builtin_midi_reference_process(
                    cpp,
                    block_events.as_ptr(),
                    block_events.len(),
                    cpp_left[start..].as_mut_ptr(),
                    cpp_right[start..].as_mut_ptr(),
                    (end - start) as u32,
                    sample_rate,
                );
            }
        }
        unsafe { hirari_builtin_midi_reference_destroy(cpp) };
        for (rust, cpp) in rust_left.iter().zip(cpp_left) {
            assert!((rust - cpp).abs() <= 1.0e-6, "{rust} != {cpp}");
        }
        assert_eq!(rust_left, rust_right);
        assert_eq!(cpp_left, cpp_right);
    }
}
