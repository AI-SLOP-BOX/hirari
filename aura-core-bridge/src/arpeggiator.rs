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
