use serde::{Deserialize, Serialize};

/// Host-neutral articulation/expression-map entry. Program and channel are
/// encoded exactly as MIDI values; transpose is a bounded semitone offset.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Articulation {
    pub name: String,
    pub program: u8,
    pub transpose: i16,
    pub channel: u8,
}

impl Articulation {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.channel < 16
            && self.transpose.abs() <= 48
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExpressionMap {
    pub name: String,
    pub articulations: Vec<Articulation>,
}

impl ExpressionMap {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.articulations.len() <= 1024
            && self.articulations.iter().all(Articulation::validate)
            && self.articulations.iter().enumerate().all(|(index, item)| {
                self.articulations[..index]
                    .iter()
                    .all(|previous| !previous.name.eq_ignore_ascii_case(&item.name))
            })
    }

    pub fn upsert(&mut self, mut articulation: Articulation) -> bool {
        if !articulation.validate() {
            return false;
        }
        articulation.name = articulation.name.trim().to_owned();
        if let Some(existing) = self
            .articulations
            .iter_mut()
            .find(|item| item.name.eq_ignore_ascii_case(&articulation.name))
        {
            *existing = articulation;
        } else if self.articulations.len() < 1024 {
            self.articulations.push(articulation);
        } else {
            return false;
        }
        self.articulations
            .sort_by_key(|item| item.name.to_ascii_lowercase());
        true
    }

    pub fn find(&self, name: &str) -> Option<&Articulation> {
        self.articulations
            .iter()
            .find(|item| item.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.articulations.len();
        self.articulations
            .retain(|item| !item.name.eq_ignore_ascii_case(name.trim()));
        before != self.articulations.len()
    }

    pub fn search(&self, query: &str) -> Vec<Articulation> {
        let query = query.trim().to_ascii_lowercase();
        let mut result: Vec<_> = self
            .articulations
            .iter()
            .filter(|item| query.is_empty() || item.name.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect();
        result.sort_by_key(|item| item.name.to_ascii_lowercase());
        result
    }

    /// Resolves the MIDI channel and transposed note for a selected program.
    pub fn resolve(&self, name: &str, note: u8) -> Option<(u8, u8, u8)> {
        let articulation = self.find(name)?;
        let transposed = i16::from(note)
            .checked_add(articulation.transpose)?
            .clamp(0, 127) as u8;
        Some((articulation.channel, articulation.program, transposed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_and_manages_articulations_case_insensitively() {
        let mut map = ExpressionMap {
            name: "Strings".into(),
            articulations: Vec::new(),
        };
        assert!(map.upsert(Articulation {
            name: "Legato ".into(),
            program: 4,
            transpose: 1,
            channel: 2
        }));
        assert_eq!(map.resolve("LEGATO", 60), Some((2, 4, 61)));
        assert_eq!(map.search("leg").len(), 1);
        assert!(map.remove("legato"));
        assert!(map.validate());
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArticulationRole {
    Direction,
    Attribute,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MidiOutput {
    KeySwitch {
        note: u8,
        velocity: u8,
        length_ticks: u32,
    },
    ProgramChange {
        bank_msb: Option<u8>,
        bank_lsb: Option<u8>,
        program: u8,
    },
    ControlChange {
        controller: u8,
        value: u8,
    },
    ChannelPressure {
        value: u8,
    },
    PitchBend {
        value: i16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteTrigger {
    Key { note: u8 },
    Program { program: u8 },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteTriggerMode {
    #[default]
    KeySwitch,
    ProgramChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupMove {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSlotMove {
    Up,
    Down,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProArticulation {
    pub id: String,
    pub name: String,
    pub role: ArticulationRole,
    pub group: u8,
    pub playback_technique: String,
    pub alias_for: Option<String>,
    pub fallback: Option<String>,
    pub remote_trigger: Option<RemoteTrigger>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SoundSlot {
    pub id: String,
    pub name: String,
    pub articulation_ids: Vec<String>,
    pub outputs: Vec<MidiOutput>,
    #[serde(default)]
    pub off_outputs: Vec<MidiOutput>,
    pub channel: Option<u8>,
    pub transpose: i8,
    pub velocity_scale: f32,
    pub pitch_range: Option<(u8, u8)>,
    pub velocity_range: Option<(u8, u8)>,
    #[serde(default)]
    pub add_on: bool,
    #[serde(default)]
    pub note_length_ticks: Option<u32>,
    #[serde(default)]
    pub attack_compensation_ticks: u32,
    #[serde(default)]
    pub separation_ticks: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ExpressionMapPro {
    pub name: String,
    pub articulations: Vec<ProArticulation>,
    pub sound_slots: Vec<SoundSlot>,
    pub default_slot: Option<String>,
    #[serde(default)]
    pub remote_trigger_mode: RemoteTriggerMode,
    pub latch_remote_triggers: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedExpressionNote {
    pub note: u8,
    pub velocity: u8,
    pub channel: u8,
    pub slot_id: String,
    pub add_on_slot_ids: Vec<String>,
    pub outputs: Vec<MidiOutput>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DynamicSymbol {
    Pppp,
    Ppp,
    Pp,
    P,
    Mp,
    Mf,
    F,
    Ff,
    Fff,
    Ffff,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DynamicRange {
    PpToFf,
    PpppToFfff,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DynamicVolumeOutput {
    Off,
    MainVolumeCc7,
    ExpressionCc11,
    Vst3Volume,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DynamicMappingEntry {
    pub symbol: DynamicSymbol,
    pub velocity_percent: f32,
    pub volume_value: u8,
    pub controller_value: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DynamicsMap {
    pub range: DynamicRange,
    pub change_velocities: bool,
    pub volume_output: DynamicVolumeOutput,
    pub send_controller: Option<u8>,
    pub entries: Vec<DynamicMappingEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedDynamics {
    pub velocity: u8,
    pub midi_outputs: Vec<MidiOutput>,
    pub vst3_volume: Option<f32>,
}

impl DynamicsMap {
    pub fn initialized(range: DynamicRange) -> Self {
        let symbols = [
            DynamicSymbol::Pppp,
            DynamicSymbol::Ppp,
            DynamicSymbol::Pp,
            DynamicSymbol::P,
            DynamicSymbol::Mp,
            DynamicSymbol::Mf,
            DynamicSymbol::F,
            DynamicSymbol::Ff,
            DynamicSymbol::Fff,
            DynamicSymbol::Ffff,
        ];
        let entries = symbols
            .into_iter()
            .enumerate()
            .map(|(index, symbol)| DynamicMappingEntry {
                symbol,
                velocity_percent: 25.0 + index as f32 * (125.0 / 9.0),
                volume_value: (16.0 + index as f32 * (111.0 / 9.0)).round() as u8,
                controller_value: (16.0 + index as f32 * (111.0 / 9.0)).round() as u8,
            })
            .collect();
        Self {
            range,
            change_velocities: true,
            volume_output: DynamicVolumeOutput::ExpressionCc11,
            send_controller: None,
            entries,
        }
    }

    pub fn apply(&self, symbol: DynamicSymbol, input_velocity: u8) -> Option<RenderedDynamics> {
        if !self.validate() || !(1..=127).contains(&input_velocity) {
            return None;
        }
        if self.range == DynamicRange::PpToFf
            && matches!(
                symbol,
                DynamicSymbol::Pppp | DynamicSymbol::Ppp | DynamicSymbol::Fff | DynamicSymbol::Ffff
            )
        {
            return Some(RenderedDynamics {
                velocity: input_velocity,
                midi_outputs: Vec::new(),
                vst3_volume: None,
            });
        }
        let entry = self.entries.iter().find(|entry| entry.symbol == symbol)?;
        let velocity = if self.change_velocities {
            (f32::from(input_velocity) * entry.velocity_percent / 100.0)
                .round()
                .clamp(1.0, 127.0) as u8
        } else {
            input_velocity
        };
        let mut midi_outputs = Vec::new();
        let mut vst3_volume = None;
        match self.volume_output {
            DynamicVolumeOutput::Off => {}
            DynamicVolumeOutput::MainVolumeCc7 => midi_outputs.push(MidiOutput::ControlChange {
                controller: 7,
                value: entry.volume_value,
            }),
            DynamicVolumeOutput::ExpressionCc11 => midi_outputs.push(MidiOutput::ControlChange {
                controller: 11,
                value: entry.volume_value,
            }),
            DynamicVolumeOutput::Vst3Volume => {
                vst3_volume = Some(f32::from(entry.volume_value) / 127.0)
            }
        }
        if let Some(controller) = self.send_controller {
            midi_outputs.push(MidiOutput::ControlChange {
                controller,
                value: entry.controller_value,
            });
        }
        Some(RenderedDynamics {
            velocity,
            midi_outputs,
            vst3_volume,
        })
    }

    pub fn validate(&self) -> bool {
        self.entries.len() == 10
            && self
                .send_controller
                .is_none_or(|controller| controller < 128)
            && self.entries.iter().all(|entry| {
                entry.velocity_percent.is_finite()
                    && (0.0..=800.0).contains(&entry.velocity_percent)
                    && entry.volume_value < 128
                    && entry.controller_value < 128
            })
            && self.entries.iter().enumerate().all(|(index, entry)| {
                self.entries[..index]
                    .iter()
                    .all(|previous| previous.symbol != entry.symbol)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpressionMapRuntime {
    directions: std::collections::BTreeMap<u8, String>,
    attributes: std::collections::BTreeSet<String>,
    latched_remote: Option<String>,
    active_slot: Option<String>,
    active_add_ons: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExpressionLaneEvent {
    Articulation(String),
    ResetGroup(u8),
    ResetAllDirections,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionSlotTransition {
    pub from_slot: Option<String>,
    pub to_slot: String,
    pub off_outputs: Vec<MidiOutput>,
    pub on_outputs: Vec<MidiOutput>,
    /// Negative means the note must be scheduled early.
    pub note_start_offset_ticks: i32,
    pub switch_offset_ticks: i32,
    pub note_length_ticks: Option<u32>,
}

/// Host-extracted VST 3 key-switch metadata. The plug-in adapter owns the VST
/// query; this transport-neutral value can safely cross the core boundary.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VstKeySwitchInfo {
    pub name: String,
    pub note: u8,
    pub velocity: u8,
    pub length_ticks: u32,
}

impl ExpressionMapPro {
    /// Creates a new expression map from the key-switch information exposed
    /// by a VST 3 instrument preset. The conversion is all-or-nothing so a
    /// malformed preset can never leave a partially usable map behind.
    pub fn from_vst_key_switches(
        name: impl Into<String>,
        key_switches: &[VstKeySwitchInfo],
    ) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty()
            || name.len() > 256
            || name.contains('\0')
            || key_switches.is_empty()
            || key_switches.len() > 128
        {
            return Err("invalid VST key-switch preset".into());
        }
        let mut names = std::collections::BTreeSet::new();
        let mut notes = std::collections::BTreeSet::new();
        if key_switches.iter().any(|key_switch| {
            key_switch.name.trim().is_empty()
                || key_switch.name.len() > 256
                || key_switch.name.contains('\0')
                || !(1..=127).contains(&key_switch.velocity)
                || !(1..=96_000).contains(&key_switch.length_ticks)
                || !names.insert(key_switch.name.trim().to_ascii_lowercase())
                || !notes.insert(key_switch.note)
        }) {
            return Err("invalid VST key-switch preset".into());
        }

        let mut articulations = Vec::with_capacity(key_switches.len());
        let mut sound_slots = Vec::with_capacity(key_switches.len());
        for (index, key_switch) in key_switches.iter().enumerate() {
            let id = format!("vst-keyswitch-{index}");
            let display_name = key_switch.name.trim().to_owned();
            articulations.push(ProArticulation {
                id: id.clone(),
                name: display_name.clone(),
                role: ArticulationRole::Direction,
                group: 1,
                playback_technique: display_name.clone(),
                alias_for: None,
                fallback: None,
                remote_trigger: Some(RemoteTrigger::Key {
                    note: key_switch.note,
                }),
            });
            sound_slots.push(SoundSlot {
                id: id.clone(),
                name: display_name,
                articulation_ids: vec![id],
                outputs: vec![MidiOutput::KeySwitch {
                    note: key_switch.note,
                    velocity: key_switch.velocity,
                    length_ticks: key_switch.length_ticks,
                }],
                off_outputs: Vec::new(),
                channel: None,
                transpose: 0,
                velocity_scale: 1.0,
                pitch_range: None,
                velocity_range: None,
                add_on: false,
                note_length_ticks: None,
                attack_compensation_ticks: 0,
                separation_ticks: 0,
            });
        }
        let map = Self {
            name: name.trim().to_owned(),
            default_slot: sound_slots.first().map(|slot| slot.id.clone()),
            articulations,
            sound_slots,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        map.validate()
            .then_some(map)
            .ok_or_else(|| "invalid VST key-switch preset".into())
    }

    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid expression map".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.validate() {
            Ok(value)
        } else {
            Err("invalid expression map".into())
        }
    }

    pub fn validate(&self) -> bool {
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self.name.contains('\0')
            || self.articulations.is_empty()
            || self.articulations.len() > 1024
            || self.sound_slots.is_empty()
            || self.sound_slots.len() > 4096
        {
            return false;
        }
        let articulation_ids: std::collections::BTreeSet<_> = self
            .articulations
            .iter()
            .map(|item| item.id.to_ascii_lowercase())
            .collect();
        let slot_ids: std::collections::BTreeSet<_> = self
            .sound_slots
            .iter()
            .map(|item| item.id.to_ascii_lowercase())
            .collect();
        if articulation_ids.len() != self.articulations.len()
            || slot_ids.len() != self.sound_slots.len()
            || !self
                .articulations
                .iter()
                .all(|item| item.validate(&articulation_ids))
            || !self
                .sound_slots
                .iter()
                .all(|item| item.validate(&articulation_ids))
            || !self.sound_slots.iter().any(|slot| !slot.add_on)
            || !self.articulations.iter().all(|item| {
                item.remote_trigger.as_ref().is_none_or(|trigger| {
                    matches!(
                        (self.remote_trigger_mode, trigger),
                        (RemoteTriggerMode::KeySwitch, RemoteTrigger::Key { .. })
                            | (
                                RemoteTriggerMode::ProgramChange,
                                RemoteTrigger::Program { .. }
                            )
                    )
                })
            })
            || self.articulations.iter().enumerate().any(|(index, item)| {
                item.remote_trigger.as_ref().is_some_and(|trigger| {
                    self.articulations[..index]
                        .iter()
                        .any(|previous| previous.remote_trigger.as_ref() == Some(trigger))
                })
            })
            || self.default_slot.as_ref().is_some_and(|id| {
                !slot_ids.contains(&id.to_ascii_lowercase())
                    || self.slot(id).is_none_or(|slot| slot.add_on)
            })
        {
            return false;
        }
        self.articulations
            .iter()
            .all(|item| self.resolve_articulation(&item.id).is_some())
    }

    pub fn articulation(&self, id: &str) -> Option<&ProArticulation> {
        self.articulations
            .iter()
            .find(|item| item.id.eq_ignore_ascii_case(id.trim()))
    }

    pub fn slot(&self, id: &str) -> Option<&SoundSlot> {
        self.sound_slots
            .iter()
            .find(|item| item.id.eq_ignore_ascii_case(id.trim()))
    }

    pub fn resolve_articulation(&self, id: &str) -> Option<&ProArticulation> {
        let mut current = self.articulation(id)?;
        let mut visited = std::collections::BTreeSet::new();
        loop {
            if !visited.insert(current.id.to_ascii_lowercase()) {
                return None;
            }
            let Some(alias) = current.alias_for.as_deref() else {
                return Some(current);
            };
            current = self.articulation(alias)?;
        }
    }

    /// Moves a mutual-exclusion group in the musical-priority order. Group 1
    /// remains the most important; moving swaps the complete groups rather
    /// than rewriting individual articulation membership.
    pub fn move_group(&mut self, group: u8, direction: GroupMove) -> bool {
        let mut groups = self
            .articulations
            .iter()
            .map(|item| item.group)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let Some(position) = groups.iter().position(|candidate| *candidate == group) else {
            return false;
        };
        let other_position = match direction {
            GroupMove::Up => position.checked_sub(1),
            GroupMove::Down if position + 1 < groups.len() => Some(position + 1),
            GroupMove::Down => None,
        };
        let Some(other_position) = other_position else {
            return false;
        };
        let other_group = groups[other_position];
        for articulation in &mut self.articulations {
            if articulation.group == group {
                articulation.group = other_group;
            } else if articulation.group == other_group {
                articulation.group = group;
            }
        }
        groups.swap(position, other_position);
        self.validate()
    }

    /// Transposes every assigned remote trigger atomically. If any trigger
    /// would leave the MIDI 0..127 range, the map is left unchanged.
    pub fn transpose_all_remote_triggers(&mut self, semitones: i16) -> bool {
        let shifted = self
            .articulations
            .iter()
            .map(|item| {
                item.remote_trigger.as_ref().map(|trigger| match trigger {
                    RemoteTrigger::Key { note } => i16::from(*note).checked_add(semitones),
                    RemoteTrigger::Program { program } => {
                        i16::from(*program).checked_add(semitones)
                    }
                })
            })
            .collect::<Vec<_>>();
        if shifted
            .iter()
            .flatten()
            .any(|value| value.is_none_or(|value| !(0..=127).contains(&value)))
        {
            return false;
        }
        for (articulation, shifted) in self.articulations.iter_mut().zip(shifted) {
            let Some(value) = shifted.flatten() else {
                continue;
            };
            articulation.remote_trigger = Some(match self.remote_trigger_mode {
                RemoteTriggerMode::KeySwitch => RemoteTrigger::Key { note: value as u8 },
                RemoteTriggerMode::ProgramChange => RemoteTrigger::Program {
                    program: value as u8,
                },
            });
        }
        self.validate()
    }

    /// Assigns consecutive remote triggers following sound-slot order. This
    /// mirrors Cubase's Reassign All action and is atomic on range overflow.
    pub fn reassign_all_remote_triggers(&mut self, first: u8) -> bool {
        let mut ordered_ids = Vec::new();
        for slot in &self.sound_slots {
            for id in &slot.articulation_ids {
                let Some(item) = self.resolve_articulation(id) else {
                    return false;
                };
                if !ordered_ids
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(&item.id))
                {
                    ordered_ids.push(item.id.clone());
                }
            }
        }
        for item in &self.articulations {
            if !ordered_ids
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&item.id))
            {
                ordered_ids.push(item.id.clone());
            }
        }
        if ordered_ids.len() > usize::from(128u8.saturating_sub(first)) {
            return false;
        }
        for item in &mut self.articulations {
            let Some(offset) = ordered_ids
                .iter()
                .position(|id| id.eq_ignore_ascii_case(&item.id))
            else {
                return false;
            };
            let value = first + offset as u8;
            item.remote_trigger = Some(match self.remote_trigger_mode {
                RemoteTriggerMode::KeySwitch => RemoteTrigger::Key { note: value },
                RemoteTriggerMode::ProgramChange => RemoteTrigger::Program { program: value },
            });
        }
        self.validate()
    }

    pub fn remove_all_remote_triggers(&mut self) {
        for articulation in &mut self.articulations {
            articulation.remote_trigger = None;
        }
    }

    pub fn duplicate_sound_slot(
        &mut self,
        source_id: &str,
        new_id: impl Into<String>,
        new_name: impl Into<String>,
    ) -> bool {
        if self.sound_slots.len() >= 4096 {
            return false;
        }
        let Some(source_index) = self
            .sound_slots
            .iter()
            .position(|slot| slot.id.eq_ignore_ascii_case(source_id.trim()))
        else {
            return false;
        };
        let mut copy = self.sound_slots[source_index].clone();
        copy.id = new_id.into().trim().to_owned();
        copy.name = new_name.into().trim().to_owned();
        let mut candidate = self.clone();
        candidate.sound_slots.insert(source_index + 1, copy);
        self.replace_if_valid(candidate)
    }

    pub fn rename_sound_slot(&mut self, id: &str, new_name: impl Into<String>) -> bool {
        let mut candidate = self.clone();
        let Some(slot) = candidate
            .sound_slots
            .iter_mut()
            .find(|slot| slot.id.eq_ignore_ascii_case(id.trim()))
        else {
            return false;
        };
        slot.name = new_name.into().trim().to_owned();
        self.replace_if_valid(candidate)
    }

    /// Marks a base slot as default and moves it to the top, matching the
    /// ordering behavior of Cubase's As Default action.
    pub fn set_default_sound_slot(&mut self, id: &str) -> bool {
        let mut candidate = self.clone();
        let Some(index) = candidate
            .sound_slots
            .iter()
            .position(|slot| slot.id.eq_ignore_ascii_case(id.trim()) && !slot.add_on)
        else {
            return false;
        };
        let slot = candidate.sound_slots.remove(index);
        candidate.default_slot = Some(slot.id.clone());
        candidate.sound_slots.insert(0, slot);
        self.replace_if_valid(candidate)
    }

    pub fn move_sound_slot(&mut self, id: &str, direction: SoundSlotMove) -> bool {
        let mut candidate = self.clone();
        let Some(index) = candidate
            .sound_slots
            .iter()
            .position(|slot| slot.id.eq_ignore_ascii_case(id.trim()))
        else {
            return false;
        };
        let other = match direction {
            SoundSlotMove::Up => index.checked_sub(1),
            SoundSlotMove::Down if index + 1 < candidate.sound_slots.len() => Some(index + 1),
            SoundSlotMove::Down => None,
        };
        let Some(other) = other else {
            return false;
        };
        candidate.sound_slots.swap(index, other);
        self.replace_if_valid(candidate)
    }

    pub fn remove_sound_slot(&mut self, id: &str) -> bool {
        let mut candidate = self.clone();
        let Some(index) = candidate
            .sound_slots
            .iter()
            .position(|slot| slot.id.eq_ignore_ascii_case(id.trim()))
        else {
            return false;
        };
        let removed = candidate.sound_slots.remove(index);
        if candidate
            .default_slot
            .as_deref()
            .is_some_and(|default| default.eq_ignore_ascii_case(&removed.id))
        {
            candidate.default_slot = candidate
                .sound_slots
                .iter()
                .find(|slot| !slot.add_on)
                .map(|slot| slot.id.clone());
        }
        self.replace_if_valid(candidate)
    }

    fn replace_if_valid(&mut self, candidate: Self) -> bool {
        if !candidate.validate() {
            return false;
        }
        *self = candidate;
        true
    }

    /// Finds an exact slot first, then the closest group-prioritized match.
    /// Group 1 has the highest weight, mirroring Cubase sound-slot fallback.
    pub fn resolve_slot(&self, requested_ids: &[String]) -> Option<&SoundSlot> {
        let requested = self.canonical_requested(requested_ids)?;
        let score = |slot: &SoundSlot| {
            let slot_ids: std::collections::BTreeSet<_> = slot
                .articulation_ids
                .iter()
                .filter_map(|id| {
                    self.resolve_articulation(id)
                        .map(|item| item.id.to_ascii_lowercase())
                })
                .collect();
            let exact = usize::from(slot_ids == requested);
            let matched = slot_ids
                .intersection(&requested)
                .filter_map(|id| self.articulation(id))
                .map(|item| 257u32 - u32::from(item.group))
                .sum::<u32>();
            (
                exact,
                matched,
                usize::MAX - slot_ids.symmetric_difference(&requested).count(),
            )
        };
        self.sound_slots
            .iter()
            .filter(|slot| !slot.add_on && (score(slot).0 == 1 || score(slot).1 > 0))
            .max_by_key(|slot| score(slot))
            .or_else(|| self.default_slot.as_deref().and_then(|id| self.slot(id)))
            .or_else(|| self.sound_slots.iter().find(|slot| !slot.add_on))
    }

    fn canonical_requested(&self, ids: &[String]) -> Option<std::collections::BTreeSet<String>> {
        let mut groups = std::collections::BTreeMap::new();
        let mut result = std::collections::BTreeSet::new();
        for id in ids {
            let item = self.resolve_articulation(id)?;
            if groups
                .insert(item.group, item.id.to_ascii_lowercase())
                .is_some()
            {
                return None;
            }
            result.insert(item.id.to_ascii_lowercase());
        }
        Some(result)
    }

    pub fn render(
        &self,
        requested_ids: &[String],
        note: u8,
        velocity: u8,
        channel: u8,
    ) -> Option<RenderedExpressionNote> {
        if !self.validate() || note > 127 || !(1..=127).contains(&velocity) || channel > 15 {
            return None;
        }
        let slot = self.resolve_slot(requested_ids)?;
        if slot
            .pitch_range
            .is_some_and(|(min, max)| !(min..=max).contains(&note))
            || slot
                .velocity_range
                .is_some_and(|(min, max)| !(min..=max).contains(&velocity))
        {
            return None;
        }
        let output_note = i16::from(note).checked_add(i16::from(slot.transpose))?;
        if !(0..=127).contains(&output_note) {
            return None;
        }
        let requested = self.canonical_requested(requested_ids)?;
        let mut add_ons = self
            .sound_slots
            .iter()
            .filter(|candidate| {
                candidate.add_on
                    && candidate
                        .pitch_range
                        .is_none_or(|(min, max)| (min..=max).contains(&note))
                    && candidate
                        .velocity_range
                        .is_none_or(|(min, max)| (min..=max).contains(&velocity))
                    && candidate.articulation_ids.iter().all(|id| {
                        self.resolve_articulation(id)
                            .is_some_and(|item| requested.contains(&item.id.to_ascii_lowercase()))
                    })
            })
            .collect::<Vec<_>>();
        add_ons.sort_by(|left, right| {
            left.id
                .to_ascii_lowercase()
                .cmp(&right.id.to_ascii_lowercase())
        });
        let mut outputs = slot.outputs.clone();
        outputs.extend(
            add_ons
                .iter()
                .flat_map(|add_on| add_on.outputs.iter().cloned()),
        );
        Some(RenderedExpressionNote {
            note: output_note as u8,
            velocity: (f32::from(velocity) * slot.velocity_scale)
                .round()
                .clamp(1.0, 127.0) as u8,
            channel: slot.channel.unwrap_or(channel),
            slot_id: slot.id.clone(),
            add_on_slot_ids: add_ons.iter().map(|slot| slot.id.clone()).collect(),
            outputs,
        })
    }
}

impl ExpressionMapRuntime {
    pub fn apply_lane_event(
        &mut self,
        map: &ExpressionMapPro,
        event: &ExpressionLaneEvent,
    ) -> bool {
        match event {
            ExpressionLaneEvent::Articulation(id) => self.activate(map, id),
            ExpressionLaneEvent::ResetGroup(group) => self.reset_group(map, *group),
            ExpressionLaneEvent::ResetAllDirections => {
                let changed = !self.directions.is_empty() || self.latched_remote.is_some();
                self.directions.clear();
                self.latched_remote = None;
                self.active_slot = None;
                self.active_add_ons.clear();
                changed
            }
        }
    }

    pub fn reset_group(&mut self, map: &ExpressionMapPro, group: u8) -> bool {
        if !(1..=16).contains(&group) {
            return false;
        }
        let mut changed = self.directions.remove(&group).is_some();
        if self
            .latched_remote
            .as_deref()
            .and_then(|id| map.resolve_articulation(id))
            .is_some_and(|item| item.group == group)
        {
            self.latched_remote = None;
            changed = true;
        }
        if changed {
            self.active_slot = None;
            self.active_add_ons.clear();
        }
        changed
    }

    pub fn activate(&mut self, map: &ExpressionMapPro, id: &str) -> bool {
        let Some(item) = map.resolve_articulation(id) else {
            return false;
        };
        match item.role {
            ArticulationRole::Direction => {
                self.directions.insert(item.group, item.id.clone());
            }
            ArticulationRole::Attribute => {
                self.attributes.insert(item.id.clone());
            }
        }
        true
    }

    pub fn trigger_remote(
        &mut self,
        map: &ExpressionMapPro,
        trigger: &RemoteTrigger,
        pressed: bool,
    ) -> bool {
        if !matches!(
            (map.remote_trigger_mode, trigger),
            (RemoteTriggerMode::KeySwitch, RemoteTrigger::Key { .. })
                | (
                    RemoteTriggerMode::ProgramChange,
                    RemoteTrigger::Program { .. }
                )
        ) {
            return false;
        }
        let Some(item) = map
            .articulations
            .iter()
            .find(|item| item.remote_trigger.as_ref() == Some(trigger))
        else {
            return false;
        };
        if map.remote_trigger_mode == RemoteTriggerMode::ProgramChange {
            return !pressed || self.activate(map, &item.id);
        }
        if pressed {
            if map.latch_remote_triggers {
                self.latched_remote = Some(item.id.clone());
            }
            self.activate(map, &item.id)
        } else if !map.latch_remote_triggers {
            match item.role {
                ArticulationRole::Attribute => {
                    self.attributes.remove(&item.id);
                }
                ArticulationRole::Direction => {
                    if self
                        .directions
                        .get(&item.group)
                        .is_some_and(|active| active.eq_ignore_ascii_case(&item.id))
                    {
                        self.directions.remove(&item.group);
                    }
                }
            }
            true
        } else {
            true
        }
    }

    pub fn render_note(
        &mut self,
        map: &ExpressionMapPro,
        note: u8,
        velocity: u8,
        channel: u8,
    ) -> Option<RenderedExpressionNote> {
        let mut requested: Vec<String> = self.directions.values().cloned().collect();
        requested.extend(self.attributes.iter().cloned());
        if let Some(remote) = &self.latched_remote {
            requested.push(remote.clone());
        }
        requested.sort();
        requested.dedup();
        let rendered = map.render(&requested, note, velocity, channel);
        self.attributes.clear();
        rendered
    }

    pub fn transition(
        &mut self,
        map: &ExpressionMapPro,
        rendered: &RenderedExpressionNote,
    ) -> Option<ExpressionSlotTransition> {
        let slot = map.slot(&rendered.slot_id)?;
        if slot.add_on {
            return None;
        }
        let next_add_ons = rendered
            .add_on_slot_ids
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        let base_changed = self
            .active_slot
            .as_ref()
            .is_none_or(|id| !id.eq_ignore_ascii_case(&slot.id));
        let mut off_outputs = Vec::new();
        if base_changed {
            if let Some(previous) = self.active_slot.as_deref().and_then(|id| map.slot(id)) {
                off_outputs.extend(previous.off_outputs.clone());
            }
        }
        for id in self.active_add_ons.difference(&next_add_ons) {
            if let Some(previous) = map.slot(id) {
                off_outputs.extend(previous.off_outputs.clone());
            }
        }
        let mut on_outputs = Vec::new();
        if base_changed {
            on_outputs.extend(slot.outputs.clone());
        }
        for id in next_add_ons.difference(&self.active_add_ons) {
            if let Some(next) = map.slot(id) {
                on_outputs.extend(next.outputs.clone());
            }
        }
        let from_slot = self.active_slot.clone();
        self.active_slot = Some(slot.id.clone());
        self.active_add_ons = next_add_ons;
        Some(ExpressionSlotTransition {
            from_slot,
            to_slot: slot.id.clone(),
            off_outputs,
            on_outputs,
            note_start_offset_ticks: -(slot.attack_compensation_ticks as i32),
            switch_offset_ticks: -(slot.separation_ticks as i32),
            note_length_ticks: slot.note_length_ticks,
        })
    }

    pub fn active_directions(&self) -> Vec<&str> {
        self.directions.values().map(String::as_str).collect()
    }
}

impl ProArticulation {
    fn validate(&self, ids: &std::collections::BTreeSet<String>) -> bool {
        let reference_valid = |reference: &Option<String>| {
            reference.as_ref().is_none_or(|id| {
                !id.eq_ignore_ascii_case(&self.id) && ids.contains(&id.to_ascii_lowercase())
            })
        };
        !self.id.trim().is_empty()
            && self.id.len() <= 128
            && !self.id.contains('\0')
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && (1..=16).contains(&self.group)
            && self.playback_technique.len() <= 256
            && !self.playback_technique.contains('\0')
            && reference_valid(&self.alias_for)
            && reference_valid(&self.fallback)
            && self
                .remote_trigger
                .as_ref()
                .is_none_or(RemoteTrigger::validate)
    }
}

impl RemoteTrigger {
    fn validate(&self) -> bool {
        match self {
            Self::Key { note } => *note < 128,
            Self::Program { program } => *program < 128,
        }
    }
}

impl MidiOutput {
    fn validate(&self) -> bool {
        match self {
            Self::KeySwitch {
                note,
                velocity,
                length_ticks,
            } => *note < 128 && (1..=127).contains(velocity) && (1..=96_000).contains(length_ticks),
            Self::ProgramChange {
                bank_msb,
                bank_lsb,
                program,
            } => {
                bank_msb.is_none_or(|v| v < 128)
                    && bank_lsb.is_none_or(|v| v < 128)
                    && *program < 128
            }
            Self::ControlChange { controller, value } => *controller < 128 && *value < 128,
            Self::ChannelPressure { value } => *value < 128,
            Self::PitchBend { value } => (-8192..=8191).contains(value),
        }
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn pro_expression_map_round_trips_only_valid_alias_graphs() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![ProArticulation {
                id: "legato".into(),
                name: "Legato".into(),
                role: ArticulationRole::Direction,
                group: 1,
                playback_technique: "legato".into(),
                alias_for: None,
                fallback: None,
                remote_trigger: Some(RemoteTrigger::Key { note: 24 }),
            }],
            sound_slots: vec![SoundSlot {
                id: "legato-slot".into(),
                name: "Legato".into(),
                articulation_ids: vec!["legato".into()],
                outputs: vec![MidiOutput::KeySwitch {
                    note: 24,
                    velocity: 100,
                    length_ticks: 120,
                }],
                off_outputs: Vec::new(),
                channel: None,
                transpose: 0,
                velocity_scale: 1.0,
                pitch_range: None,
                velocity_range: None,
                add_on: false,
                note_length_ticks: None,
                attack_compensation_ticks: 0,
                separation_ticks: 0,
            }],
            default_slot: Some("legato-slot".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: true,
        };
        let json = map.to_json().unwrap();
        assert_eq!(ExpressionMapPro::from_json(&json).unwrap(), map);
        let invalid = json.replace(
            "\"default_slot\":\"legato-slot\"",
            "\"default_slot\":\"missing-slot\"",
        );
        assert!(ExpressionMapPro::from_json(&invalid).is_err());
    }
}

impl SoundSlot {
    fn validate(&self, ids: &std::collections::BTreeSet<String>) -> bool {
        let own_ids: std::collections::BTreeSet<_> = self
            .articulation_ids
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .collect();
        !self.id.trim().is_empty()
            && self.id.len() <= 128
            && !self.id.contains('\0')
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && !self.articulation_ids.is_empty()
            && self.articulation_ids.len() <= 16
            && own_ids.len() == self.articulation_ids.len()
            && own_ids.iter().all(|id| ids.contains(id))
            && self.outputs.len() <= 32
            && self.off_outputs.len() <= 32
            && self.outputs.iter().all(MidiOutput::validate)
            && self.off_outputs.iter().all(MidiOutput::validate)
            && self.channel.is_none_or(|value| value < 16)
            && (-48..=48).contains(&self.transpose)
            && self.velocity_scale.is_finite()
            && (0.01..=8.0).contains(&self.velocity_scale)
            && self
                .pitch_range
                .is_none_or(|(min, max)| min <= max && max < 128)
            && self
                .velocity_range
                .is_none_or(|(min, max)| min >= 1 && min <= max && max <= 127)
            && (!self.add_on
                || self.channel.is_none()
                    && self.transpose == 0
                    && (self.velocity_scale - 1.0).abs() < f32::EPSILON)
            && self
                .note_length_ticks
                .is_none_or(|length| (1..=96_000).contains(&length))
            && self.attack_compensation_ticks <= 96_000
            && self.separation_ticks <= 96_000
    }
}

#[cfg(test)]
mod pro_tests {
    use super::*;

    fn articulation(id: &str, role: ArticulationRole, group: u8) -> ProArticulation {
        ProArticulation {
            id: id.into(),
            name: id.into(),
            role,
            group,
            playback_technique: id.into(),
            alias_for: None,
            fallback: None,
            remote_trigger: None,
        }
    }
    fn slot(id: &str, articulations: &[&str]) -> SoundSlot {
        SoundSlot {
            id: id.into(),
            name: id.into(),
            articulation_ids: articulations.iter().map(|id| (*id).into()).collect(),
            outputs: vec![],
            off_outputs: vec![],
            channel: None,
            transpose: 0,
            velocity_scale: 1.0,
            pitch_range: None,
            velocity_range: None,
            add_on: false,
            note_length_ticks: None,
            attack_compensation_ticks: 0,
            separation_ticks: 0,
        }
    }

    #[test]
    fn imports_vst3_preset_key_switches_as_a_complete_expression_map() {
        let switches = vec![
            VstKeySwitchInfo {
                name: " Sustain ".into(),
                note: 24,
                velocity: 100,
                length_ticks: 120,
            },
            VstKeySwitchInfo {
                name: "Pizzicato".into(),
                note: 25,
                velocity: 127,
                length_ticks: 60,
            },
        ];
        let map = ExpressionMapPro::from_vst_key_switches(" Iconica VX ", &switches).unwrap();
        assert_eq!(map.name, "Iconica VX");
        assert_eq!(map.default_slot.as_deref(), Some("vst-keyswitch-0"));
        assert_eq!(map.articulations.len(), 2);
        assert_eq!(map.sound_slots.len(), 2);
        assert_eq!(map.articulations[0].name, "Sustain");
        assert_eq!(
            map.articulations[1].remote_trigger,
            Some(RemoteTrigger::Key { note: 25 })
        );
        assert_eq!(
            map.sound_slots[1].outputs,
            vec![MidiOutput::KeySwitch {
                note: 25,
                velocity: 127,
                length_ticks: 60
            }]
        );
        assert_eq!(
            map.render(&["vst-keyswitch-1".into()], 60, 100, 0)
                .unwrap()
                .slot_id,
            "vst-keyswitch-1"
        );
        assert!(map.validate());
    }

    #[test]
    fn vst_key_switch_import_rejects_ambiguous_or_malformed_metadata() {
        let valid = VstKeySwitchInfo {
            name: "Sustain".into(),
            note: 24,
            velocity: 100,
            length_ticks: 120,
        };
        assert!(ExpressionMapPro::from_vst_key_switches("", &[valid.clone()]).is_err());
        let duplicate_note = VstKeySwitchInfo {
            name: "Pizz".into(),
            ..valid.clone()
        };
        assert!(ExpressionMapPro::from_vst_key_switches(
            "Strings",
            &[valid.clone(), duplicate_note]
        )
        .is_err());
        let duplicate_name = VstKeySwitchInfo {
            note: 25,
            ..valid.clone()
        };
        assert!(ExpressionMapPro::from_vst_key_switches(
            "Strings",
            &[valid.clone(), duplicate_name]
        )
        .is_err());
        let invalid_velocity = VstKeySwitchInfo {
            velocity: 0,
            ..valid
        };
        assert!(ExpressionMapPro::from_vst_key_switches("Strings", &[invalid_velocity]).is_err());
    }

    #[test]
    fn sound_slot_actions_preserve_a_valid_editable_map() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("sustain", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![slot("sustain", &["sustain"]), slot("pizz", &["pizz"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };

        assert!(map.duplicate_sound_slot("pizz", "pizz-soft", "Pizzicato Soft"));
        assert_eq!(
            map.sound_slots
                .iter()
                .map(|slot| slot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["sustain", "pizz", "pizz-soft"]
        );
        assert!(map.rename_sound_slot("pizz-soft", "Pizzicato molto piano"));
        assert_eq!(map.slot("pizz-soft").unwrap().name, "Pizzicato molto piano");
        assert!(map.move_sound_slot("pizz-soft", SoundSlotMove::Up));
        assert_eq!(map.sound_slots[1].id, "pizz-soft");
        assert!(map.set_default_sound_slot("pizz"));
        assert_eq!(map.default_slot.as_deref(), Some("pizz"));
        assert_eq!(map.sound_slots[0].id, "pizz");
        assert!(map.remove_sound_slot("pizz"));
        assert_eq!(map.default_slot.as_deref(), Some("sustain"));
        assert!(map.validate());
    }

    #[test]
    fn invalid_sound_slot_actions_are_atomic() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![articulation("sustain", ArticulationRole::Direction, 1)],
            sound_slots: vec![slot("sustain", &["sustain"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let snapshot = map.clone();
        assert!(!map.duplicate_sound_slot("sustain", "sustain", "Duplicate ID"));
        assert_eq!(map, snapshot);
        assert!(!map.rename_sound_slot("sustain", "  "));
        assert_eq!(map, snapshot);
        assert!(!map.move_sound_slot("sustain", SoundSlotMove::Up));
        assert_eq!(map, snapshot);
        assert!(!map.remove_sound_slot("sustain"));
        assert_eq!(map, snapshot);
    }

    #[test]
    fn exact_sound_slot_renders_multiple_midi_outputs() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![SoundSlot {
                outputs: vec![
                    MidiOutput::KeySwitch {
                        note: 24,
                        velocity: 127,
                        length_ticks: 120,
                    },
                    MidiOutput::ControlChange {
                        controller: 1,
                        value: 96,
                    },
                ],
                channel: Some(3),
                transpose: 12,
                velocity_scale: 0.5,
                ..slot("pizz-accent", &["pizz", "accent"])
            }],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let rendered = map
            .render(&["pizz".into(), "accent".into()], 60, 100, 0)
            .unwrap();
        assert_eq!(
            (
                rendered.note,
                rendered.velocity,
                rendered.channel,
                rendered.outputs.len()
            ),
            (72, 50, 3, 2)
        );
        assert_eq!(rendered.slot_id, "pizz-accent");
        assert!(rendered.add_on_slot_ids.is_empty());
    }

    #[test]
    fn add_on_slots_layer_independent_switches_without_combination_slot() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("sordino", ArticulationRole::Direction, 2),
            ],
            sound_slots: vec![
                SoundSlot {
                    outputs: vec![MidiOutput::KeySwitch {
                        note: 24,
                        velocity: 127,
                        length_ticks: 120,
                    }],
                    ..slot("pizz", &["pizz"])
                },
                SoundSlot {
                    outputs: vec![MidiOutput::ControlChange {
                        controller: 15,
                        value: 127,
                    }],
                    add_on: true,
                    ..slot("sordino-addon", &["sordino"])
                },
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        let rendered = map
            .render(&["pizz".into(), "sordino".into()], 60, 100, 0)
            .unwrap();
        assert_eq!(rendered.slot_id, "pizz");
        assert_eq!(rendered.add_on_slot_ids, vec!["sordino-addon"]);
        assert_eq!(
            rendered.outputs,
            vec![
                MidiOutput::KeySwitch {
                    note: 24,
                    velocity: 127,
                    length_ticks: 120
                },
                MidiOutput::ControlChange {
                    controller: 15,
                    value: 127
                },
            ]
        );
        assert!(map
            .render(&["pizz".into()], 60, 100, 0)
            .unwrap()
            .add_on_slot_ids
            .is_empty());
    }

    #[test]
    fn slot_transition_sends_off_before_on_and_exposes_timing_modifiers() {
        let mut legato_slot = slot("legato", &["legato"]);
        legato_slot.outputs = vec![MidiOutput::ControlChange {
            controller: 15,
            value: 127,
        }];
        legato_slot.off_outputs = vec![MidiOutput::ControlChange {
            controller: 15,
            value: 0,
        }];
        legato_slot.note_length_ticks = Some(360);
        legato_slot.attack_compensation_ticks = 48;
        legato_slot.separation_ticks = 12;
        let mut pizz_slot = slot("pizz", &["pizz"]);
        pizz_slot.outputs = vec![MidiOutput::KeySwitch {
            note: 24,
            velocity: 100,
            length_ticks: 30,
        }];
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("legato", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![legato_slot, pizz_slot],
            default_slot: Some("legato".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "legato"));
        let rendered = runtime.render_note(&map, 60, 100, 0).unwrap();
        let first = runtime.transition(&map, &rendered).unwrap();
        assert!(first.off_outputs.is_empty());
        assert_eq!(
            first.on_outputs,
            vec![MidiOutput::ControlChange {
                controller: 15,
                value: 127
            }]
        );
        assert_eq!(
            (
                first.note_start_offset_ticks,
                first.switch_offset_ticks,
                first.note_length_ticks
            ),
            (-48, -12, Some(360))
        );

        assert!(runtime.activate(&map, "pizz"));
        let rendered = runtime.render_note(&map, 62, 100, 0).unwrap();
        let second = runtime.transition(&map, &rendered).unwrap();
        assert_eq!(second.from_slot.as_deref(), Some("legato"));
        assert_eq!(
            second.off_outputs,
            vec![MidiOutput::ControlChange {
                controller: 15,
                value: 0
            }]
        );
        assert_eq!(
            second.on_outputs,
            vec![MidiOutput::KeySwitch {
                note: 24,
                velocity: 100,
                length_ticks: 30
            }]
        );
    }

    #[test]
    fn unchanged_slot_does_not_retransmit_on_events() {
        let mut sustain = slot("sustain", &["sustain"]);
        sustain.outputs = vec![MidiOutput::ProgramChange {
            bank_msb: None,
            bank_lsb: None,
            program: 3,
        }];
        let map = ExpressionMapPro {
            name: "Brass".into(),
            articulations: vec![articulation("sustain", ArticulationRole::Direction, 1)],
            sound_slots: vec![sustain],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::ProgramChange,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "sustain"));
        let rendered = runtime.render_note(&map, 60, 100, 0).unwrap();
        assert_eq!(
            runtime
                .transition(&map, &rendered)
                .unwrap()
                .on_outputs
                .len(),
            1
        );
        let rendered = runtime.render_note(&map, 62, 100, 0).unwrap();
        let transition = runtime.transition(&map, &rendered).unwrap();
        assert!(transition.on_outputs.is_empty());
        assert!(transition.off_outputs.is_empty());
    }

    #[test]
    fn add_on_slot_cannot_be_the_only_or_default_base_slot() {
        let mut add_on = slot("addon", &["accent"]);
        add_on.add_on = true;
        let map = ExpressionMapPro {
            name: "Invalid".into(),
            articulations: vec![articulation("accent", ArticulationRole::Attribute, 1)],
            sound_slots: vec![add_on],
            default_slot: Some("addon".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(!map.validate());
    }

    #[test]
    fn closest_slot_prioritizes_the_most_important_group() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "pizz"
        );
    }

    #[test]
    fn moving_groups_changes_closest_match_priority_for_the_complete_group() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("arco", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "pizz"
        );
        assert!(map.move_group(2, GroupMove::Up));
        assert_eq!(map.articulation("accent").unwrap().group, 1);
        assert_eq!(map.articulation("pizz").unwrap().group, 2);
        assert_eq!(map.articulation("arco").unwrap().group, 2);
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "accent"
        );
        assert!(!map.move_group(1, GroupMove::Up));
    }

    #[test]
    fn remote_trigger_bulk_actions_are_ordered_atomic_and_mode_aware() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.reassign_all_remote_triggers(24));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 24 })
        );
        assert_eq!(
            map.articulation("pizz").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 25 })
        );
        assert!(map.transpose_all_remote_triggers(12));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 36 })
        );
        let snapshot = map.clone();
        assert!(!map.transpose_all_remote_triggers(100));
        assert_eq!(map, snapshot);
        assert!(!map.reassign_all_remote_triggers(127));
        assert_eq!(map, snapshot);
        map.remove_all_remote_triggers();
        assert!(map
            .articulations
            .iter()
            .all(|item| item.remote_trigger.is_none()));

        map.remote_trigger_mode = RemoteTriggerMode::ProgramChange;
        assert!(map.reassign_all_remote_triggers(5));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Program { program: 5 })
        );
    }

    #[test]
    fn directions_persist_and_attributes_apply_to_one_note() {
        let mut accent = articulation("accent", ArticulationRole::Attribute, 2);
        accent.remote_trigger = Some(RemoteTrigger::Key { note: 12 });
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![articulation("pizz", ArticulationRole::Direction, 1), accent],
            sound_slots: vec![
                slot("pizz", &["pizz"]),
                slot("pizz-accent", &["pizz", "accent"]),
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "pizz"));
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 12 }, true));
        assert_eq!(
            runtime.render_note(&map, 60, 100, 0).unwrap().slot_id,
            "pizz-accent"
        );
        assert_eq!(
            runtime.render_note(&map, 62, 100, 0).unwrap().slot_id,
            "pizz"
        );
        assert_eq!(runtime.active_directions(), vec!["pizz"]);
    }

    #[test]
    fn direction_reset_event_clears_one_group_or_all_groups() {
        let map = ExpressionMapPro {
            name: "Orchestra".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("vibrato", ArticulationRole::Direction, 2),
            ],
            sound_slots: vec![
                slot("pizz", &["pizz"]),
                slot("vibrato", &["vibrato"]),
                slot("both", &["pizz", "vibrato"]),
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::Articulation("pizz".into())));
        assert!(
            runtime.apply_lane_event(&map, &ExpressionLaneEvent::Articulation("vibrato".into()))
        );
        assert_eq!(
            runtime.render_note(&map, 60, 100, 0).unwrap().slot_id,
            "both"
        );
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetGroup(1)));
        assert_eq!(runtime.active_directions(), vec!["vibrato"]);
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetAllDirections));
        assert!(runtime.active_directions().is_empty());
        assert!(!runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetGroup(0)));
    }

    #[test]
    fn momentary_remote_direction_is_removed_on_key_release() {
        let mut legato = articulation("legato", ArticulationRole::Direction, 1);
        legato.remote_trigger = Some(RemoteTrigger::Key { note: 24 });
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![legato],
            sound_slots: vec![slot("legato", &["legato"])],
            default_slot: Some("legato".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 24 }, true));
        assert_eq!(runtime.active_directions(), vec!["legato"]);
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 24 }, false));
        assert!(runtime.active_directions().is_empty());
    }

    #[test]
    fn program_change_remote_mode_persists_without_note_off_semantics() {
        let mut sustain = articulation("sustain", ArticulationRole::Direction, 1);
        sustain.remote_trigger = Some(RemoteTrigger::Program { program: 5 });
        let map = ExpressionMapPro {
            name: "Brass".into(),
            articulations: vec![sustain],
            sound_slots: vec![slot("sustain", &["sustain"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::ProgramChange,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Program { program: 5 }, true));
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Program { program: 5 }, false));
        assert_eq!(runtime.active_directions(), vec!["sustain"]);
        assert!(!runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 5 }, true));
    }

    #[test]
    fn remote_mode_mismatch_and_duplicate_triggers_are_rejected() {
        let mut first = articulation("one", ArticulationRole::Direction, 1);
        first.remote_trigger = Some(RemoteTrigger::Key { note: 20 });
        let mut second = articulation("two", ArticulationRole::Direction, 2);
        second.remote_trigger = Some(RemoteTrigger::Key { note: 20 });
        let mut map = ExpressionMapPro {
            name: "Invalid".into(),
            articulations: vec![first, second],
            sound_slots: vec![slot("one", &["one"]), slot("two", &["two"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: true,
        };
        assert!(!map.validate());
        map.articulations[1].remote_trigger = Some(RemoteTrigger::Key { note: 21 });
        assert!(map.validate());
        map.remote_trigger_mode = RemoteTriggerMode::ProgramChange;
        assert!(!map.validate());
    }

    #[test]
    fn rejects_conflicting_articulations_from_the_same_group() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("arco", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![slot("arco", &["arco"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        assert!(map.resolve_slot(&["arco".into(), "pizz".into()]).is_none());
    }

    #[test]
    fn dynamics_mapping_changes_velocity_and_sends_volume_and_custom_cc() {
        let mut dynamics = DynamicsMap::initialized(DynamicRange::PpppToFfff);
        dynamics.volume_output = DynamicVolumeOutput::ExpressionCc11;
        dynamics.send_controller = Some(1);
        let rendered = dynamics.apply(DynamicSymbol::Ff, 100).unwrap();
        assert!(rendered.velocity > 100);
        assert_eq!(rendered.midi_outputs.len(), 2);
        assert!(matches!(
            rendered.midi_outputs[0],
            MidiOutput::ControlChange { controller: 11, .. }
        ));
        assert!(matches!(
            rendered.midi_outputs[1],
            MidiOutput::ControlChange { controller: 1, .. }
        ));
        assert!(rendered.vst3_volume.is_none());
        assert!(dynamics.validate());
    }

    #[test]
    fn compact_dynamic_range_ignores_extreme_symbols() {
        let dynamics = DynamicsMap::initialized(DynamicRange::PpToFf);
        let rendered = dynamics.apply(DynamicSymbol::Pppp, 80).unwrap();
        assert_eq!(rendered.velocity, 80);
        assert!(rendered.midi_outputs.is_empty());
    }

    #[test]
    fn vst3_dynamic_volume_is_normalized_without_emitting_midi_volume() {
        let mut dynamics = DynamicsMap::initialized(DynamicRange::PpppToFfff);
        dynamics.change_velocities = false;
        dynamics.volume_output = DynamicVolumeOutput::Vst3Volume;
        let rendered = dynamics.apply(DynamicSymbol::Mf, 96).unwrap();
        assert_eq!(rendered.velocity, 96);
        assert!(rendered.midi_outputs.is_empty());
        assert!(rendered
            .vst3_volume
            .is_some_and(|value| (0.0..=1.0).contains(&value)));
    }
}
