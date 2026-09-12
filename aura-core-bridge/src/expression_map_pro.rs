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
