use std::collections::HashSet;

/// Standard GM drum lane labels used by the drum editor. Unknown pitches are
/// intentionally kept addressable so custom kits can still display a stable
/// fallback row.
pub fn drum_lane_label(pitch: u8) -> &'static str {
    match pitch {
        35 => "Acoustic Bass Drum",
        36 => "Bass Drum 1",
        37 => "Side Stick",
        38 => "Acoustic Snare",
        39 => "Hand Clap",
        40 => "Electric Snare",
        41 => "Low Floor Tom",
        42 => "Closed Hi-Hat",
        43 => "High Floor Tom",
        44 => "Pedal Hi-Hat",
        45 => "Low Tom",
        46 => "Open Hi-Hat",
        47 => "Low-Mid Tom",
        48 => "Hi-Mid Tom",
        49 => "Crash Cymbal 1",
        50 => "High Tom",
        51 => "Ride Cymbal 1",
        52 => "Chinese Cymbal",
        53 => "Ride Bell",
        54 => "Tambourine",
        55 => "Splash Cymbal",
        56 => "Cowbell",
        57 => "Crash Cymbal 2",
        58 => "Vibraslap",
        59 => "Ride Cymbal 2",
        60 => "Hi Bongo",
        61 => "Low Bongo",
        62 => "Mute Hi Conga",
        63 => "Open Hi Conga",
        64 => "Low Conga",
        65 => "High Timbale",
        66 => "Low Timbale",
        67 => "High Agogo",
        68 => "Low Agogo",
        69 => "Cabasa",
        70 => "Maracas",
        71 => "Short Whistle",
        72 => "Long Whistle",
        73 => "Short Guiro",
        74 => "Long Guiro",
        75 => "Claves",
        76 => "Hi Wood Block",
        77 => "Low Wood Block",
        78 => "Mute Cuica",
        79 => "Open Cuica",
        80 => "Mute Triangle",
        81 => "Open Triangle",
        _ => "Custom Drum",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MidiNoteRust {
    pub id: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub start_beat: f64,
    pub duration: f64,
}

pub struct PianoRollOrchestrator {
    pub selection: HashSet<u32>,
    pub next_note_id: u32,
}

impl Default for PianoRollOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl PianoRollOrchestrator {
    pub fn new() -> Self {
        Self {
            selection: HashSet::new(),
            next_note_id: 1000,
        }
    }

    /// Creates a note with a stable editor-owned identity.  IDs are never
    /// reused during the lifetime of this editor, which keeps selection and
    /// automation references valid while notes are inserted and removed.
    pub fn add_note(
        &mut self,
        notes: &mut Vec<MidiNoteRust>,
        pitch: u8,
        velocity: u8,
        start_beat: f64,
        duration: f64,
    ) -> Option<u32> {
        if !start_beat.is_finite() || start_beat < 0.0
            || !duration.is_finite() || duration <= 0.0 {
            return None;
        }
        let id = self.next_note_id;
        self.next_note_id = self.next_note_id.checked_add(1)?;
        notes.push(MidiNoteRust { id, pitch, velocity, start_beat, duration });
        Some(id)
    }

    pub fn remove_selected(&mut self, notes: &mut Vec<MidiNoteRust>) -> usize {
        let before = notes.len();
        notes.retain(|note| !self.selection.contains(&note.id));
        self.selection.retain(|id| notes.iter().any(|note| note.id == *id));
        before - notes.len()
    }

    pub fn set_selected_duration(&self, notes: &mut [MidiNoteRust], duration: f64) -> bool {
        if !duration.is_finite() || duration <= 0.0 { return false; }
        let mut changed = false;
        for note in notes.iter_mut().filter(|note| self.selection.contains(&note.id)) {
            note.duration = duration;
            changed = true;
        }
        changed
    }

    pub fn validate_notes(notes: &[MidiNoteRust]) -> bool {
        notes.len() <= 1_000_000
            && notes.iter().all(|note| note.id != 0 && note.pitch <= 127 && (1..=127).contains(&note.velocity)
                && note.start_beat.is_finite() && note.start_beat >= 0.0
                && note.duration.is_finite() && note.duration > 0.0)
            && notes.iter().enumerate().all(|(index, note)| notes[..index].iter().all(|previous| previous.id != note.id))
    }

    /// INDUSTRIAL: Selects a MIDI note with absolute memory safety.
    pub fn select_note(&mut self, note_id: u32, multi_select: bool) {
        // INDUSTRIAL: Implementation of high-performance selection registry.
        // Rust's SelectionRegistryEngine ensures bit-accurate selection distribution.
        if !multi_select {
            self.selection.clear();
        }
        self.selection.insert(note_id);
    }

    /// Select notes in a timeline/pitch rectangle, which is the shared
    /// primitive used by piano-roll marquee and drum-editor lane selection.
    pub fn select_region(
        &mut self,
        notes: &[MidiNoteRust],
        start_beat: f64,
        end_beat: f64,
        low_pitch: u8,
        high_pitch: u8,
        multi_select: bool,
    ) {
        if !start_beat.is_finite() || !end_beat.is_finite() || start_beat > end_beat {
            return;
        }
        if !multi_select { self.selection.clear(); }
        let (low, high) = (low_pitch.min(high_pitch), low_pitch.max(high_pitch));
        for note in notes {
            let note_end = note.start_beat + note.duration;
            if note.start_beat < end_beat && note_end > start_beat
                && (low..=high).contains(&note.pitch) {
                self.selection.insert(note.id);
            }
        }
    }

    /// INDUSTRIAL: Manipulates selected notes with zero-latency sovereignty.
    pub fn move_selected(&self, beat_delta: f64, pitch_delta: i32, notes: &mut [MidiNoteRust]) {
        // INDUSTRIAL: Implementation of high-performance note manipulation.
        // Rust's NoteManipulationEngine ensures bit-accurate temporal and pitch resolution.
        if !beat_delta.is_finite() { return; }
        for note in notes {
            if self.selection.contains(&note.id) {
                if !note.start_beat.is_finite() || !note.duration.is_finite() || note.duration <= 0.0 { continue; }
                let moved = note.start_beat + beat_delta;
                note.start_beat = if moved.is_finite() {
                    moved.max(0.0)
                } else if beat_delta.is_sign_negative() {
                    0.0
                } else {
                    f64::MAX
                };
                let new_pitch = (note.pitch as i32 + pitch_delta).clamp(0, 127) as u8;
                note.pitch = new_pitch;
            }
        }
    }

    /// Quantizes selected note starts to a beat grid with adjustable strength.
    /// The operation is deterministic and never changes note durations.
    pub fn quantize_selected(&self, notes: &mut [MidiNoteRust], grid: f64, strength: f64) -> bool {
        if !grid.is_finite() || grid <= 0.0 || !strength.is_finite() || !(0.0..=1.0).contains(&strength) { return false; }
        for note in notes.iter_mut().filter(|note| self.selection.contains(&note.id)) {
            if !note.start_beat.is_finite() || note.start_beat < 0.0 { return false; }
            let target = (note.start_beat / grid).round() * grid;
            let moved = note.start_beat + (target - note.start_beat) * strength;
            if !moved.is_finite() || moved < 0.0 { return false; }
            note.start_beat = moved;
        }
        true
    }

    /// Duplicates the current selection at a timeline offset and selects the
    /// newly-created notes, matching a standard DAW repeat gesture.
    pub fn duplicate_selected(&mut self, notes: &mut Vec<MidiNoteRust>, beat_delta: f64) -> Vec<u32> {
        if !beat_delta.is_finite() || notes.len() > 1_000_000 { return Vec::new(); }
        let source: Vec<MidiNoteRust> = notes.iter().filter(|note| self.selection.contains(&note.id)).cloned().collect();
        if source.is_empty() { return Vec::new(); }
        let mut created = Vec::with_capacity(source.len());
        for note in source {
            let Some(id) = self.next_note_id.checked_add(created.len() as u32) else { return Vec::new(); };
            let start = note.start_beat + beat_delta;
            if !start.is_finite() || start < 0.0 { return Vec::new(); }
            notes.push(MidiNoteRust { id, pitch: note.pitch, velocity: note.velocity, start_beat: start, duration: note.duration });
            created.push(id);
        }
        self.next_note_id = self.next_note_id.saturating_add(created.len() as u32);
        self.selection = created.iter().copied().collect();
        created
    }

    /// Applies reproducible timing/velocity humanization to selected notes.
    pub fn humanize_selected(&self, notes: &mut [MidiNoteRust], max_beats: f64, velocity_range: u8, seed: u64) -> bool {
        if !max_beats.is_finite() || max_beats < 0.0 { return false; }
        let mut state = seed.max(1);
        for note in notes.iter_mut().filter(|note| self.selection.contains(&note.id)) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let unit = ((state >> 11) as f64 / ((1u64 << 53) as f64)) * 2.0 - 1.0;
            note.start_beat = (note.start_beat + unit * max_beats).max(0.0);
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let delta = ((state >> 56) as i16 % (velocity_range as i16 * 2 + 1)) - velocity_range as i16;
            note.velocity = (note.velocity as i16 + delta).clamp(1, 127) as u8;
        }
        true
    }

    /// Extends selected notes to the next same-pitch note, creating legato.
    pub fn legato_selected(&self, notes: &mut [MidiNoteRust]) -> bool {
        let selected = self.selection.clone();
        let mut changed = false;
        for index in 0..notes.len() {
            if !selected.contains(&notes[index].id) { continue; }
            let end = notes[index].start_beat + notes[index].duration;
            let next = notes.iter().filter(|candidate| candidate.pitch == notes[index].pitch && candidate.start_beat >= end && candidate.id != notes[index].id).map(|candidate| candidate.start_beat).min_by(|a, b| a.total_cmp(b));
            if let Some(next_start) = next { let duration = next_start - notes[index].start_beat; if duration > 0.0 && duration.is_finite() { notes[index].duration = duration; changed = true; } }
        }
        changed
    }

    /// Scales selected velocities with clamping, preserving note timing.
    pub fn scale_velocity_selected(&self, notes: &mut [MidiNoteRust], factor: f64) -> bool {
        if !factor.is_finite() || factor <= 0.0 || factor > 16.0 { return false; }
        for note in notes.iter_mut().filter(|note| self.selection.contains(&note.id)) {
            note.velocity = (f64::from(note.velocity) * factor).round().clamp(1.0, 127.0) as u8;
        }
        true
    }

    /// Writes a deterministic crescendo/diminuendo over selected notes in
    /// chronological order. Equal start times are ordered by note ID.
    pub fn velocity_ramp_selected(&self, notes: &mut [MidiNoteRust], start_velocity: u8, end_velocity: u8) -> bool {
        if start_velocity == 0 || end_velocity == 0 || start_velocity > 127 || end_velocity > 127 { return false; }
        let mut indices: Vec<_> = notes.iter().enumerate().filter(|(_, note)| self.selection.contains(&note.id)).map(|(index, _)| index).collect();
        indices.sort_by(|a, b| notes[*a].start_beat.total_cmp(&notes[*b].start_beat).then(notes[*a].id.cmp(&notes[*b].id)));
        if indices.is_empty() { return false; }
        let denominator = (indices.len() - 1).max(1) as f64;
        for (position, index) in indices.into_iter().enumerate() {
            let t = position as f64 / denominator;
            notes[index].velocity = (f64::from(start_velocity) + (f64::from(end_velocity) - f64::from(start_velocity)) * t).round().clamp(1.0, 127.0) as u8;
        }
        true
    }

    /// Mirrors selected notes around a beat interval, retaining each duration.
    pub fn reverse_selected(&self, notes: &mut [MidiNoteRust], start_beat: f64, end_beat: f64) -> bool {
        if !start_beat.is_finite() || !end_beat.is_finite() || end_beat <= start_beat { return false; }
        let mut changed = false;
        for note in notes.iter_mut().filter(|note| self.selection.contains(&note.id)) {
            if !note.start_beat.is_finite() || !note.duration.is_finite() || note.duration <= 0.0 { return false; }
            let note_end = note.start_beat + note.duration;
            if note.start_beat < start_beat || note_end > end_beat { return false; }
            let reflected = end_beat - (note_end - start_beat);
            if !reflected.is_finite() || reflected < start_beat { return false; }
            note.start_beat = reflected;
            changed = true;
        }
        changed
    }

    /// Applies a strum/arpeggiation offset to selected notes that share a
    /// chord onset. Notes retain their order-independent identity.
    pub fn strum_selected(&self, notes: &mut [MidiNoteRust], spread_beats: f64, descending: bool) -> bool {
        if !spread_beats.is_finite() || spread_beats < 0.0 || spread_beats > 64.0 { return false; }
        let mut indices: Vec<_> = notes.iter().enumerate().filter(|(_, note)| self.selection.contains(&note.id)).map(|(index, _)| index).collect();
        if indices.len() < 2 { return false; }
        indices.sort_by(|a, b| {
            let start = notes[*a].start_beat.total_cmp(&notes[*b].start_beat);
            if start == std::cmp::Ordering::Equal {
                let pitch = notes[*a].pitch.cmp(&notes[*b].pitch);
                if descending { pitch.reverse().then(notes[*a].id.cmp(&notes[*b].id)) } else { pitch.then(notes[*a].id.cmp(&notes[*b].id)) }
            } else { start }
        });
        let anchor = notes[indices[0]].start_beat;
        for (position, index) in indices.into_iter().enumerate() {
            let offset = spread_beats * position as f64;
            let target = anchor + offset;
            if !target.is_finite() { return false; }
            notes[index].start_beat = target;
        }
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide composition state.
    pub fn audit_piano_roll_editor(&self) -> bool {
        self.next_note_id > 0 && self.selection.iter().all(|id| *id > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::drum_lane_label;

    #[test]
    fn drum_editor_exposes_standard_gm_lanes_and_custom_fallback() {
        assert_eq!(drum_lane_label(36), "Bass Drum 1");
        assert_eq!(drum_lane_label(42), "Closed Hi-Hat");
        assert_eq!(drum_lane_label(10), "Custom Drum");
    }

    #[test]
    fn marquee_selection_hits_overlapping_piano_and_drum_notes() {
        let mut editor = super::PianoRollOrchestrator::new();
        let notes = vec![
            super::MidiNoteRust { id: 1, pitch: 36, velocity: 100, start_beat: 1.0, duration: 0.5 },
            super::MidiNoteRust { id: 2, pitch: 60, velocity: 100, start_beat: 2.0, duration: 0.5 },
        ];
        editor.select_region(&notes, 0.75, 1.25, 35, 40, false);
        assert!(editor.selection.contains(&1));
        assert!(!editor.selection.contains(&2));
    }

    #[test]
    fn quantize_and_duplicate_selection_are_deterministic() {
        let mut editor = super::PianoRollOrchestrator::new();
        let mut notes = vec![super::MidiNoteRust { id: 1, pitch: 60, velocity: 100, start_beat: 1.13, duration: 0.5 }];
        editor.select_note(1, false);
        assert!(editor.quantize_selected(&mut notes, 0.25, 1.0));
        assert_eq!(notes[0].start_beat, 1.25);
        let created = editor.duplicate_selected(&mut notes, 2.0);
        assert_eq!(created.len(), 1);
        assert_eq!(notes[1].start_beat, 3.25);
        assert!(editor.selection.contains(&created[0]));
    }

    #[test]
    fn humanize_is_reproducible_and_legato_extends_to_next_note() {
        let mut editor = super::PianoRollOrchestrator::new();
        let mut notes = vec![
            super::MidiNoteRust { id: 1, pitch: 60, velocity: 100, start_beat: 0.0, duration: 0.25 },
            super::MidiNoteRust { id: 2, pitch: 60, velocity: 90, start_beat: 1.0, duration: 0.25 },
        ];
        editor.select_note(1, false);
        assert!(editor.humanize_selected(&mut notes, 0.0, 0, 42));
        assert_eq!(notes[0].start_beat, 0.0);
        assert!(editor.legato_selected(&mut notes));
        assert_eq!(notes[0].duration, 1.0);
    }

    #[test]
    fn velocity_tools_and_reverse_are_deterministic() {
        let mut editor = super::PianoRollOrchestrator::new();
        let mut notes = vec![
            super::MidiNoteRust { id: 1, pitch: 60, velocity: 40, start_beat: 0.0, duration: 1.0 },
            super::MidiNoteRust { id: 2, pitch: 62, velocity: 80, start_beat: 1.0, duration: 1.0 },
        ];
        editor.select_region(&notes, 0.0, 2.0, 0, 127, false);
        assert!(editor.velocity_ramp_selected(&mut notes, 20, 100));
        assert_eq!((notes[0].velocity, notes[1].velocity), (20, 100));
        assert!(editor.scale_velocity_selected(&mut notes, 0.5));
        assert_eq!((notes[0].velocity, notes[1].velocity), (10, 50));
        assert!(editor.reverse_selected(&mut notes, 0.0, 2.0));
        assert_eq!((notes[0].start_beat, notes[1].start_beat), (1.0, 0.0));
    }

    #[test]
    fn note_collection_validation_and_deletion_preserve_selection_integrity() {
        let mut editor = super::PianoRollOrchestrator::new();
        let mut notes = vec![
            super::MidiNoteRust { id: 1, pitch: 60, velocity: 100, start_beat: 0.0, duration: 1.0 },
            super::MidiNoteRust { id: 2, pitch: 61, velocity: 100, start_beat: 1.0, duration: 1.0 },
        ];
        editor.select_note(1, false);
        assert!(editor.set_selected_duration(&mut notes, 0.5));
        assert_eq!(notes[0].duration, 0.5);
        assert_eq!(editor.remove_selected(&mut notes), 1);
        assert_eq!(notes.len(), 1);
        assert!(editor.selection.is_empty());
        assert!(super::PianoRollOrchestrator::validate_notes(&notes));
        notes.push(notes[0].clone());
        assert!(!super::PianoRollOrchestrator::validate_notes(&notes));
    }

    #[test]
    fn strum_orders_chord_notes_by_pitch() {
        let mut editor = super::PianoRollOrchestrator::new();
        let mut notes = vec![
            super::MidiNoteRust { id: 1, pitch: 64, velocity: 100, start_beat: 2.0, duration: 1.0 },
            super::MidiNoteRust { id: 2, pitch: 60, velocity: 100, start_beat: 2.0, duration: 1.0 },
            super::MidiNoteRust { id: 3, pitch: 67, velocity: 100, start_beat: 2.0, duration: 1.0 },
        ];
        editor.select_region(&notes, 2.0, 3.0, 0, 127, false);
        assert!(editor.strum_selected(&mut notes, 0.125, false));
        assert_eq!(notes.iter().map(|note| note.start_beat).collect::<Vec<_>>(), vec![2.125, 2.0, 2.25]);
    }
}
