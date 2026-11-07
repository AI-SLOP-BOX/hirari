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
