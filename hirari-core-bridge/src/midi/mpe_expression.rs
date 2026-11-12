//! Note Expression model + MIDI/Controller conversion, split out of mpe.rs.
use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteExpressionParameter {
    Pitch,
    Pressure,
    Timbre,
    Controller(u8),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteExpressionPoint {
    pub offset_samples: u64,
    pub value: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteExpressionNote {
    pub id: u64,
    pub pitch: u8,
    pub start_sample: u64,
    pub length_samples: u64,
    pub release_samples: u64,
    pub curves: std::collections::BTreeMap<NoteExpressionParameter, Vec<NoteExpressionPoint>>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiControllerPoint {
    pub sample: u64,
    pub controller: u8,
    pub value: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControllerConversionResult {
    pub converted_points: usize,
    pub remaining: Vec<MidiControllerPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteExpressionMidiSetup {
    pub enabled_controllers: Vec<u8>,
    pub pitchbend: bool,
    pub aftertouch: bool,
    pub poly_pressure: bool,
    pub controller_catch_samples: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IncomingExpressionKind {
    ControlChange { controller: u8, value: u8 },
    PitchBend { value: i16 },
    Aftertouch { value: u8 },
    PolyPressure { note: u8, value: u8 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingExpressionEvent {
    pub sample: u64,
    pub channel: u8,
    pub kind: IncomingExpressionKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiExpressionConversionResult {
    pub converted_points: usize,
    pub remaining: Vec<IncomingExpressionEvent>,
}

impl NoteExpressionMidiSetup {
    pub fn cubase_default() -> Self {
        Self {
            enabled_controllers: vec![74],
            pitchbend: true,
            aftertouch: true,
            poly_pressure: true,
            controller_catch_samples: 0,
        }
    }

    pub fn validate(&self) -> bool {
        self.enabled_controllers.len() <= 128
            && self
                .enabled_controllers
                .iter()
                .all(|controller| *controller < 128)
            && self
                .enabled_controllers
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            && self.controller_catch_samples <= 10_000_000
    }
}

impl NoteExpressionNote {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid note expression".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.validate() {
            Ok(value)
        } else {
            Err("invalid note expression".into())
        }
    }

    pub fn validate(&self) -> bool {
        self.id != 0
            && self.pitch < 128
            && self.length_samples > 0
            && self.curves.len() <= 256
            && self.curves.iter().all(|(parameter, points)| {
                parameter_valid(*parameter)
                    && points.len() <= 1_000_000
                    && points.iter().all(|point| {
                        point.value.is_finite()
                            && (-1.0..=1.0).contains(&point.value)
                            && point.offset_samples <= self.total_expression_length()
                    })
                    && points
                        .windows(2)
                        .all(|pair| pair[0].offset_samples < pair[1].offset_samples)
            })
    }

    pub fn total_expression_length(&self) -> u64 {
        self.length_samples.saturating_add(self.release_samples)
    }

    pub fn set_release_length(&mut self, samples: u64) -> bool {
        if self.length_samples.checked_add(samples).is_none() {
            return false;
        }
        self.release_samples = samples;
        self.trim_to_expression_length();
        true
    }

    pub fn upsert_point(
        &mut self,
        parameter: NoteExpressionParameter,
        point: NoteExpressionPoint,
    ) -> bool {
        if !parameter_valid(parameter)
            || !point.value.is_finite()
            || !(-1.0..=1.0).contains(&point.value)
            || point.offset_samples > self.total_expression_length()
        {
            return false;
        }
        let curve = self.curves.entry(parameter).or_default();
        match curve.binary_search_by_key(&point.offset_samples, |existing| existing.offset_samples)
        {
            Ok(index) => curve[index] = point,
            Err(index) if curve.len() < 1_000_000 => curve.insert(index, point),
            Err(_) => return false,
        }
        true
    }

    pub fn overdub(
        &mut self,
        parameter: NoteExpressionParameter,
        points: &[NoteExpressionPoint],
    ) -> bool {
        let mut candidate = self.clone();
        if !points
            .iter()
            .copied()
            .all(|point| candidate.upsert_point(parameter, point))
        {
            return false;
        }
        *self = candidate;
        true
    }

    pub fn evaluate(&self, parameter: NoteExpressionParameter, offset_samples: u64) -> Option<f32> {
        let curve = self.curves.get(&parameter)?;
        let first = *curve.first()?;
        if offset_samples <= first.offset_samples {
            return Some(first.value);
        }
        let last = *curve.last()?;
        if offset_samples >= last.offset_samples {
            return Some(last.value);
        }
        let upper = curve.partition_point(|point| point.offset_samples < offset_samples);
        let left = curve[upper - 1];
        let right = curve[upper];
        let ratio = (offset_samples - left.offset_samples) as f64
            / (right.offset_samples - left.offset_samples) as f64;
        Some((f64::from(left.value) + f64::from(right.value - left.value) * ratio) as f32)
    }

    pub fn trim_to_note_length(&mut self) {
        self.release_samples = 0;
        self.trim_to_expression_length();
    }

    pub fn trim_to_expression_length(&mut self) {
        let end = self.total_expression_length();
        self.curves.retain(|_, points| {
            points.retain(|point| point.offset_samples <= end);
            !points.is_empty()
        });
    }

    /// Paste a curve to another parameter and scale its timing to this note's
    /// complete note+release duration.
    pub fn paste_scaled(
        &mut self,
        source: &NoteExpressionNote,
        source_parameter: NoteExpressionParameter,
        target_parameter: NoteExpressionParameter,
    ) -> bool {
        let Some(points) = source.curves.get(&source_parameter) else {
            return false;
        };
        let source_length = source.total_expression_length();
        if source_length == 0 || !parameter_valid(target_parameter) {
            return false;
        }
        let target_length = self.total_expression_length();
        let mut pasted = Vec::with_capacity(points.len());
        for point in points {
            let offset = ((point.offset_samples as u128 * target_length as u128)
                / source_length as u128)
                .min(u64::MAX as u128) as u64;
            pasted.push(NoteExpressionPoint {
                offset_samples: offset,
                value: point.value,
            });
        }
        pasted.sort_by_key(|point| point.offset_samples);
        pasted.dedup_by_key(|point| point.offset_samples);
        self.curves.insert(target_parameter, pasted);
        self.validate()
    }

    /// Repeat a selected curve section. Existing destination points in the
    /// repeated ranges are replaced atomically.
    pub fn repeat_section(
        &mut self,
        parameter: NoteExpressionParameter,
        start: u64,
        end: u64,
        repetitions: u32,
    ) -> bool {
        if start >= end || repetitions == 0 || repetitions > 1024 {
            return false;
        }
        let Some(source_curve) = self.curves.get(&parameter) else {
            return false;
        };
        let selected: Vec<_> = source_curve
            .iter()
            .copied()
            .filter(|point| (start..end).contains(&point.offset_samples))
            .collect();
        if selected.is_empty() {
            return false;
        }
        let width = end - start;
        let Some(destination_end) = width
            .checked_mul(u64::from(repetitions))
            .and_then(|span| end.checked_add(span))
        else {
            return false;
        };
        if destination_end > self.total_expression_length() {
            return false;
        }
        let mut candidate = source_curve.clone();
        candidate
            .retain(|point| point.offset_samples < end || point.offset_samples >= destination_end);
        for repetition in 1..=repetitions {
            let shift = width * u64::from(repetition);
            candidate.extend(selected.iter().map(|point| NoteExpressionPoint {
                offset_samples: point.offset_samples + shift,
                value: point.value,
            }));
        }
        candidate.sort_by_key(|point| point.offset_samples);
        candidate.dedup_by_key(|point| point.offset_samples);
        self.curves.insert(parameter, candidate);
        self.validate()
    }

    pub fn clear_expression(&mut self, parameter: Option<NoteExpressionParameter>) {
        if let Some(parameter) = parameter {
            self.curves.remove(&parameter);
        } else {
            self.curves.clear();
        }
    }
}

/// Convert controller-lane events into every note sounding at that sample.
/// If an event immediately follows a note, its release phase is extended up to
/// `maximum_release_samples`, preserving controller tails instead of dropping them.
pub fn convert_controllers_to_note_expression(
    notes: &mut [NoteExpressionNote],
    events: &[MidiControllerPoint],
    mappings: &std::collections::BTreeMap<u8, NoteExpressionParameter>,
    maximum_release_samples: u64,
) -> ControllerConversionResult {
    let mut candidates = notes.to_vec();
    let mut result = ControllerConversionResult::default();
    for event in events {
        let Some(&parameter) = mappings
            .get(&event.controller)
            .filter(|parameter| parameter_valid(**parameter))
        else {
            result.remaining.push(*event);
            continue;
        };
        let mut targets: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                event.sample >= note.start_sample
                    && event.sample
                        <= note
                            .start_sample
                            .saturating_add(note.total_expression_length())
            })
            .map(|(index, _)| index)
            .collect();
        if targets.is_empty() {
            let latest_end = candidates
                .iter()
                .filter_map(|note| {
                    let end = note.start_sample.saturating_add(note.length_samples);
                    (end <= event.sample && event.sample - end <= maximum_release_samples)
                        .then_some(end)
                })
                .max();
            if let Some(end) = latest_end {
                targets = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| {
                        note.start_sample.saturating_add(note.length_samples) == end
                    })
                    .map(|(index, _)| index)
                    .collect();
                for &index in &targets {
                    candidates[index].release_samples = event.sample - end;
                }
            }
        }
        if targets.is_empty() {
            result.remaining.push(*event);
            continue;
        }
        let normalized = f32::from(event.value) / 63.5 - 1.0;
        let mut converted = true;
        for index in targets {
            let offset = event.sample - candidates[index].start_sample;
            converted &= candidates[index].upsert_point(
                parameter,
                NoteExpressionPoint {
                    offset_samples: offset,
                    value: normalized.clamp(-1.0, 1.0),
                },
            );
        }
        if converted {
            result.converted_points += 1;
        } else {
            result.remaining.push(*event);
        }
    }
    if candidates.iter().all(NoteExpressionNote::validate) {
        notes.clone_from_slice(&candidates);
    } else {
        result.remaining = events.to_vec();
        result.converted_points = 0;
    }
    result
}

/// Convert enabled MIDI messages into per-note curves. Channel-specific data
/// is associated only with notes on that channel; Poly Pressure additionally
/// requires a matching pitch. Events sent just before Note On are captured
/// within the configured catch range. Disabled/unmatched data is returned for
/// the ordinary controller lane.
pub fn convert_midi_expression_with_setup(
    notes: &mut [NoteExpressionNote],
    note_channels: &std::collections::BTreeMap<u64, u8>,
    events: &[IncomingExpressionEvent],
    setup: &NoteExpressionMidiSetup,
) -> MidiExpressionConversionResult {
    if !setup.validate()
        || notes.iter().any(|note| !note.validate())
        || note_channels
            .iter()
            .any(|(id, channel)| *channel > 15 || !notes.iter().any(|note| note.id == *id))
    {
        return MidiExpressionConversionResult {
            converted_points: 0,
            remaining: events.to_vec(),
        };
    }
    let mut candidate = notes.to_vec();
    let mut result = MidiExpressionConversionResult::default();
    for event in events {
        if event.channel > 15 {
            result.remaining.push(*event);
            continue;
        }
        let Some((parameter, value, poly_pitch)) = setup.map_event(event.kind) else {
            result.remaining.push(*event);
            continue;
        };
        let mut targets = candidate
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                note_channels.get(&note.id) == Some(&event.channel)
                    && poly_pitch.is_none_or(|pitch| pitch == note.pitch)
                    && ((event.sample >= note.start_sample
                        && event.sample
                            <= note
                                .start_sample
                                .saturating_add(note.total_expression_length()))
                        || (event.sample < note.start_sample
                            && note.start_sample - event.sample <= setup.controller_catch_samples))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if targets
            .iter()
            .all(|index| event.sample < candidate[*index].start_sample)
        {
            if let Some(nearest_start) = targets
                .iter()
                .map(|index| candidate[*index].start_sample)
                .min()
            {
                targets.retain(|index| candidate[*index].start_sample == nearest_start);
            }
        }
        if targets.is_empty() {
            result.remaining.push(*event);
            continue;
        }
        let mut converted = true;
        for index in targets {
            let offset = event.sample.saturating_sub(candidate[index].start_sample);
            converted &= candidate[index].upsert_point(
                parameter,
                NoteExpressionPoint {
                    offset_samples: offset,
                    value,
                },
            );
        }
        if converted {
            result.converted_points += 1;
        } else {
            result.remaining.push(*event);
        }
    }
    if candidate.iter().all(NoteExpressionNote::validate) {
        notes.clone_from_slice(&candidate);
    } else {
        return MidiExpressionConversionResult {
            converted_points: 0,
            remaining: events.to_vec(),
        };
    }
    result
}

impl NoteExpressionMidiSetup {
    fn map_event(
        &self,
        event: IncomingExpressionKind,
    ) -> Option<(NoteExpressionParameter, f32, Option<u8>)> {
        match event {
            IncomingExpressionKind::ControlChange { controller, value }
                if value < 128 && self.enabled_controllers.binary_search(&controller).is_ok() =>
            {
                Some((
                    NoteExpressionParameter::Controller(controller),
                    normalize_7bit(value),
                    None,
                ))
            }
            IncomingExpressionKind::PitchBend { value }
                if self.pitchbend && (-8192..=8191).contains(&value) =>
            {
                Some((
                    NoteExpressionParameter::Pitch,
                    (f32::from(value) / 8192.0).clamp(-1.0, 1.0),
                    None,
                ))
            }
            IncomingExpressionKind::Aftertouch { value } if self.aftertouch && value < 128 => {
                Some((
                    NoteExpressionParameter::Pressure,
                    normalize_7bit(value),
                    None,
                ))
            }
            IncomingExpressionKind::PolyPressure { note, value }
                if self.poly_pressure && note < 128 && value < 128 =>
            {
                Some((
                    NoteExpressionParameter::Pressure,
                    normalize_7bit(value),
                    Some(note),
                ))
            }
            _ => None,
        }
    }
}

pub(super) fn normalize_7bit(value: u8) -> f32 {
    (f32::from(value) / 63.5 - 1.0).clamp(-1.0, 1.0)
}

pub(super) fn parameter_valid(parameter: NoteExpressionParameter) -> bool {
    match parameter {
        NoteExpressionParameter::Controller(controller) => controller < 128,
        _ => true,
    }
}

#[cfg(test)]
#[path = "mpe_tests.rs"]
mod mpe_tests;
