use std::collections::HashMap;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum Scale {
    Chromatic,
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    Pentatonic,
    MinorPentatonic,
}

const FACTORY_SCALES: [Scale; 12] = [
    Scale::Chromatic,
    Scale::Major,
    Scale::Minor,
    Scale::HarmonicMinor,
    Scale::MelodicMinor,
    Scale::Dorian,
    Scale::Phrygian,
    Scale::Lydian,
    Scale::Mixolydian,
    Scale::Locrian,
    Scale::Pentatonic,
    Scale::MinorPentatonic,
];

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ChordTrackMode {
    Scales,
    Chords,
    ChordsAndScales,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ChordScaleEvent {
    pub tick: u64,
    pub chord_pitch_classes: Vec<u8>,
    pub scale_root: u8,
    pub scale: Scale,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ScaleSuggestion {
    pub root: u8,
    pub scale: Scale,
    pub outside_notes: usize,
}

impl Scale {
    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::Pentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ScaleAssistantEngine {
    pub root: i32,
    pub scale: Scale,
    pub active_notes: Vec<i32>,
    // Repeated Note Ons may overlap, so each channel/note pair owns a stack.
    #[serde(skip)]
    active_note_map: HashMap<(u8, u8), Vec<u8>>,
}

impl Default for ScaleAssistantEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScaleAssistantEngine {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit_scale_assistant() {
            return Err("invalid scale assistant state".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.audit_scale_assistant() {
            Ok(value)
        } else {
            Err("invalid scale assistant state".into())
        }
    }

    pub fn new() -> Self {
        let mut engine = Self {
            root: 0,
            scale: Scale::Major,
            active_notes: Vec::new(),
            active_note_map: HashMap::new(),
        };
        engine.update_active_notes();
        engine
    }

    /// Clear live Note On/Off tracking on transport stop or MIDI panic.
    pub fn reset(&mut self) {
        self.active_note_map.clear();
    }

    pub fn set_root(&mut self, root: i32) {
        self.root = root.rem_euclid(12);
        self.update_active_notes();
    }

    pub fn set_scale(&mut self, scale: Scale) {
        self.scale = scale;
        self.update_active_notes();
    }

    pub fn is_note_in_scale(&self, note: u8) -> bool {
        self.active_notes.contains(&i32::from(note % 12))
    }

    fn update_active_notes(&mut self) {
        self.active_notes = self
            .scale
            .intervals()
            .iter()
            .map(|interval| (self.root + i32::from(*interval)).rem_euclid(12))
            .collect();
        self.active_notes.sort_unstable();
        self.active_notes.dedup();
    }

    /// Find the closest valid absolute pitch, including across octave boundaries.
    /// Equal-distance ties resolve downward for deterministic editing.
    pub fn nearest_note(&self, note: u8) -> u8 {
        if self.is_note_in_scale(note) {
            return note;
        }
        for distance in 1..=127i16 {
            let lower = i16::from(note) - distance;
            if lower >= 0 && self.is_note_in_scale(lower as u8) {
                return lower as u8;
            }
            let upper = i16::from(note) + distance;
            if upper <= 127 && self.is_note_in_scale(upper as u8) {
                return upper as u8;
            }
        }
        note
    }

    /// Quantize existing editor notes and return the number changed.
    pub fn quantize_pitches(&self, notes: &mut [u8]) -> usize {
        let mut changed = 0;
        for note in notes {
            let snapped = self.nearest_note(*note);
            if snapped != *note {
                *note = snapped;
                changed += 1;
            }
        }
        changed
    }

    /// Quantize timestamped notes against changing chord-track harmony.
    /// `ChordsAndScales` accepts the union, preserving explicitly authored
    /// chord tensions while also making every scale degree available.
    pub fn quantize_from_chord_track(
        &self,
        notes: &mut [(u64, u8)],
        events: &[ChordScaleEvent],
        mode: ChordTrackMode,
    ) -> Result<usize, String> {
        if events.is_empty()
            || events.windows(2).any(|pair| pair[0].tick >= pair[1].tick)
            || events.iter().any(|event| !event.validate())
        {
            return Err("invalid chord-track events".into());
        }
        let mut changed = 0;
        for (tick, pitch) in notes {
            let Some(event) = events.iter().rev().find(|event| event.tick <= *tick) else {
                continue;
            };
            let scale_notes = event
                .scale
                .intervals()
                .iter()
                .map(|interval| (event.scale_root + interval) % 12)
                .collect::<Vec<_>>();
            let mut allowed = match mode {
                ChordTrackMode::Scales => scale_notes,
                ChordTrackMode::Chords => event.chord_pitch_classes.clone(),
                ChordTrackMode::ChordsAndScales => {
                    let mut combined = scale_notes;
                    combined.extend_from_slice(&event.chord_pitch_classes);
                    combined
                }
            };
            allowed.sort_unstable();
            allowed.dedup();
            let snapped = nearest_from_pitch_classes(*pitch, &allowed);
            if snapped != *pitch {
                *pitch = snapped;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Rank factory scales against selected editor notes. Exact matches are
    /// returned first, followed by scales with the fewest outside pitches.
    pub fn suggest_scales(notes: &[u8], limit: usize) -> Vec<ScaleSuggestion> {
        if notes.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut candidates = Vec::with_capacity(12 * FACTORY_SCALES.len());
        for root in 0..12u8 {
            for scale in FACTORY_SCALES {
                let outside_notes = notes
                    .iter()
                    .filter(|note| {
                        let relative = (12 + (**note % 12) - root) % 12;
                        !scale.intervals().contains(&relative)
                    })
                    .count();
                candidates.push(ScaleSuggestion {
                    root,
                    scale,
                    outside_notes,
                });
            }
        }
        candidates.sort_by_key(|candidate| {
            (
                candidate.outside_notes,
                candidate.scale == Scale::Chromatic,
                candidate.root,
            )
        });
        candidates.dedup_by_key(|candidate| (candidate.root, candidate.scale));
        candidates.truncate(limit.min(candidates.len()));
        candidates
    }

    /// Snap live Note Ons and map Note Offs to the exact pitch that was emitted.
    pub fn process(&mut self, events: &mut [(u32, Vec<u8>)]) {
        for (_, data) in events.iter_mut() {
            if data.len() < 3 || data[1] > 127 {
                continue;
            }
            let status = data[0] & 0xf0;
            let channel = data[0] & 0x0f;
            let original = data[1];
            let velocity = data[2];
            if status == 0x90 && velocity > 0 {
                let snapped = self.nearest_note(original);
                self.active_note_map
                    .entry((channel, original))
                    .or_default()
                    .push(snapped);
                data[1] = snapped;
            } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                let key = (channel, original);
                let mut remove_entry = false;
                if let Some(stack) = self.active_note_map.get_mut(&key) {
                    if let Some(snapped) = stack.pop() {
                        data[1] = snapped;
                    }
                    remove_entry = stack.is_empty();
                }
                if remove_entry {
                    self.active_note_map.remove(&key);
                }
            }
        }
    }

    pub fn audit_scale_assistant(&self) -> bool {
        (0..12).contains(&self.root)
            && !self.active_notes.is_empty()
            && self
                .active_notes
                .windows(2)
                .all(|notes| notes[0] < notes[1])
            && self.active_notes.iter().all(|note| (0..12).contains(note))
            && self
                .active_note_map
                .iter()
                .all(|(&(channel, note), snapped)| {
                    channel < 16
                        && note < 128
                        && !snapped.is_empty()
                        && snapped.iter().all(|pitch| self.is_note_in_scale(*pitch))
                })
    }
}

impl ChordScaleEvent {
    fn validate(&self) -> bool {
        self.scale_root < 12
            && !self.chord_pitch_classes.is_empty()
            && self.chord_pitch_classes.len() <= 12
            && self.chord_pitch_classes.iter().all(|note| *note < 12)
            && self
                .chord_pitch_classes
                .windows(2)
                .all(|pair| pair[0] < pair[1])
    }
}

fn nearest_from_pitch_classes(note: u8, allowed: &[u8]) -> u8 {
    if allowed.contains(&(note % 12)) {
        return note;
    }
    for distance in 1..=127i16 {
        let lower = i16::from(note) - distance;
        if lower >= 0 && allowed.contains(&((lower as u8) % 12)) {
            return lower as u8;
        }
        let upper = i16::from(note) + distance;
        if upper <= 127 && allowed.contains(&((upper as u8) % 12)) {
            return upper as u8;
        }
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_negative_root_and_crosses_octaves() {
        let mut engine = ScaleAssistantEngine::new();
        engine.set_root(-1);
        assert_eq!(engine.root, 11);
        assert_eq!(engine.nearest_note(60), 59); // equal-distance tie resolves down
        engine.set_root(0);
        engine.set_scale(Scale::Pentatonic);
        assert_eq!(engine.nearest_note(59), 60);
        assert!(engine.audit_scale_assistant());
    }

    #[test]
    fn quantizes_existing_editor_notes() {
        let engine = ScaleAssistantEngine::new();
        let mut notes = [60, 61, 63, 71];
        assert_eq!(engine.quantize_pitches(&mut notes), 2);
        assert_eq!(notes, [60, 60, 62, 71]);
    }

    #[test]
    fn tracks_overlapping_notes_per_midi_channel() {
        let mut engine = ScaleAssistantEngine::new();
        let mut events = vec![
            (0, vec![0x90, 61, 100]),
            (1, vec![0x91, 61, 100]),
            (2, vec![0x90, 61, 100]),
            (3, vec![0x80, 61, 0]),
            (4, vec![0x81, 61, 0]),
            (5, vec![0x80, 61, 0]),
        ];
        engine.process(&mut events);
        assert!(events.iter().all(|event| event.1[1] == 60));
        assert!(engine.audit_scale_assistant());
    }

    #[test]
    fn scale_change_does_not_create_hanging_note() {
        let mut engine = ScaleAssistantEngine::new();
        let mut note_on = vec![(0, vec![0x90, 61, 100])];
        engine.process(&mut note_on);
        engine.set_root(1);
        let mut note_off = vec![(10, vec![0x80, 61, 0])];
        engine.process(&mut note_off);
        assert_eq!(note_on[0].1[1], note_off[0].1[1]);
    }

    #[test]
    fn chord_track_modes_follow_harmony_changes_over_time() {
        let engine = ScaleAssistantEngine::new();
        let events = vec![
            ChordScaleEvent {
                tick: 0,
                chord_pitch_classes: vec![0, 4, 7],
                scale_root: 0,
                scale: Scale::Major,
            },
            ChordScaleEvent {
                tick: 480,
                chord_pitch_classes: vec![2, 5, 9],
                scale_root: 0,
                scale: Scale::Major,
            },
        ];
        let mut notes = [(0, 61), (240, 65), (480, 64), (960, 71)];
        assert_eq!(
            engine
                .quantize_from_chord_track(&mut notes, &events, ChordTrackMode::Chords)
                .unwrap(),
            4
        );
        assert_eq!(notes, [(0, 60), (240, 64), (480, 65), (960, 69)]);
    }

    #[test]
    fn chords_and_scales_preserves_authored_chord_tensions() {
        let engine = ScaleAssistantEngine::new();
        let events = [ChordScaleEvent {
            tick: 0,
            chord_pitch_classes: vec![0, 3, 7],
            scale_root: 0,
            scale: Scale::Major,
        }];
        let mut notes = [(0, 63)];
        assert_eq!(
            engine
                .quantize_from_chord_track(&mut notes, &events, ChordTrackMode::ChordsAndScales)
                .unwrap(),
            0
        );
        assert_eq!(notes[0].1, 63);
    }

    #[test]
    fn scale_suggestions_rank_matching_non_chromatic_scales_first() {
        let suggestions = ScaleAssistantEngine::suggest_scales(&[60, 62, 64, 65, 67, 69, 71], 5);
        assert_eq!(
            suggestions[0],
            ScaleSuggestion {
                root: 0,
                scale: Scale::Major,
                outside_notes: 0
            }
        );
        assert!(suggestions
            .iter()
            .all(|suggestion| suggestion.outside_notes == 0));
    }
}
