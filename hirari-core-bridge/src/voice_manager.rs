use std::ffi::c_void;

type VoiceTriggerCallback = unsafe extern "C" fn(*mut c_void, u8, u8);
type VoiceReleaseCallback = unsafe extern "C" fn(*mut c_void, u8);
type VoiceRenderCallback = unsafe extern "C" fn(*mut c_void, *mut f32, *mut f32, usize);

#[repr(C)]
struct MidiEventView {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

/// Performs sample-accurate MIDI dispatch and partitions rendering around
/// events. Concrete voice storage and render algorithms stay behind callbacks.
///
/// # Safety
/// `midi_state` must be a live Rust-owned MidiBuffer state, outputs must each
/// have `frames` writable samples, and callbacks must not unwind across C ABI.
#[no_mangle]
pub unsafe extern "C" fn hirari_voice_manager_render_midi(
    midi_state: *mut c_void,
    user_data: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    trigger: Option<VoiceTriggerCallback>,
    release: Option<VoiceReleaseCallback>,
    render_range: Option<VoiceRenderCallback>,
) {
    if midi_state.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let (Some(trigger), Some(release), Some(render_range)) = (trigger, release, render_range)
    else {
        return;
    };

    // MidiBuffer is already Rust-owned; sorting and storage stay in that owner.
    unsafe { crate::midi_buffer_ops::hirari_midi_buffer_sort_owned(midi_state) };
    let events = unsafe { crate::midi_buffer_ops::hirari_midi_buffer_event_data(midi_state) }
        .cast::<MidiEventView>();
    let event_count = unsafe { crate::midi_buffer_ops::hirari_midi_buffer_event_count(midi_state) };
    if events.is_null() || event_count > 1024 {
        return;
    }
    for frame in 0..frames {
        unsafe {
            left.add(frame).write(0.0);
            right.add(frame).write(0.0);
        }
    }

    let events = unsafe { std::slice::from_raw_parts(events, event_count) };
    let mut cursor = 0usize;
    for event in events {
        let event_offset = event.sample_offset.min(frames as u64) as usize;
        if event_offset > cursor {
            unsafe {
                render_range(
                    user_data,
                    left.add(cursor),
                    right.add(cursor),
                    event_offset - cursor,
                );
            }
            cursor = event_offset;
        }
        if event.size < 2 {
            continue;
        }
        let status = event.data[0] & 0xf0;
        let note = event.data[1] & 0x7f;
        let value = if event.size >= 3 {
            event.data[2] & 0x7f
        } else {
            0
        };
        if status == 0x90 && value != 0 {
            unsafe { trigger(user_data, note, value) };
        } else if status == 0x80 || (status == 0x90 && value == 0) {
            unsafe { release(user_data, note) };
        }
    }
    if cursor < frames {
        unsafe {
            render_range(
                user_data,
                left.add(cursor),
                right.add(cursor),
                frames - cursor,
            )
        };
    }
}

pub struct Voice {
    pub is_active: bool,
    pub note: u8,
    pub velocity: f32,
    phase: f32,
}

pub struct VoiceManagerEngine {
    pub voices: Vec<Voice>,
    pub max_voices: usize,
}

impl VoiceManagerEngine {
    pub fn new(max_voices: usize) -> Self {
        let max_voices = max_voices.min(4096);
        let mut voices = Vec::with_capacity(max_voices);
        for _ in 0..max_voices {
            voices.push(Voice {
                is_active: false,
                note: 0,
                velocity: 0.0,
                phase: 0.0,
            });
        }
        Self { voices, max_voices }
    }

    pub fn trigger_voice(&mut self, note: u8, velocity: u8) {
        let vel_float = velocity as f32 / 127.0;

        // 1. Check if note is already playing
        for v in &mut self.voices {
            if v.is_active && v.note == note {
                v.velocity = vel_float;
                return;
            }
        }

        // 2. Find free voice
        for v in &mut self.voices {
            if !v.is_active {
                v.is_active = true;
                v.note = note;
                v.velocity = vel_float;
                v.phase = 0.0;
                return;
            }
        }

        // 3. Voice Stealing (Simplistic: steal the first one)
        if !self.voices.is_empty() {
            let v = &mut self.voices[0];
            v.is_active = true;
            v.note = note;
            v.velocity = vel_float;
            v.phase = 0.0;
        }
    }

    pub fn release_voice(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.is_active && v.note == note {
                // In a real ADSR, this would trigger release phase.
                // Here we just mark it inactive for simplicity.
                v.is_active = false;
                v.phase = 0.0;
            }
        }
    }

    /// INDUSTRIAL: Coordinates voice rendering.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        // Keep the bridge self-contained: each active voice contributes a
        // bounded sine wave to the shared output buffers.  `zip` also makes
        // mismatched channel lengths safe without allocating a temporary
        // buffer.
        const SAMPLE_RATE: f32 = 44_100.0;
        const TWO_PI: f32 = core::f32::consts::TAU;
        const VOICE_GAIN: f32 = 0.1;

        if l.is_empty() || r.is_empty() {
            return;
        }

        for voice in &mut self.voices {
            if !voice.is_active {
                continue;
            }

            let frequency = 440.0 * 2.0f32.powf((voice.note as f32 - 69.0) / 12.0);
            if !frequency.is_finite() {
                continue;
            }

            let gain = voice.velocity.clamp(0.0, 1.0) * VOICE_GAIN;
            let phase_step = frequency / SAMPLE_RATE;
            for (left, right) in l.iter_mut().zip(r.iter_mut()) {
                let sample = (voice.phase * TWO_PI).sin() * gain;
                if sample.is_finite() {
                    *left += sample;
                    *right += sample;
                }
                voice.phase = (voice.phase + phase_step).fract();
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Voice Manager state.
    pub fn audit_voice_manager(&self) -> bool {
        self.voices.len() <= self.max_voices
            && self.voices.iter().all(|voice| {
                voice.velocity.is_finite()
                    && (0.0..=1.0).contains(&voice.velocity)
                    && voice.phase.is_finite()
                    && (0.0..1.0).contains(&voice.phase)
            })
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::hirari_voice_manager_render_midi;
    use std::ffi::c_void;

    struct Trace(Vec<(u8, u32, usize)>);

    unsafe extern "C" fn note_on(user_data: *mut c_void, note: u8, velocity: u8) {
        unsafe { &mut *user_data.cast::<Trace>() }
            .0
            .push((1, note as u32, velocity as usize));
    }

    unsafe extern "C" fn note_off(user_data: *mut c_void, note: u8) {
        unsafe { &mut *user_data.cast::<Trace>() }
            .0
            .push((2, note as u32, 0));
    }

    unsafe extern "C" fn render_range(
        user_data: *mut c_void,
        left: *mut f32,
        right: *mut f32,
        frames: usize,
    ) {
        unsafe { &mut *user_data.cast::<Trace>() }
            .0
            .push((0, 0, frames));
        for frame in 0..frames {
            unsafe {
                left.add(frame).write(0.25);
                right.add(frame).write(-0.25);
            }
        }
    }

    #[test]
    fn midi_events_are_sorted_and_split_render_ranges_at_sample_offsets() {
        let state = crate::midi_buffer_ops::hirari_midi_buffer_create();
        assert!(!state.is_null());
        let on = [0x90u8, 61, 100];
        let off = [0x80u8, 60, 0];
        let end_off = [0x80u8, 61, 0];
        unsafe {
            crate::midi_buffer_ops::hirari_midi_buffer_add(state, 2, on.as_ptr(), 3, 0);
            crate::midi_buffer_ops::hirari_midi_buffer_add(state, 2, off.as_ptr(), 3, 0);
            crate::midi_buffer_ops::hirari_midi_buffer_add(state, 6, end_off.as_ptr(), 3, 0);
        }
        let mut trace = Trace(Vec::new());
        let mut left = [9.0f32; 10];
        let mut right = [9.0f32; 10];
        unsafe {
            hirari_voice_manager_render_midi(
                state,
                (&mut trace as *mut Trace).cast(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len(),
                Some(note_on),
                Some(note_off),
                Some(render_range),
            );
            crate::midi_buffer_ops::hirari_midi_buffer_destroy(state);
        }
        assert_eq!(
            trace.0,
            vec![
                (0, 0, 2),
                (2, 60, 0),
                (1, 61, 100),
                (0, 0, 4),
                (2, 61, 0),
                (0, 0, 4),
            ]
        );
        assert_eq!(left, [0.25; 10]);
        assert_eq!(right, [-0.25; 10]);
    }
}
