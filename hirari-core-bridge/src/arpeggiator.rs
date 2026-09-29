use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const REALTIME_EVENT_CAPACITY: usize = 1024;
const NO_ACTIVE_NOTE: u8 = 0xff;

/// Native layout shared with Core::MidiEvent. The event list itself remains
/// owned by the C++ MIDI buffer; Rust owns the arpeggiator state and output.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeMidiEvent {
    pub sample_offset: u64,
    pub size: u32,
    pub data: [u8; 256],
    pub articulation_id: u8,
}

impl Default for NativeMidiEvent {
    fn default() -> Self {
        Self {
            sample_offset: 0,
            size: 0,
            data: [0; 256],
            articulation_id: 0,
        }
    }
}

const _: [(); 272] = [(); std::mem::size_of::<NativeMidiEvent>()];

struct RealtimeArpeggiator {
    sample_rate: f64,
    held_notes: [u8; 128],
    held_count: usize,
    active_note: u8,
    channel: u8,
    step_counter: u32,
    mode: AtomicU32,
    output: Vec<NativeMidiEvent>,
}

impl RealtimeArpeggiator {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            held_notes: [0; 128],
            held_count: 0,
            active_note: NO_ACTIVE_NOTE,
            channel: 1,
            step_counter: 0,
            mode: AtomicU32::new(0),
            output: Vec::with_capacity(REALTIME_EVENT_CAPACITY),
        }
    }

    fn reset(&mut self) {
        self.held_count = 0;
        self.active_note = NO_ACTIVE_NOTE;
        self.channel = 1;
        self.step_counter = 0;
        self.output.clear();
    }

    fn push(&mut self, event: NativeMidiEvent) {
        if self.output.len() < REALTIME_EVENT_CAPACITY {
            self.output.push(event);
        }
    }

    fn kill_active_note(&mut self, offset: u64) {
        if self.active_note != NO_ACTIVE_NOTE {
            let mut event = NativeMidiEvent {
                sample_offset: offset,
                size: 3,
                ..Default::default()
            };
            event.data[..3].copy_from_slice(&[0x80, self.active_note, 0]);
            self.push(event);
            self.active_note = NO_ACTIVE_NOTE;
        }
    }

    fn select_index(&self, step: u32, count: usize, mode: u32) -> usize {
        if count == 0 {
            return 0;
        }
        match mode {
            1 => count - 1 - (step as usize % count),
            2 => {
                if (step as usize / count) % 2 == 0 {
                    step as usize % count
                } else {
                    count - 1 - (step as usize % count)
                }
            }
            3 => (step.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) >> 16) as usize % count,
            _ => step as usize % count,
        }
    }

    fn process(
        &mut self,
        input: &[NativeMidiEvent],
        bpm: f64,
        sample_rate: f64,
        block_start: u64,
        num_samples: u32,
    ) -> usize {
        self.output.clear();
        if num_samples == 0 {
            return 0;
        }
        let bpm = if bpm.is_finite() {
            bpm.clamp(20.0, 300.0)
        } else {
            120.0
        };
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 1000.0 {
            sample_rate
        } else if self.sample_rate.is_finite() && self.sample_rate >= 1000.0 {
            self.sample_rate
        } else {
            44_100.0
        };
        let mode = self.mode.load(Ordering::Acquire);
        let step_samples = (sample_rate * 60.0 / bpm / 4.0).max(1.0) as u64;

        for event in input {
            let size = event.size as usize;
            if size < 2 || event.data[0] < 0x80 {
                self.push(*event);
                continue;
            }
            let status = event.data[0] & 0xf0;
            let channel = (event.data[0] & 0x0f) + 1;
            let note = event.data[1];
            if status == 0x90 && size >= 3 && event.data[2] != 0 {
                self.channel = channel;
                if !self.held_notes[..self.held_count].contains(&note)
                    && self.held_count < self.held_notes.len()
                {
                    self.held_notes[self.held_count] = note;
                    self.held_count += 1;
                }
            } else if status == 0x80 || (status == 0x90 && size >= 3 && event.data[2] == 0) {
                if let Some(index) = self.held_notes[..self.held_count]
                    .iter()
                    .position(|&held| held == note)
                {
                    self.held_count -= 1;
                    self.held_notes[index] = self.held_notes[self.held_count];
                }
                if self.held_count == 0 || self.active_note == note {
                    self.kill_active_note(event.sample_offset);
                }
            } else {
                self.push(*event);
            }
        }

        let block_end = block_start.saturating_add(num_samples as u64);
        if self.held_count != 0 {
            let first = block_start.saturating_add(step_samples - 1) / step_samples * step_samples;
            let mut absolute = first;
            while absolute < block_end && self.output.len() + 2 <= REALTIME_EVENT_CAPACITY {
                let offset = absolute - block_start;
                self.kill_active_note(offset);
                let index = self.select_index(self.step_counter, self.held_count, mode);
                self.step_counter = self.step_counter.wrapping_add(1);
                self.active_note = self.held_notes[index];
                let mut event = NativeMidiEvent {
                    sample_offset: offset,
                    size: 3,
                    ..Default::default()
                };
                event.data[..3].copy_from_slice(&[
                    0x90 | (self.channel - 1),
                    self.active_note,
                    100,
                ]);
                self.push(event);
                let Some(next) = absolute.checked_add(step_samples) else {
                    break;
                };
                absolute = next;
            }
        }
        self.output.len()
    }
}

#[no_mangle]
pub extern "C" fn hirari_arpeggiator_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(RealtimeArpeggiator::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<RealtimeArpeggiator>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_set_mode(state: *mut c_void, mode: u32) {
    if let Some(state) = unsafe { state.cast::<RealtimeArpeggiator>().as_mut() } {
        state.mode.store(mode.min(3), Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_get_mode(state: *const c_void) -> u32 {
    unsafe { state.cast::<RealtimeArpeggiator>().as_ref() }
        .map_or(0, |state| state.mode.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<RealtimeArpeggiator>().as_mut() } {
        if sample_rate.is_finite() && sample_rate > 1000.0 {
            state.sample_rate = sample_rate;
        }
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<RealtimeArpeggiator>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_process(
    state: *mut c_void,
    events: *const NativeMidiEvent,
    count: usize,
    bpm: f64,
    sample_rate: f64,
    block_start: u64,
    num_samples: u32,
) -> usize {
    let Some(state) = (unsafe { state.cast::<RealtimeArpeggiator>().as_mut() }) else {
        return 0;
    };
    if events.is_null() && count != 0 {
        return 0;
    }
    let input = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(events, count.min(REALTIME_EVENT_CAPACITY)) }
    };
    state.process(input, bpm, sample_rate, block_start, num_samples)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_arpeggiator_output(state: *const c_void) -> *const NativeMidiEvent {
    unsafe { state.cast::<RealtimeArpeggiator>().as_ref() }
        .map_or(std::ptr::null(), |state| state.output.as_ptr())
}

#[cfg(test)]
mod realtime_tests {
    use super::{NativeMidiEvent, RealtimeArpeggiator};

    fn event(offset: u64, bytes: &[u8], articulation_id: u8) -> NativeMidiEvent {
        let mut event = NativeMidiEvent {
            sample_offset: offset,
            size: bytes.len() as u32,
            articulation_id,
            ..Default::default()
        };
        event.data[..bytes.len()].copy_from_slice(bytes);
        event
    }

    #[test]
    fn sample_accurate_steps_emit_note_lifetimes_without_allocating_per_block() {
        let mut arp = RealtimeArpeggiator::new(48_000.0);
        let input = [event(0, &[0x90, 60, 90], 3), event(0, &[0x90, 64, 90], 4)];
        let count = arp.process(&input, 120.0, 48_000.0, 0, 24_000);
        let output = &arp.output[..count];

        assert_eq!(output.len(), 7);
        assert_eq!(
            (output[0].sample_offset, output[0].data[..3].to_vec()),
            (0, vec![0x90, 60, 100])
        );
        assert_eq!(
            (output[1].sample_offset, output[1].data[..3].to_vec()),
            (6_000, vec![0x80, 60, 0])
        );
        assert_eq!(
            (output[2].sample_offset, output[2].data[..3].to_vec()),
            (6_000, vec![0x90, 64, 100])
        );
        assert_eq!(
            (output[3].sample_offset, output[3].data[..3].to_vec()),
            (12_000, vec![0x80, 64, 0])
        );
        assert_eq!(
            (output[4].sample_offset, output[4].data[..3].to_vec()),
            (12_000, vec![0x90, 60, 100])
        );
        assert_eq!(
            (output[5].sample_offset, output[5].data[..3].to_vec()),
            (18_000, vec![0x80, 60, 0])
        );
        assert_eq!(
            (output[6].sample_offset, output[6].data[..3].to_vec()),
            (18_000, vec![0x90, 64, 100])
        );
        assert_eq!(output[0].articulation_id, 0);
        assert_eq!(arp.output.capacity(), super::REALTIME_EVENT_CAPACITY);
    }

    #[test]
    fn non_note_events_pass_through_and_keep_articulation_metadata() {
        let mut arp = RealtimeArpeggiator::new(48_000.0);
        let input = [event(17, &[0xb2, 1, 65], 9), event(23, &[0x90, 60, 100], 2)];
        let count = arp.process(&input, 120.0, 48_000.0, 1, 1);

        assert_eq!(count, 1);
        assert_eq!(arp.output[0], input[0]);
        assert_eq!(arp.held_notes[..arp.held_count], [60]);
    }

    #[test]
    fn patterns_and_reset_preserve_the_cpp_mode_selection_contract() {
        let mut arp = RealtimeArpeggiator::new(48_000.0);
        arp.held_notes[..3].copy_from_slice(&[60, 64, 67]);
        arp.held_count = 3;
        assert_eq!(
            (0..6)
                .map(|step| arp.select_index(step, 3, 0))
                .collect::<Vec<_>>(),
            [0, 1, 2, 0, 1, 2]
        );
        assert_eq!(
            (0..3)
                .map(|step| arp.select_index(step, 3, 1))
                .collect::<Vec<_>>(),
            [2, 1, 0]
        );
        assert_eq!(
            (0..6)
                .map(|step| arp.select_index(step, 3, 2))
                .collect::<Vec<_>>(),
            [0, 1, 2, 2, 1, 0]
        );
        arp.reset();
        assert_eq!(arp.held_count, 0);
        assert_eq!(arp.active_note, super::NO_ACTIVE_NOTE);
    }
}

pub enum ArpeggiatorMode {
    Up,
    Down,
    Range,
    Random,
}

pub struct ArpeggiatorEngine {
    pub sample_rate: f64,
    pub held_notes: Vec<u8>,
    pub active_note: u8,
    pub step_counter: usize,
    pub mode: ArpeggiatorMode,
    pub rng_state: u32,
    pub going_up: bool,
}

impl ArpeggiatorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            held_notes: Vec::new(),
            active_note: 0xFF, // Sentinel for 'None'
            step_counter: 0,
            mode: ArpeggiatorMode::Up,
            rng_state: 0x12345678,
            going_up: true,
        }
    }

    pub fn reset(&mut self) {
        self.held_notes.clear();
        self.active_note = 0xFF;
        self.step_counter = 0;
        self.going_up = true;
    }

    fn kill_active_note(&mut self, events: &mut Vec<(u32, Vec<u8>)>, offset: u32) {
        if self.active_note != 0xFF {
            let note_off = vec![0x80, self.active_note, 0];
            events.push((offset, note_off));
            self.active_note = 0xFF;
        }
    }

    /**
     * @brief PROCESS: Professional MIDI Arpeggiator with note-lifespan management and fully implemented patterns.
     * INDUSTRIAL: Respects Up, Down, Range (UpDown), and Random modes deterministically.
     */
    pub fn process(
        &mut self,
        events: &mut Vec<(u32, Vec<u8>)>,
        bpm: f64,
        playhead: u64,
        num_samples: u32,
    ) {
        let mut output_events = Vec::with_capacity(events.len());
        if !bpm.is_finite()
            || bpm <= 0.0
            || !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || num_samples == 0
        {
            output_events.append(events);
            *events = output_events;
            return;
        }

        // 1. DYNAMICALLY CAPTURE HELD NOTES
        for (offset, data) in events.drain(..) {
            if data.len() < 3 {
                output_events.push((offset, data));
                continue;
            }

            let status = data[0] & 0xF0;
            let note = data[1];
            let vel = data[2];

            if status == 0x90 && vel > 0 {
                // Note On
                if !self.held_notes.contains(&note) {
                    self.held_notes.push(note);
                }
            } else if status == 0x80 || (status == 0x90 && vel == 0) {
                // Note Off
                if let Some(pos) = self.held_notes.iter().position(|&n| n == note) {
                    self.held_notes.remove(pos);
                }
            } else {
                output_events.push((offset, data));
            }
        }

        if self.held_notes.is_empty() {
            self.kill_active_note(&mut output_events, 0);
            *events = output_events;
            return;
        }

        self.held_notes.sort();

        // Note Off may have removed the note at (or before) the current step.
        // Keep the public counter valid before it is used for indexing below.
        let size = self.held_notes.len();
        self.step_counter %= size;

        // 2. PATTERN SYNC (1/16th Note resolution)
        let samples_per_16th = (60.0 / bpm) * self.sample_rate / 4.0;
        let current_sample = playhead;

        // Check if a trigger point exists within this buffer
        let next_trigger_sample =
            ((current_sample as f64 / samples_per_16th).floor() + 1.0) * samples_per_16th;
        let next_trigger_sample = if next_trigger_sample.is_finite() && next_trigger_sample >= 0.0 {
            next_trigger_sample.min(u64::MAX as f64) as u64
        } else {
            current_sample
        };
        let block_end = current_sample.saturating_add(num_samples as u64);

        if next_trigger_sample >= current_sample && next_trigger_sample < block_end {
            let trigger_offset = (next_trigger_sample - current_sample) as u32;

            // --- PROFESSIONAL KILL PREVIOUS ---
            self.kill_active_note(&mut output_events, trigger_offset);

            // 3. DIRECTIONAL STEP CALCULATIONS
            match self.mode {
                ArpeggiatorMode::Up => {
                    self.step_counter = if self.step_counter + 1 >= size {
                        0
                    } else {
                        self.step_counter + 1
                    };
                }
                ArpeggiatorMode::Down => {
                    self.step_counter = if self.step_counter == 0 {
                        size - 1
                    } else {
                        self.step_counter - 1
                    };
                }
                ArpeggiatorMode::Range => {
                    if size <= 1 {
                        self.step_counter = 0;
                    } else if self.going_up {
                        if self.step_counter >= size - 1 {
                            self.going_up = false;
                            self.step_counter = size - 2;
                        } else {
                            self.step_counter += 1;
                        }
                    } else if self.step_counter == 0 {
                        self.going_up = true;
                        self.step_counter = 1;
                    } else {
                        self.step_counter -= 1;
                    }
                }
                ArpeggiatorMode::Random => {
                    // LCG fast random step selection
                    self.rng_state = self.rng_state.wrapping_mul(1103515245).wrapping_add(12345);
                    self.step_counter = ((self.rng_state >> 16) as usize) % size;
                }
            }

            self.active_note = self.held_notes[self.step_counter];

            let note_on = vec![0x90, self.active_note, 100];
            output_events.push((trigger_offset, note_on));
        }

        *events = output_events;
    }

    pub fn audit_arpeggiator(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.step_counter < self.held_notes.len().max(1)
            && self.held_notes.windows(2).all(|pair| pair[0] < pair[1])
            && (self.active_note == 0xFF || self.held_notes.contains(&self.active_note))
            && self.rng_state != 0
    }
}
