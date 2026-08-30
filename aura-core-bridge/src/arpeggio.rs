pub enum ArpPattern {
    Up,
    Down,
    UpDown,
    Random,
    Chord,
}

pub struct HeldNote {
    pub pitch: u8,
    pub velocity: u8,
}

pub struct ArpeggioOrchestrator {
    pub held_notes: Vec<HeldNote>,
    pub pattern: ArpPattern,
    pub octaves: u32,
    pub current_step: u32,
    pub rng_state: u32,
}

impl Default for ArpeggioOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ArpeggioOrchestrator {
    pub fn new() -> Self {
        Self {
            held_notes: Vec::new(),
            pattern: ArpPattern::Up,
            octaves: 1,
            current_step: 0,
            rng_state: 0x87654321,
        }
    }

    /// INDUSTRIAL: Updates the set of held notes with memory-safe Rust collections and absolute rhythmic sovereignty.
    pub fn update_notes(&mut self, notes: Vec<HeldNote>) {
        self.held_notes = notes;
        self.held_notes.sort_by_key(|n| n.pitch);
    }

    /**
     * @brief CHORD: Returns all held notes simultaneously transposed across the active octaves.
     * INDUSTRIAL: Beyond single-note arpeggiations, this triggers full polyphonic block chords.
     */
    pub fn get_chord_notes(&mut self) -> Vec<(u8, u8)> {
        if self.held_notes.is_empty() {
            return Vec::new();
        }
        let mut chord = Vec::with_capacity(self.held_notes.len() * self.octaves as usize);

        for oct in 0..self.octaves {
            for note in &self.held_notes {
                let pitch = (note.pitch as i32 + (oct as i32 * 12)).clamp(0, 127) as u8;
                chord.push((pitch, note.velocity));
            }
        }
        self.current_step += 1;
        chord
    }

    /**
     * @brief NEXT: Generates the next note in the arpeggio sequence with absolute precision.
     * INDUSTRIAL: Handles Up, Down, UpDown, and fully deterministic Random note selections.
     */
    pub fn get_next_note(&mut self) -> Option<(u8, u8)> {
        if self.held_notes.is_empty() {
            return None;
        }

        let size = self.held_notes.len();

        // Handle chord mode explicitly via delegate to keep API backward-compatible
        if let ArpPattern::Chord = self.pattern {
            let chord = self.get_chord_notes();
            return chord.first().copied(); // Returns first note as fallback for single-note API
        }

        let note_idx = match self.pattern {
            ArpPattern::Up => self.current_step as usize % size,
            ArpPattern::Down => (size - 1) - (self.current_step as usize % size),
            ArpPattern::UpDown => {
                let total_steps = size * 2 - 2;
                let step = self.current_step as usize % total_steps.max(1);
                if step < size {
                    step
                } else {
                    total_steps - step
                }
            }
            ArpPattern::Random => {
                // Deterministic LCG Randomization
                self.rng_state = self.rng_state.wrapping_mul(1103515245).wrapping_add(12345);
                ((self.rng_state >> 16) as usize) % size
            }
            _ => 0,
        };

        let note = &self.held_notes[note_idx];
        let octave = (self.current_step as usize / size) % self.octaves.max(1) as usize;
        let final_pitch = (note.pitch as i32 + (octave as i32 * 12)).clamp(0, 127) as u8;

        self.current_step += 1;
        Some((final_pitch, note.velocity))
    }

    pub fn audit_arpeggio(&self) -> bool {
        let pitches_are_sorted_and_unique = self
            .held_notes
            .windows(2)
            .all(|notes| notes[0].pitch < notes[1].pitch);
        self.octaves > 0 && self.octaves <= 10 && pitches_are_sorted_and_unique
    }
}
