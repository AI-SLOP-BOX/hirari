use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MpeCurvePoint {
    pub time: f64,
    pub value: f32,
}

impl MpeCurvePoint {
    pub fn valid(&self) -> bool {
        self.time.is_finite()
            && self.time >= 0.0
            && self.value.is_finite()
            && (-1.0..=1.0).contains(&self.value)
    }
}

pub fn validate_mpe_curve(points: &[MpeCurvePoint]) -> bool {
    points.len() <= 16_384
        && points.iter().all(MpeCurvePoint::valid)
        && points.windows(2).all(|w| w[0].time < w[1].time)
}

pub fn evaluate_mpe_curve(points: &[MpeCurvePoint], time: f64) -> Option<f32> {
    if !validate_mpe_curve(points) || !time.is_finite() || points.is_empty() {
        return None;
    }
    if time <= points[0].time {
        return Some(points[0].value);
    }
    let last = points.last()?;
    if time >= last.time {
        return Some(last.value);
    }
    let upper = points.partition_point(|point| point.time < time);
    let a = &points[upper - 1];
    let b = &points[upper];
    let t = ((time - a.time) / (b.time - a.time)) as f32;
    Some(a.value + (b.value - a.value) * t)
}

pub fn upsert_mpe_point(points: &mut Vec<MpeCurvePoint>, point: MpeCurvePoint) -> bool {
    if !point.valid() || points.len() > 16_384 {
        return false;
    }
    match points.binary_search_by(|existing| existing.time.total_cmp(&point.time)) {
        Ok(index) => points[index] = point,
        Err(index) => {
            if points.len() >= 16_384 {
                return false;
            }
            points.insert(index, point);
        }
    }
    true
}

pub fn remove_mpe_point(points: &mut Vec<MpeCurvePoint>, time: f64) -> bool {
    if !time.is_finite() {
        return false;
    }
    let Ok(index) = points.binary_search_by(|existing| existing.time.total_cmp(&time)) else {
        return false;
    };
    points.remove(index);
    true
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

    pub fn voice_for_note(&self, note: u8) -> Option<u8> {
        self.note_to_channel.get(&note).copied().filter(|channel| {
            (2..=16).contains(channel) && self.voices[(*channel - 1) as usize].active
        })
    }

    pub fn active_notes(&self) -> Vec<u8> {
        let mut notes: Vec<_> = self.note_to_channel.keys().copied().collect();
        notes.sort_unstable();
        notes
    }

    pub fn release_channel(&mut self, channel: u8) -> bool {
        if !(2..=16).contains(&channel) {
            return false;
        }
        let index = (channel - 1) as usize;
        if !self.voices[index].active {
            return false;
        }
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
            && self
                .voices
                .iter()
                .enumerate()
                .skip(1)
                .all(|(index, voice)| {
                    !voice.active
                        || self.note_to_channel.get(&voice.note) == Some(&((index + 1) as u8))
                })
    }
}


#[path = "mpe_expression.rs"]
mod mpe_expression;
pub use mpe_expression::*;
