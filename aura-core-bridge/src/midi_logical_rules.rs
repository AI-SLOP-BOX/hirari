#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MidiNotePredicate {
    pub track_id: Option<u32>,
    pub pitch_min: Option<u8>,
    pub pitch_max: Option<u8>,
    pub velocity_min: Option<u8>,
    pub velocity_max: Option<u8>,
    pub start_sample_min: Option<u64>,
    pub start_sample_max: Option<u64>,
    pub lyric_equals: Option<String>,
    pub phoneme_equals: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MidiNoteTransform {
    Transpose { semitones: i16 },
    SetVelocity { velocity: u8 },
    ScaleVelocity { factor: f32 },
    Move { delta_samples: i64 },
    ScaleLength { factor: f64 },
    SetPhoneme { phoneme: String },
    SetProbability { probability: u8 },
    SetRepeatCount { repeat_count: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiLogicalRule {
    pub predicate: MidiNotePredicate,
    pub transforms: Vec<MidiNoteTransform>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiTransformPreset { pub name: String, pub rules: Vec<MidiLogicalRule> }
impl MidiTransformPreset { pub fn validate(&self) -> bool { !self.name.trim().is_empty() && self.name.len() <= 256 && !self.rules.is_empty() && self.rules.len() <= 128 && self.rules.iter().all(|r| r.validate().is_ok()) } pub fn apply(&self, notes: &mut [MidiNoteContract]) -> Result<usize, String> { if !self.validate() { return Err("invalid MIDI transform preset".into()); } let mut count=0; for r in &self.rules { count += apply_rule(notes, r)?; } Ok(count) } }

impl MidiLogicalRule {
    pub fn validate(&self) -> Result<(), String> {
        let ordered = |low: Option<u8>, high: Option<u8>| low.zip(high).is_none_or(|(l, h)| l <= h);
        if !ordered(self.predicate.pitch_min, self.predicate.pitch_max)
            || !ordered(self.predicate.velocity_min, self.predicate.velocity_max)
            || self
                .predicate
                .start_sample_min
                .zip(self.predicate.start_sample_max)
                .is_some_and(|(low, high)| low > high)
        {
            return Err("MIDI logical predicate range is reversed".into());
        }
        if self.transforms.is_empty() {
            return Err("MIDI logical rule has no transforms".into());
        }
        for text in [self.predicate.lyric_equals.as_deref(), self.predicate.phoneme_equals.as_deref()]
            .into_iter()
            .flatten()
        {
            if text.len() > 256 || text.contains('\0') {
                return Err("MIDI logical predicate text is invalid".into());
            }
        }
        for transform in &self.transforms {
            match transform {
                MidiNoteTransform::SetVelocity { velocity } if *velocity == 0 || *velocity > 127 => {
                    return Err("MIDI logical velocity is outside 1..=127".into());
                }
                MidiNoteTransform::ScaleVelocity { factor }
                    if !factor.is_finite() || *factor <= 0.0 || *factor > 16.0 =>
                {
                    return Err("MIDI logical velocity factor is invalid".into());
                }
                MidiNoteTransform::ScaleLength { factor }
                    if !factor.is_finite() || *factor <= 0.0 || *factor > 1024.0 =>
                {
                    return Err("MIDI logical length factor is invalid".into());
                }
                MidiNoteTransform::SetPhoneme { phoneme }
                    if phoneme.len() > 128 || phoneme.contains('\0') =>
                {
                    return Err("MIDI logical phoneme is invalid".into());
                }
                MidiNoteTransform::SetProbability { probability } if *probability > 100 => return Err("MIDI probability must be 0..=100".into()),
                MidiNoteTransform::SetRepeatCount { repeat_count } if *repeat_count == 0 => return Err("MIDI repeat count must be at least one".into()),
                _ => {}
            }
        }
        Ok(())
    }
}

pub fn apply_rule(notes: &mut [MidiNoteContract], rule: &MidiLogicalRule) -> Result<usize, String> {
    rule.validate()?;
    let mut changed = 0;
    // Work on a shadow copy so a failing transform cannot leave a partially
    // applied rule across a selection.
    let mut shadow = notes.to_vec();
    for note in shadow.iter_mut().filter(|note| matches(note, &rule.predicate)) {
        let mut candidate = note.clone();
        for transform in &rule.transforms {
            apply_transform(&mut candidate, transform)?;
        }
        candidate.validate().map_err(|error| error.to_string())?;
        if candidate != *note {
            *note = candidate;
            changed += 1;
        }
    }
    notes.clone_from_slice(&shadow);
    Ok(changed)
}

/// Expands a selected note set into deterministic repeated phrases.
pub fn repeat_notes(notes: &[MidiNoteContract], repetitions: u32, spacing_samples: u64) -> Result<Vec<MidiNoteContract>, String> {
    if repetitions == 0 || repetitions > 1024 { return Err("MIDI repeat count is outside 1..=1024".into()); }
    let mut output = Vec::with_capacity(notes.len().saturating_mul(repetitions as usize));
    for note in notes {
        for index in 0..repetitions {
            let mut copy = note.clone();
            let offset = spacing_samples.checked_mul(index as u64).ok_or_else(|| "MIDI repeat offset overflow".to_owned())?;
            copy.start_sample = copy.start_sample.checked_add(offset).ok_or_else(|| "MIDI repeat exceeds timeline".to_owned())?;
            output.push(copy);
        }
    }
    output.sort_by_key(|note| note.start_sample);
    Ok(output)
}

fn matches(note: &MidiNoteContract, predicate: &MidiNotePredicate) -> bool {
    predicate.track_id.is_none_or(|value| note.track_id == value)
        && predicate.pitch_min.is_none_or(|value| note.pitch >= value)
        && predicate.pitch_max.is_none_or(|value| note.pitch <= value)
        && predicate.velocity_min.is_none_or(|value| note.velocity >= value)
        && predicate.velocity_max.is_none_or(|value| note.velocity <= value)
        && predicate.start_sample_min.is_none_or(|value| note.start_sample >= value)
        && predicate.start_sample_max.is_none_or(|value| note.start_sample <= value)
        && predicate.lyric_equals.as_ref().is_none_or(|value| note.lyric == *value)
        && predicate.phoneme_equals.as_ref().is_none_or(|value| note.phoneme == *value)
}

fn apply_transform(note: &mut MidiNoteContract, transform: &MidiNoteTransform) -> Result<(), String> {
    match transform {
        MidiNoteTransform::Transpose { semitones } => {
            note.pitch = i16::from(note.pitch)
                .checked_add(*semitones)
                .filter(|pitch| (0..=127).contains(pitch))
                .ok_or_else(|| "MIDI logical transpose exceeds the pitch range".to_owned())?
                as u8;
        }
        MidiNoteTransform::SetVelocity { velocity } => note.velocity = *velocity,
        MidiNoteTransform::ScaleVelocity { factor } => {
            note.velocity = (f32::from(note.velocity) * factor).round().clamp(1.0, 127.0) as u8;
        }
        MidiNoteTransform::Move { delta_samples } => {
            note.start_sample = if *delta_samples < 0 {
                note.start_sample
                    .checked_sub(delta_samples.unsigned_abs())
                    .ok_or_else(|| "MIDI logical move precedes the project start".to_owned())?
            } else {
                note.start_sample
                    .checked_add(*delta_samples as u64)
                    .ok_or_else(|| "MIDI logical move overflows the timeline".to_owned())?
            };
        }
        MidiNoteTransform::ScaleLength { factor } => {
            let scaled = note.length_samples as f64 * factor;
            if !scaled.is_finite() || scaled < 1.0 || scaled > u64::MAX as f64 {
                return Err("MIDI logical note length is outside the timeline".into());
            }
            note.length_samples = scaled.round() as u64;
        }
        MidiNoteTransform::SetPhoneme { phoneme } => note.phoneme.clone_from(phoneme),
        MidiNoteTransform::SetProbability { probability } => note.probability = *probability,
        MidiNoteTransform::SetRepeatCount { repeat_count } => note.repeat_count = *repeat_count,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(track_id: u32, pitch: u8, velocity: u8, start_sample: u64) -> MidiNoteContract {
        MidiNoteContract {
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples: 480,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        }
    }

    #[test]
    fn filters_and_applies_ordered_transform_chain() {
        let mut notes = vec![note(1, 60, 40, 0), note(1, 72, 100, 480), note(2, 64, 50, 0)];
        let rule = MidiLogicalRule {
            predicate: MidiNotePredicate {
                track_id: Some(1),
                pitch_max: Some(64),
                velocity_max: Some(64),
                ..Default::default()
            },
            transforms: vec![
                MidiNoteTransform::Transpose { semitones: 12 },
                MidiNoteTransform::ScaleVelocity { factor: 2.0 },
                MidiNoteTransform::Move { delta_samples: 240 },
            ],
        };
        assert_eq!(apply_rule(&mut notes, &rule).unwrap(), 1);
        assert_eq!((notes[0].pitch, notes[0].velocity, notes[0].start_sample), (72, 80, 240));
        assert_eq!((notes[1].pitch, notes[1].velocity), (72, 100));
        assert_eq!((notes[2].pitch, notes[2].velocity), (64, 50));
    }

    #[test]
    fn repeats_notes_with_bounded_offsets() {
        let source = vec![note(1, 60, 90, 10)];
        let repeated = repeat_notes(&source, 3, 480).unwrap();
        assert_eq!(repeated.iter().map(|n| n.start_sample).collect::<Vec<_>>(), vec![10, 490, 970]);
        assert!(repeat_notes(&source, 0, 1).is_err());
    }

    #[test]
    fn rejects_invalid_rules_without_mutating_notes() {
        let mut notes = vec![note(1, 124, 100, 0)];
        let original = notes.clone();
        let rule = MidiLogicalRule {
            predicate: MidiNotePredicate::default(),
            transforms: vec![MidiNoteTransform::Transpose { semitones: 12 }],
        };
        assert!(apply_rule(&mut notes, &rule).is_err());
        assert_eq!(notes, original);
    }

    #[test]
    fn rule_application_is_atomic_across_selection() {
        let mut notes = vec![note(1, 60, 100, 0), note(1, 124, 100, 480)];
        let original = notes.clone();
        let rule = MidiLogicalRule {
            predicate: MidiNotePredicate::default(),
            transforms: vec![MidiNoteTransform::Transpose { semitones: 12 }],
        };
        assert!(apply_rule(&mut notes, &rule).is_err());
        assert_eq!(notes, original);
    }
}
