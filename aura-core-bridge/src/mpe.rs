use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MpeCurvePoint { pub time: f64, pub value: f32 }

impl MpeCurvePoint { pub fn valid(&self) -> bool { self.time.is_finite() && self.time >= 0.0 && self.value.is_finite() && (-1.0..=1.0).contains(&self.value) } }

pub fn validate_mpe_curve(points: &[MpeCurvePoint]) -> bool { points.len() <= 16_384 && points.iter().all(MpeCurvePoint::valid) && points.windows(2).all(|w| w[0].time < w[1].time) }

pub fn evaluate_mpe_curve(points: &[MpeCurvePoint], time: f64) -> Option<f32> {
    if !validate_mpe_curve(points) || !time.is_finite() || points.is_empty() { return None; }
    if time <= points[0].time { return Some(points[0].value); }
    let last = points.last()?;
    if time >= last.time { return Some(last.value); }
    let upper = points.partition_point(|point| point.time < time);
    let a = &points[upper - 1]; let b = &points[upper];
    let t = ((time - a.time) / (b.time - a.time)) as f32;
    Some(a.value + (b.value - a.value) * t)
}

pub fn upsert_mpe_point(points: &mut Vec<MpeCurvePoint>, point: MpeCurvePoint) -> bool {
    if !point.valid() || points.len() > 16_384 { return false; }
    match points.binary_search_by(|existing| existing.time.total_cmp(&point.time)) {
        Ok(index) => points[index] = point,
        Err(index) => { if points.len() >= 16_384 { return false; } points.insert(index, point); }
    }
    true
}

pub fn remove_mpe_point(points: &mut Vec<MpeCurvePoint>, time: f64) -> bool {
    if !time.is_finite() { return false; }
    let Ok(index) = points.binary_search_by(|existing| existing.time.total_cmp(&time)) else { return false; };
    points.remove(index); true
}

pub struct MPEVoice {
    pub note: u8,
    pub active: bool,
    pub pressure: f32,
    pub timbre: f32,
    pub pitch_bend: f32, // -1.0 to 1.0
    pub last_used: u64,  // Timestamp/counter for LRU voice stealing
}

pub struct MPEOrchestrator {
    pub voices: Vec<MPEVoice>,
    pub note_to_channel: HashMap<u8, u8>,
    pub allocation_counter: u64,
}

impl Default for MPEOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MPEOrchestrator {
    pub fn new() -> Self {
        let mut voices = Vec::with_capacity(16);
        for _ in 0..16 {
            voices.push(MPEVoice {
                note: 0,
                active: false,
                pressure: 0.0,
                timbre: 0.0,
                pitch_bend: 0.0,
                last_used: 0,
            });
        }
        Self {
            voices,
            note_to_channel: HashMap::new(),
            allocation_counter: 0,
        }
    }

    /**
     * @brief ALLOCATE: High-performance MPE voice allocation with LRU stealing.
     * INDUSTRIAL: If all member channels (2-16) are occupied, steals the oldest
     * active voice to prevent note silencing, maintaining absolute project playability.
     */
    pub fn allocate_voice(&mut self, note: u8) -> u8 {
        self.allocation_counter += 1;

        // Retriggering an already-active note must not leave its previous
        // member channel orphaned when the note->channel map is replaced.
        if let Some(previous_channel) = self.note_to_channel.remove(&note) {
            if (2..=16).contains(&previous_channel) {
                let previous = &mut self.voices[(previous_channel - 1) as usize];
                previous.active = false;
                previous.pressure = 0.0;
                previous.timbre = 0.0;
                previous.pitch_bend = 0.0;
            }
        }

        // 1. Try to find a free voice
        for i in 1..16 {
            // Channels 2-16 (Member Channels)
            if !self.voices[i].active {
                self.voices[i].active = true;
                self.voices[i].note = note;
                self.voices[i].last_used = self.allocation_counter;
                self.note_to_channel.insert(note, (i + 1) as u8);
                return (i + 1) as u8;
            }
        }

        // 2. STOLEN: LRU Voice Stealing
        let mut oldest_idx = 1;
        let mut oldest_time = u64::MAX;
        for i in 1..16 {
            if self.voices[i].last_used < oldest_time {
                oldest_time = self.voices[i].last_used;
                oldest_idx = i;
            }
        }

        // Steal the voice
        let stolen_note = self.voices[oldest_idx].note;
        self.note_to_channel.remove(&stolen_note);

        self.voices[oldest_idx].note = note;
        self.voices[oldest_idx].last_used = self.allocation_counter;
        self.voices[oldest_idx].active = true;
        self.note_to_channel.insert(note, (oldest_idx + 1) as u8);

        (oldest_idx + 1) as u8
    }

    /// INDUSTRIAL: Releases an MPE voice with absolute precision.
    pub fn release_voice(&mut self, note: u8) {
        if let Some(channel) = self.note_to_channel.remove(&note) {
            let idx = (channel - 1) as usize;
            self.voices[idx].active = false;
            self.voices[idx].pressure = 0.0;
            self.voices[idx].timbre = 0.0;
            self.voices[idx].pitch_bend = 0.0;
        }
    }

    pub fn voice_for_note(&self, note: u8) -> Option<u8> { self.note_to_channel.get(&note).copied().filter(|channel| (2..=16).contains(channel) && self.voices[(*channel - 1) as usize].active) }

    pub fn active_notes(&self) -> Vec<u8> { let mut notes: Vec<_> = self.note_to_channel.keys().copied().collect(); notes.sort_unstable(); notes }

    pub fn release_channel(&mut self, channel: u8) -> bool {
        if !(2..=16).contains(&channel) { return false; }
        let index = (channel - 1) as usize;
        if !self.voices[index].active { return false; }
        let note = self.voices[index].note;
        self.note_to_channel.remove(&note);
        self.voices[index].active = false;
        self.voices[index].pressure = 0.0;
        self.voices[index].timbre = 0.0;
        self.voices[index].pitch_bend = 0.0;
        true
    }

    /// INDUSTRIAL: Updates an MPE voice with forensic parameter mapping.
    pub fn update_voice(&mut self, channel: u8, pressure: f32, timbre: f32, bend: f32) {
        if channel > 1 && channel <= 16 {
            let idx = (channel - 1) as usize;
            let v = &mut self.voices[idx];
            if v.active {
                v.pressure = if pressure.is_finite() {
                    pressure.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                v.timbre = if timbre.is_finite() {
                    timbre.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                v.pitch_bend = if bend.is_finite() {
                    bend.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                v.last_used = self.allocation_counter; // Keep warm
            }
        }
    }

    /**
     * @brief PITCH: Translates MPE pitch bend values into an absolute frequency multiplier.
     * INDUSTRIAL: Uses equal-temperament scaling based on the pitch bend range (default 48 semitones).
     */
    pub fn calculate_frequency_multiplier(&self, note: u8, bend_range_semitones: f32) -> f64 {
        if let Some(&channel) = self.note_to_channel.get(&note) {
            let idx = (channel - 1) as usize;
            let voice = &self.voices[idx];
            if voice.active {
                if !bend_range_semitones.is_finite() {
                    return 1.0;
                }
                let semitone_shift = voice.pitch_bend * bend_range_semitones.clamp(-96.0, 96.0);
                return 2.0f64.powf((semitone_shift as f64) / 12.0);
            }
        }
        1.0
    }

    pub fn audit_mpe(&self) -> bool {
        self.voices.len() == 16
            && self.voices.iter().all(|voice| {
                voice.pressure.is_finite()
                    && voice.timbre.is_finite()
                    && voice.pitch_bend.is_finite()
                    && (0.0..=1.0).contains(&voice.pressure)
                    && (0.0..=1.0).contains(&voice.timbre)
                    && (-1.0..=1.0).contains(&voice.pitch_bend)
            })
            && self.note_to_channel.len() <= 15
            && self.note_to_channel.iter().all(|(note, channel)| {
                *channel > 1
                    && *channel <= 16
                    && self.voices[(*channel - 1) as usize].active
                    && self.voices[(*channel - 1) as usize].note == *note
            })
            && self.voices.iter().enumerate().skip(1).all(|(index, voice)| {
                !voice.active || self.note_to_channel.get(&voice.note) == Some(&((index + 1) as u8))
            })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteExpressionParameter { Pitch, Pressure, Timbre, Controller(u8) }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteExpressionPoint { pub offset_samples: u64, pub value: f32 }

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
pub struct MidiControllerPoint { pub sample: u64, pub controller: u8, pub value: u8 }

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
pub struct IncomingExpressionEvent { pub sample: u64, pub channel: u8, pub kind: IncomingExpressionKind }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiExpressionConversionResult {
    pub converted_points: usize,
    pub remaining: Vec<IncomingExpressionEvent>,
}

impl NoteExpressionMidiSetup {
    pub fn cubase_default() -> Self {
        Self { enabled_controllers: vec![74], pitchbend: true, aftertouch: true,
            poly_pressure: true, controller_catch_samples: 0 }
    }

    pub fn validate(&self) -> bool {
        self.enabled_controllers.len() <= 128 && self.enabled_controllers.iter().all(|controller| *controller < 128)
            && self.enabled_controllers.windows(2).all(|pair| pair[0] < pair[1])
            && self.controller_catch_samples <= 10_000_000
    }
}

impl NoteExpressionNote {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() { return Err("invalid note expression".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.validate() { Ok(value) } else { Err("invalid note expression".into()) }
    }

    pub fn validate(&self) -> bool {
        self.id != 0 && self.pitch < 128 && self.length_samples > 0 && self.curves.len() <= 256
            && self.curves.iter().all(|(parameter, points)| {
                parameter_valid(*parameter) && points.len() <= 1_000_000
                    && points.iter().all(|point| point.value.is_finite() && (-1.0..=1.0).contains(&point.value)
                        && point.offset_samples <= self.total_expression_length())
                    && points.windows(2).all(|pair| pair[0].offset_samples < pair[1].offset_samples)
            })
    }

    pub fn total_expression_length(&self) -> u64 { self.length_samples.saturating_add(self.release_samples) }

    pub fn set_release_length(&mut self, samples: u64) -> bool {
        if self.length_samples.checked_add(samples).is_none() { return false; }
        self.release_samples = samples;
        self.trim_to_expression_length();
        true
    }

    pub fn upsert_point(&mut self, parameter: NoteExpressionParameter, point: NoteExpressionPoint) -> bool {
        if !parameter_valid(parameter) || !point.value.is_finite() || !(-1.0..=1.0).contains(&point.value)
            || point.offset_samples > self.total_expression_length() { return false; }
        let curve = self.curves.entry(parameter).or_default();
        match curve.binary_search_by_key(&point.offset_samples, |existing| existing.offset_samples) {
            Ok(index) => curve[index] = point,
            Err(index) if curve.len() < 1_000_000 => curve.insert(index, point),
            Err(_) => return false,
        }
        true
    }

    pub fn overdub(&mut self, parameter: NoteExpressionParameter, points: &[NoteExpressionPoint]) -> bool {
        let mut candidate = self.clone();
        if !points.iter().copied().all(|point| candidate.upsert_point(parameter, point)) { return false; }
        *self = candidate; true
    }

    pub fn evaluate(&self, parameter: NoteExpressionParameter, offset_samples: u64) -> Option<f32> {
        let curve = self.curves.get(&parameter)?;
        let first = *curve.first()?;
        if offset_samples <= first.offset_samples { return Some(first.value); }
        let last = *curve.last()?;
        if offset_samples >= last.offset_samples { return Some(last.value); }
        let upper = curve.partition_point(|point| point.offset_samples < offset_samples);
        let left = curve[upper - 1]; let right = curve[upper];
        let ratio = (offset_samples - left.offset_samples) as f64 / (right.offset_samples - left.offset_samples) as f64;
        Some((f64::from(left.value) + f64::from(right.value - left.value) * ratio) as f32)
    }

    pub fn trim_to_note_length(&mut self) {
        self.release_samples = 0;
        self.trim_to_expression_length();
    }

    pub fn trim_to_expression_length(&mut self) {
        let end = self.total_expression_length();
        self.curves.retain(|_, points| { points.retain(|point| point.offset_samples <= end); !points.is_empty() });
    }

    /// Paste a curve to another parameter and scale its timing to this note's
    /// complete note+release duration.
    pub fn paste_scaled(&mut self, source: &NoteExpressionNote, source_parameter: NoteExpressionParameter,
        target_parameter: NoteExpressionParameter) -> bool {
        let Some(points) = source.curves.get(&source_parameter) else { return false; };
        let source_length = source.total_expression_length();
        if source_length == 0 || !parameter_valid(target_parameter) { return false; }
        let target_length = self.total_expression_length();
        let mut pasted = Vec::with_capacity(points.len());
        for point in points {
            let offset = ((point.offset_samples as u128 * target_length as u128) / source_length as u128)
                .min(u64::MAX as u128) as u64;
            pasted.push(NoteExpressionPoint { offset_samples: offset, value: point.value });
        }
        pasted.sort_by_key(|point| point.offset_samples);
        pasted.dedup_by_key(|point| point.offset_samples);
        self.curves.insert(target_parameter, pasted);
        self.validate()
    }

    /// Repeat a selected curve section. Existing destination points in the
    /// repeated ranges are replaced atomically.
    pub fn repeat_section(&mut self, parameter: NoteExpressionParameter, start: u64, end: u64,
        repetitions: u32) -> bool {
        if start >= end || repetitions == 0 || repetitions > 1024 { return false; }
        let Some(source_curve) = self.curves.get(&parameter) else { return false; };
        let selected: Vec<_> = source_curve.iter().copied()
            .filter(|point| (start..end).contains(&point.offset_samples)).collect();
        if selected.is_empty() { return false; }
        let width = end - start;
        let Some(destination_end) = width.checked_mul(u64::from(repetitions)).and_then(|span| end.checked_add(span)) else { return false; };
        if destination_end > self.total_expression_length() { return false; }
        let mut candidate = source_curve.clone();
        candidate.retain(|point| point.offset_samples < end || point.offset_samples >= destination_end);
        for repetition in 1..=repetitions {
            let shift = width * u64::from(repetition);
            candidate.extend(selected.iter().map(|point| NoteExpressionPoint {
                offset_samples: point.offset_samples + shift, value: point.value,
            }));
        }
        candidate.sort_by_key(|point| point.offset_samples);
        candidate.dedup_by_key(|point| point.offset_samples);
        self.curves.insert(parameter, candidate);
        self.validate()
    }

    pub fn clear_expression(&mut self, parameter: Option<NoteExpressionParameter>) {
        if let Some(parameter) = parameter { self.curves.remove(&parameter); } else { self.curves.clear(); }
    }
}

/// Convert controller-lane events into every note sounding at that sample.
/// If an event immediately follows a note, its release phase is extended up to
/// `maximum_release_samples`, preserving controller tails instead of dropping them.
pub fn convert_controllers_to_note_expression(notes: &mut [NoteExpressionNote], events: &[MidiControllerPoint],
    mappings: &std::collections::BTreeMap<u8, NoteExpressionParameter>, maximum_release_samples: u64)
    -> ControllerConversionResult {
    let mut candidates = notes.to_vec();
    let mut result = ControllerConversionResult::default();
    for event in events {
        let Some(&parameter) = mappings.get(&event.controller).filter(|parameter| parameter_valid(**parameter)) else {
            result.remaining.push(*event); continue;
        };
        let mut targets: Vec<usize> = candidates.iter().enumerate().filter(|(_, note)| {
            event.sample >= note.start_sample && event.sample <= note.start_sample.saturating_add(note.total_expression_length())
        }).map(|(index, _)| index).collect();
        if targets.is_empty() {
            let latest_end = candidates.iter().filter_map(|note| {
                let end = note.start_sample.saturating_add(note.length_samples);
                (end <= event.sample && event.sample - end <= maximum_release_samples).then_some(end)
            }).max();
            if let Some(end) = latest_end {
                targets = candidates.iter().enumerate().filter(|(_, note)| note.start_sample.saturating_add(note.length_samples) == end)
                    .map(|(index, _)| index).collect();
                for &index in &targets { candidates[index].release_samples = event.sample - end; }
            }
        }
        if targets.is_empty() { result.remaining.push(*event); continue; }
        let normalized = f32::from(event.value) / 63.5 - 1.0;
        let mut converted = true;
        for index in targets {
            let offset = event.sample - candidates[index].start_sample;
            converted &= candidates[index].upsert_point(parameter, NoteExpressionPoint { offset_samples: offset, value: normalized.clamp(-1.0, 1.0) });
        }
        if converted { result.converted_points += 1; } else { result.remaining.push(*event); }
    }
    if candidates.iter().all(NoteExpressionNote::validate) { notes.clone_from_slice(&candidates); }
    else { result.remaining = events.to_vec(); result.converted_points = 0; }
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
    if !setup.validate() || notes.iter().any(|note| !note.validate())
        || note_channels.iter().any(|(id, channel)| *channel > 15 || !notes.iter().any(|note| note.id == *id)) {
        return MidiExpressionConversionResult { converted_points: 0, remaining: events.to_vec() };
    }
    let mut candidate = notes.to_vec();
    let mut result = MidiExpressionConversionResult::default();
    for event in events {
        if event.channel > 15 { result.remaining.push(*event); continue; }
        let Some((parameter, value, poly_pitch)) = setup.map_event(event.kind) else {
            result.remaining.push(*event); continue;
        };
        let mut targets = candidate.iter().enumerate().filter(|(_, note)| {
            note_channels.get(&note.id) == Some(&event.channel)
                && poly_pitch.is_none_or(|pitch| pitch == note.pitch)
                && ((event.sample >= note.start_sample
                    && event.sample <= note.start_sample.saturating_add(note.total_expression_length()))
                    || (event.sample < note.start_sample && note.start_sample - event.sample <= setup.controller_catch_samples))
        }).map(|(index, _)| index).collect::<Vec<_>>();
        if targets.iter().all(|index| event.sample < candidate[*index].start_sample) {
            if let Some(nearest_start) = targets.iter().map(|index| candidate[*index].start_sample).min() {
                targets.retain(|index| candidate[*index].start_sample == nearest_start);
            }
        }
        if targets.is_empty() { result.remaining.push(*event); continue; }
        let mut converted = true;
        for index in targets {
            let offset = event.sample.saturating_sub(candidate[index].start_sample);
            converted &= candidate[index].upsert_point(parameter, NoteExpressionPoint { offset_samples: offset, value });
        }
        if converted { result.converted_points += 1; } else { result.remaining.push(*event); }
    }
    if candidate.iter().all(NoteExpressionNote::validate) { notes.clone_from_slice(&candidate); }
    else { return MidiExpressionConversionResult { converted_points: 0, remaining: events.to_vec() }; }
    result
}

impl NoteExpressionMidiSetup {
    fn map_event(&self, event: IncomingExpressionKind) -> Option<(NoteExpressionParameter, f32, Option<u8>)> {
        match event {
            IncomingExpressionKind::ControlChange { controller, value }
                if value < 128 && self.enabled_controllers.binary_search(&controller).is_ok() =>
                Some((NoteExpressionParameter::Controller(controller), normalize_7bit(value), None)),
            IncomingExpressionKind::PitchBend { value } if self.pitchbend && (-8192..=8191).contains(&value) =>
                Some((NoteExpressionParameter::Pitch, (f32::from(value) / 8192.0).clamp(-1.0, 1.0), None)),
            IncomingExpressionKind::Aftertouch { value } if self.aftertouch && value < 128 =>
                Some((NoteExpressionParameter::Pressure, normalize_7bit(value), None)),
            IncomingExpressionKind::PolyPressure { note, value } if self.poly_pressure && note < 128 && value < 128 =>
                Some((NoteExpressionParameter::Pressure, normalize_7bit(value), Some(note))),
            _ => None,
        }
    }
}

fn normalize_7bit(value: u8) -> f32 { (f32::from(value) / 63.5 - 1.0).clamp(-1.0, 1.0) }

fn parameter_valid(parameter: NoteExpressionParameter) -> bool {
    match parameter { NoteExpressionParameter::Controller(controller) => controller < 128, _ => true }
}

#[cfg(test)]
#[path = "mpe_tests.rs"]
mod mpe_tests;
