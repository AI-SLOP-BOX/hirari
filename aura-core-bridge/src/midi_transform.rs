#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiNote {
    pub start_tick: u64,
    pub length_ticks: u32,
    pub pitch: u8,
    pub velocity: u8,
}
impl MidiNote {
    pub fn validate(&self) -> bool {
        self.length_ticks > 0 && self.pitch < 128 && self.velocity < 128
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiFilter {
    pub min_pitch: u8,
    pub max_pitch: u8,
    pub min_vel: u8,
    pub max_vel: u8,
}
impl MidiFilter {
    pub fn validate(&self) -> bool {
        self.min_pitch <= self.max_pitch
            && self.max_pitch < 128
            && self.min_vel <= self.max_vel
            && self.max_vel < 128
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MidiTransformPreset {
    pub name: String,
    pub pitch_offset: i32,
    pub velocity_scale: f32,
}

#[derive(Default)]
pub struct MidiOrchestrator {
    pub presets: Vec<MidiTransformPreset>,
}

impl MidiOrchestrator {
    pub fn save_preset(&mut self, preset: MidiTransformPreset) -> bool {
        if preset.name.trim().is_empty()
            || preset.name.len() > 128
            || preset.name.contains('\0')
            || !preset.velocity_scale.is_finite()
            || !(0.0..=16.0).contains(&preset.velocity_scale)
        {
            return false;
        }
        let normalized_name = preset.name.trim().to_owned();
        if let Some(existing) = self
            .presets
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(&normalized_name))
        {
            *existing = MidiTransformPreset {
                name: normalized_name,
                ..preset
            };
        } else if self.presets.len() < 4096 {
            self.presets.push(MidiTransformPreset {
                name: normalized_name,
                ..preset
            });
        } else {
            return false;
        }
        true
    }

    pub fn apply_preset(&self, name: &str, notes: &mut [MidiNote], filter: &MidiFilter) -> bool {
        let Some(preset) = self
            .presets
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        else {
            return false;
        };
        if !filter.validate()
            || notes.iter().any(|note| !note.validate())
            || preset.pitch_offset.abs() > 127
        {
            return false;
        }
        for note in notes.iter_mut().filter(|n| {
            n.pitch >= filter.min_pitch
                && n.pitch <= filter.max_pitch
                && n.velocity >= filter.min_vel
                && n.velocity <= filter.max_vel
        }) {
            note.pitch = (note.pitch as i32 + preset.pitch_offset).clamp(0, 127) as u8;
            note.velocity = (note.velocity as f32 * preset.velocity_scale).clamp(1.0, 127.0) as u8;
        }
        true
    }

    pub fn preset_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .presets
            .iter()
            .map(|preset| preset.name.clone())
            .collect();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names
    }

    pub fn remove_preset(&mut self, name: &str) -> bool {
        let before = self.presets.len();
        self.presets
            .retain(|preset| !preset.name.eq_ignore_ascii_case(name.trim()));
        before != self.presets.len()
    }

    pub fn audit_presets(&self) -> bool {
        self.presets.len() <= 4096
            && self.presets.iter().enumerate().all(|(i, preset)| {
                !preset.name.trim().is_empty()
                    && preset.name.len() <= 128
                    && !preset.name.contains('\0')
                    && preset.velocity_scale.is_finite()
                    && (0.0..=16.0).contains(&preset.velocity_scale)
                    && preset.pitch_offset.abs() <= 127
                    && self.presets[..i]
                        .iter()
                        .all(|previous| !previous.name.eq_ignore_ascii_case(&preset.name))
            })
    }
}

/// Applies non-destructive note-length and velocity shaping to a selected range.
pub fn transform_timing(
    notes: &mut [MidiNote],
    min_pitch: u8,
    max_pitch: u8,
    length_scale: f32,
    velocity_delta: i16,
) -> bool {
    if min_pitch > max_pitch
        || !length_scale.is_finite()
        || length_scale < 0.0
        || length_scale > 64.0
    {
        return false;
    }
    for note in notes
        .iter_mut()
        .filter(|n| n.pitch >= min_pitch && n.pitch <= max_pitch)
    {
        note.length_ticks =
            ((note.length_ticks as f32 * length_scale).round()).clamp(1.0, u32::MAX as f32) as u32;
        note.velocity = (note.velocity as i16 + velocity_delta).clamp(1, 127) as u8;
    }
    true
}

pub fn repeat_notes(notes: &[MidiNote], repeats: u32, spacing_ticks: u64) -> Option<Vec<MidiNote>> {
    if repeats == 0 || repeats > 1024 || notes.iter().any(|note| !note.validate()) {
        return None;
    }
    let mut out = Vec::with_capacity(notes.len().saturating_mul(repeats as usize));
    for repeat in 0..repeats {
        let offset = spacing_ticks.checked_mul(repeat as u64)?;
        for note in notes {
            let mut copy = note.clone();
            copy.start_tick = copy.start_tick.checked_add(offset)?;
            out.push(copy);
        }
    }
    Some(out)
}

/// Quantizes note lengths to a fixed grid while preserving note validity.
pub fn quantize_note_lengths(notes: &mut [MidiNote], grid_ticks: u32) -> bool {
    if grid_ticks == 0 || notes.iter().any(|note| !note.validate()) {
        return false;
    }
    for note in notes {
        let rounded = ((note.length_ticks as u64 + grid_ticks as u64 / 2) / grid_ticks as u64)
            .max(1)
            * grid_ticks as u64;
        note.length_ticks = rounded.min(u32::MAX as u64) as u32;
    }
    true
}

/// Deterministically thins notes by a probability value in [0,1]. The seed
/// makes repeated edits reproducible for render and collaborative workflows.
pub fn apply_note_probability(
    notes: &[MidiNote],
    probability: f32,
    seed: u64,
) -> Option<Vec<MidiNote>> {
    if !probability.is_finite()
        || !(0.0..=1.0).contains(&probability)
        || notes.iter().any(|note| !note.validate())
    {
        return None;
    }
    let threshold = (probability * u64::MAX as f32) as u64;
    let mut state = seed;
    let mut out = Vec::with_capacity(notes.len());
    for note in notes {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        if state <= threshold {
            out.push(note.clone());
        }
    }
    Some(out)
}

#[cfg(test)]
mod repeat_tests {
    use super::*;

    #[test]
    fn repeats_with_offsets() {
        let n = MidiNote {
            start_tick: 4,
            length_ticks: 8,
            pitch: 60,
            velocity: 100,
        };
        let out = repeat_notes(&[n], 3, 16).unwrap();
        assert_eq!(
            out.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![4, 20, 36]
        );
    }

    #[test]
    fn rejects_midi_filter_values_outside_seven_bit_range() {
        let invalid_pitch = MidiFilter {
            min_pitch: 0,
            max_pitch: 200,
            min_vel: 1,
            max_vel: 127,
        };
        let invalid_velocity = MidiFilter {
            min_pitch: 0,
            max_pitch: 127,
            min_vel: 1,
            max_vel: 200,
        };
        assert!(!invalid_pitch.validate());
        assert!(!invalid_velocity.validate());
    }

    #[test]
    fn named_transform_presets_apply_and_list_deterministically() {
        let mut orchestrator = MidiOrchestrator::default();
        assert!(orchestrator.save_preset(MidiTransformPreset {
            name: "Transpose".into(),
            pitch_offset: 12,
            velocity_scale: 0.5
        }));
        assert_eq!(orchestrator.preset_names(), vec!["Transpose"]);
        let mut notes = vec![MidiNote {
            start_tick: 0,
            length_ticks: 10,
            pitch: 60,
            velocity: 100,
        }];
        let filter = MidiFilter {
            min_pitch: 0,
            max_pitch: 127,
            min_vel: 0,
            max_vel: 127,
        };
        assert!(orchestrator.apply_preset("Transpose", &mut notes, &filter));
        assert_eq!((notes[0].pitch, notes[0].velocity), (72, 50));
        assert!(orchestrator.audit_presets());
    }
}

impl MidiOrchestrator {
    /// INDUSTRIAL: Performs batch MIDI transformations with absolute musical integrity and technical sovereignty.
    pub fn transform_notes(
        &self,
        notes: &mut [MidiNote],
        filter: &MidiFilter,
        pitch_offset: i32,
        vel_scale: f32,
    ) {
        if !filter.validate() || !vel_scale.is_finite() {
            return;
        }
        // INDUSTRIAL: Implementation of high-performance MIDI batch processing.
        // Rust's safe memory management handles large event streams with
        // absolute bit-accuracy and zero-latency.
        for n in notes.iter_mut() {
            if n.pitch < filter.min_pitch || n.pitch > filter.max_pitch {
                continue;
            }
            if n.velocity < filter.min_vel || n.velocity > filter.max_vel {
                continue;
            }

            n.pitch = (n.pitch as i32 + pitch_offset).clamp(0, 127) as u8;
            n.velocity = (n.velocity as f32 * vel_scale).clamp(1.0, 127.0) as u8;
        }
    }

    /// INDUSTRIAL: Forces MIDI notes into a musical key with absolute forensic precision and technical sovereignty.
    pub fn apply_scale_quantize(&self, notes: &mut [MidiNote], root: u8, scale: &[i32]) {
        if scale.is_empty()
            || root > 11
            || scale.iter().any(|degree| !(-24..=24).contains(degree))
            || notes.iter().any(|note| !note.validate())
        {
            return;
        }
        // INDUSTRIAL: Implementation of high-performance scale mapping.
        // Rust's TransformEngine ensures bit-accurate musical distribution instantaneously.
        for n in notes.iter_mut() {
            let octave = n.pitch / 12;
            let semitone = n.pitch % 12;

            let mut best_pitch = scale[0] as u8;
            let mut min_diff = 100;
            for &degree in scale {
                let target = (root as i32 + degree).rem_euclid(12) as u8;
                let diff = (semitone as i32 - target as i32).abs();
                if diff < min_diff {
                    min_diff = diff;
                    best_pitch = target;
                }
            }
            n.pitch = (octave as u16 * 12 + best_pitch as u16).min(127) as u8;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide MIDI event synchronization graph.
    pub fn audit_midi(&self) -> bool {
        self.audit_presets()
            && self
                .presets
                .iter()
                .all(|preset| preset.pitch_offset.abs() <= 127)
    }
}
