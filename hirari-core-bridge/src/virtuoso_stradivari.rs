use std::ffi::c_void;

const VOICE_COUNT: usize = 8;
const DELAY_SIZE: usize = 4096;
const DELAY_MASK: usize = DELAY_SIZE - 1;
const MIDI_EVENT_BYTES: usize = 256;

#[repr(C)]
pub struct MidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; MIDI_EVENT_BYTES],
    articulation_id: u8,
}

const _: [(); 272] = [(); std::mem::size_of::<MidiEvent>()];

struct Voice {
    buffer: [f32; DELAY_SIZE],
    write_index: usize,
    frequency: f32,
    envelope: f32,
    note: u8,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            buffer: [0.0; DELAY_SIZE],
            write_index: 0,
            frequency: 440.0,
            envelope: 0.0,
            note: 0,
        }
    }
}

struct VirtuosoStradivari {
    sample_rate: f64,
    voices: [Voice; VOICE_COUNT],
    note_frequencies: [f32; 128],
    next_voice: usize,
    random_state: u32,
}

impl VirtuosoStradivari {
    fn new(sample_rate: f64) -> Self {
        let mut synth = Self {
            sample_rate,
            voices: std::array::from_fn(|_| Voice::default()),
            note_frequencies: [0.0; 128],
            next_voice: 0,
            random_state: 0x5EED,
        };
        for (note, frequency) in synth.note_frequencies.iter_mut().enumerate() {
            *frequency = 440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
        }
        synth.reset();
        synth
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.buffer.fill(0.0);
            voice.envelope = 0.0;
            voice.write_index = 0;
        }
    }

    fn fast_rand(&mut self) -> u32 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 17;
        self.random_state ^= self.random_state << 5;
        self.random_state
    }

    fn trigger_note(&mut self, note: u8, velocity: f32) {
        let voice_index = self.next_voice & (VOICE_COUNT - 1);
        self.next_voice = self.next_voice.wrapping_add(1);
        let voice = &mut self.voices[voice_index];
        voice.note = note;
        voice.frequency = self.note_frequencies[(note & 127) as usize];
        voice.envelope = velocity;
        voice.write_index = 0;
    }

    fn process(&mut self, events: &[MidiEvent], left: &mut [f32], right: &mut [f32]) {
        for event in events {
            if event.size >= 3 && event.data[0] & 0xf0 == 0x90 && event.data[2] > 0 {
                self.trigger_note(event.data[1], event.data[2] as f32 / 127.0);
            }
        }

        let frames = left.len().min(right.len());
        for frame in 0..frames {
            let mut sum = 0.0_f32;
            for voice_index in 0..VOICE_COUNT {
                if self.voices[voice_index].envelope <= 0.0001 {
                    continue;
                }
                let delay_samples = self.sample_rate / self.voices[voice_index].frequency as f64;
                let integer_delay = delay_samples as usize;
                let fraction = delay_samples - integer_delay as f64;
                let write_index = self.voices[voice_index].write_index;
                let read_index = write_index.wrapping_sub(integer_delay) & DELAY_MASK;
                let next_index = (read_index + 1) & DELAY_MASK;
                let value = (self.voices[voice_index].buffer[read_index] as f64 * (1.0 - fraction)
                    + self.voices[voice_index].buffer[next_index] as f64 * fraction)
                    as f32;

                let random = self.fast_rand() as f32 / u32::MAX as f32 - 0.5;
                let envelope = self.voices[voice_index].envelope;
                let excitation = random * envelope * 0.1;
                let next_value = (value + excitation) * 0.9994;
                let voice = &mut self.voices[voice_index];
                voice.buffer[voice.write_index] = next_value;
                voice.write_index = (voice.write_index + 1) & DELAY_MASK;
                sum += next_value;
                voice.envelope *= 0.99985;
            }
            left[frame] += sum;
            right[frame] += sum;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_stradivari_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoStradivari::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_stradivari_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<VirtuosoStradivari>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_stradivari_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<VirtuosoStradivari>().as_mut() } {
        state.sample_rate = sample_rate;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_stradivari_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<VirtuosoStradivari>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_stradivari_process(
    state: *mut c_void,
    events: *const MidiEvent,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<VirtuosoStradivari>().as_mut() }) else {
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
