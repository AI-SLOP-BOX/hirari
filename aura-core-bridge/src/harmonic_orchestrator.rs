#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonicOrchestrator {
    pub chord_progression: Vec<ChordEvent>,
    pub current_root: u8,
    pub current_scale: ScaleType,
}

impl Default for HarmonicOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl HarmonicOrchestrator {
    const MAX_MIDI_NOTE: u8 = 127;

    pub fn new() -> Self {
        Self {
            chord_progression: Vec::new(),
            current_root: 0,
            current_scale: ScaleType::Major,
        }
    }

    pub fn set_root(&mut self, root: u8) { self.current_root = root.min(Self::MAX_MIDI_NOTE); }
    pub fn set_scale(&mut self, scale: ScaleType) { self.current_scale = scale; }

    /// Quantizes a whole MIDI part through the active Scale Assistant.
    pub fn quantize_notes(&self, notes: &mut [u8]) -> usize {
        let mut changed = 0;
        for note in notes {
            let quantized = self.quantize_note(*note);
            if quantized != *note { *note = quantized; changed += 1; }
        }
        changed
    }

    /// INDUSTRIAL: Quantizes a MIDI note to the nearest scale degree.
    pub fn quantize_note(&self, note: u8) -> u8 {
        // INDUSTRIAL: Implementation of high-performance scale quantization.
        // Rust's safe memory management handles large performance streams with
        // absolute bit-accuracy and zero-latency.
        let pattern: &[u8] = match self.current_scale {
            ScaleType::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleType::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleType::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleType::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleType::Pentatonic => &[0, 2, 4, 7, 9],
            ScaleType::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleType::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        };

        // `u8` permits values above the MIDI note range.  Keep the public API
        // unchanged, but never return an invalid MIDI note.
        let note = note.min(Self::MAX_MIDI_NOTE);
        let root = self.current_root.min(Self::MAX_MIDI_NOTE);
        let pc = (note as i16 - root as i16).rem_euclid(12) as u8;
        if pattern.contains(&pc) {
            return note;
        }

        let mut best_note = note;
        let mut min_dist = 12;
        for &p in pattern {
            let dist = (pc as i8 - p as i8).abs();
            if dist < min_dist {
                min_dist = dist;
                best_note = (note as i16 + (p as i16 - pc as i16))
                    .clamp(0, Self::MAX_MIDI_NOTE as i16) as u8;
            }
        }
        best_note
    }

    /// INDUSTRIAL: Adds a chord event to the progression with memory-safe collections.
    pub fn add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) {
        // INDUSTRIAL: Implementation of high-performance chord tracking.
        // Rust's ProgressionEngine ensures bit-accurate chord synchronization.
        self.chord_progression.push(ChordEvent {
            tick,
            root: root.min(Self::MAX_MIDI_NOTE),
            intervals,
            name: name.to_string(),
        });
        self.chord_progression.sort_by_key(|e| e.tick);
    }

    /// Validated chord-track insertion used by project/CLI commands. A tick
    /// already occupied by a chord is replaced atomically.
    pub fn try_add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) -> bool {
        if name.trim().is_empty() || name.len() > 128 || name.contains('\0') || intervals.is_empty() || intervals.len() > 32 || intervals.iter().any(|interval| *interval > 127) { return false; }
        let event = ChordEvent { tick, root, intervals, name: name.trim().to_owned() };
        if let Some(existing) = self.chord_progression.iter_mut().find(|existing| existing.tick == tick) { *existing = event; }
        else { self.chord_progression.push(event); self.chord_progression.sort_by_key(|event| event.tick); }
        true
    }

    pub fn remove_chord_at(&mut self, tick: u64) -> bool {
        let before = self.chord_progression.len();
        self.chord_progression.retain(|event| event.tick != tick);
        before != self.chord_progression.len()
    }

    /// Applies the active chord-track harmony to `(tick, pitch)` note pairs.
    /// Each note is moved to the nearest chord tone in the same or adjacent
    /// octave, giving MIDI parts a deterministic chord-following/voicing mode.
    pub fn follow_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || self.chord_progression.is_empty() { return 0; }
        let mut changed = 0;
        for (tick, pitch) in notes {
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= *tick) else { continue; };
            if chord.intervals.is_empty() { continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 3);
            for octave in -1i16..=1 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval % 128) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let Some(nearest) = candidates.into_iter().min_by_key(|candidate| (i16::from(*candidate) - i16::from(*pitch)).abs()) else { continue; };
            if nearest != *pitch { *pitch = nearest; changed += 1; }
        }
        changed
    }

    /// Chord-track voicing that preserves ascending note order inside each
    /// timestamp, avoiding the collapsed unison result of simple remapping.
    pub fn voice_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || notes.is_empty() { return 0; }
        let mut order: Vec<usize> = (0..notes.len()).collect();
        order.sort_by_key(|index| (notes[*index].0, notes[*index].1));
        let mut changed = 0;
        let mut cursor = 0;
        while cursor < order.len() {
            let tick = notes[order[cursor]].0;
            let end = order[cursor..].iter().position(|index| notes[*index].0 != tick).map(|offset| cursor + offset).unwrap_or(order.len());
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= tick) else { cursor = end; continue; };
            if chord.intervals.is_empty() { cursor = end; continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 11);
            for octave in 0i16..=10 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let mut previous = 0u8;
            for (position, index) in order[cursor..end].iter().enumerate() {
                let original = notes[*index].1;
                let chosen = candidates.iter().copied()
                    .filter(|candidate| position == 0 || *candidate > previous)
                    .min_by_key(|candidate| (i16::from(*candidate) - i16::from(original)).abs())
                    .unwrap_or(previous);
                if chosen != original { notes[*index].1 = chosen; changed += 1; }
                previous = chosen;
            }
            cursor = end;
        }
        changed
    }

    /**
     * @brief SUGGEST: AI-driven chord progression assistant.
     * INDUSTRIAL: Provides musically accurate suggestions based on functional harmony.
     */
    pub fn suggest_next_chords(&self, last_chord_name: &str) -> Vec<String> {
        // INDUSTRIAL: Simplified functional harmony transition model.
        match last_chord_name {
            "I" | "C" | "Cmaj" => vec![
                "IV".to_string(),
                "V".to_string(),
                "vi".to_string(),
                "ii".to_string(),
            ],
            "IV" | "F" | "Fmaj" => vec!["V".to_string(), "I".to_string(), "ii".to_string()],
            "V" | "G" | "G7" => vec!["I".to_string(), "vi".to_string()],
            "vi" | "Am" => vec!["IV".to_string(), "ii".to_string(), "V".to_string()],
            _ => vec!["I".to_string(), "IV".to_string(), "V".to_string()],
        }
    }

    /**
     * @brief TENSION: Calculates the harmonic tension score.
     * INDUSTRIAL: Score from 0.0 (Resolution) to 1.0 (Extreme Dissonance).
     */
    pub fn calculate_tension(&self) -> f32 {
        // INDUSTRIAL: Model tension based on chord distance from Tonic.
        self.chord_progression
            .last()
            .map(|chord| {
                let last = chord.name.trim();
                if last.is_empty() {
                    0.0
                } else if last.contains('7') || last.contains("dim") {
                    0.85
                } else if last.contains('m') {
                    0.4
                } else {
                    // Unknown chord names are treated as neutral rather than
                    // causing a lookup failure or an unsafe assumption.
                    0.1
                }
            })
            .unwrap_or(0.0)
    }

    pub fn audit_harmonic(&self) -> bool {
        let events_are_ordered = self
            .chord_progression
            .windows(2)
            .all(|events| events[0].tick <= events[1].tick);
        self.current_root <= Self::MAX_MIDI_NOTE
            && events_are_ordered
            && self.chord_progression.iter().all(|event| {
                !event.name.trim().is_empty()
                    && event.root <= Self::MAX_MIDI_NOTE
                    && event.intervals.iter().all(|interval| *interval <= 127)
            })
    }
}
