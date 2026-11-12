impl HirariCore {
    pub fn step_sequencer_patterns_json(&self) -> String {
        self.step_sequencer_patterns
            .lock()
            .ok()
            .and_then(|patterns| serde_json::to_string(&*patterns).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    /// Replace the persisted step-sequencer pattern display settings.
    ///
    /// Validation accepts a pattern when its track exists and is
    /// MIDI/Instrument — and, when `region_id != 0`, the region is also
    /// present on that track — with unique lane pitches (≤ 127), an allowed
    /// resolution (4/8/16/32/64) and a feel (≤ 2), at most 1 024 patterns,
    /// and no duplicate `(track_id, region_id)` scopes.
    pub fn set_step_sequencer_patterns_json(&self, snapshot: &str) -> bool {
        let Ok(patterns) = serde_json::from_str::<
            Vec<crate::project::StepSequencerPatternContract>,
        >(snapshot) else {
            return false;
        };
        if patterns.len() > 1024 {
            return false;
        }
        let Ok(layout) =
            serde_json::from_str::<serde_json::Value>(&self.get_project_layout_json())
        else {
            return false;
        };
        // Track types by id and the owning track per region id, for the
        // existence/ownership checks the document-level validator applies.
        let mut track_types = std::collections::HashMap::with_capacity(8);
        let mut region_owner = std::collections::HashMap::with_capacity(32);
        for track in layout
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(id) = track.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let track_type = track
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            track_types.insert(id, track_type);
            for region in track
                .get("regions")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(region_id) = region.get("id").and_then(serde_json::Value::as_u64) {
                    region_owner.insert(region_id, id);
                }
            }
        }
        let mut unique_scope = std::collections::HashSet::with_capacity(patterns.len());
        for pattern in &patterns {
            if pattern.track_id == 0 {
                return false;
            }
            let Some(track_type) = track_types.get(&u64::from(pattern.track_id)) else {
                return false;
            };
            if !matches!(track_type.as_str(), "Midi" | "MIDI" | "Instrument") {
                return false;
            }
            if !matches!(pattern.resolution, 4 | 8 | 16 | 32 | 64) || pattern.feel > 2 {
                return false;
            }
            let unique_pitches: std::collections::HashSet<u8> =
                pattern.lane_pitches.iter().copied().collect();
            if unique_pitches.len() != pattern.lane_pitches.len() {
                return false;
            }
            if pattern.region_id != 0 {
                match region_owner.get(&u64::from(pattern.region_id)) {
                    Some(owner) if *owner == u64::from(pattern.track_id) => {}
                    _ => return false,
                }
            }
            if !unique_scope.insert((pattern.track_id, pattern.region_id)) {
                return false;
            }
        }
        let Ok(mut current) = self.step_sequencer_patterns.lock() else {
            return false;
        };
        *current = patterns;
        true
    }

    pub fn track_stacks_json(&self) -> String {
        self.track_stacks
            .lock()
            .ok()
            .and_then(|stacks| serde_json::to_string(&*stacks).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn markers_json(&self) -> String {
        self.markers
            .lock()
            .ok()
            .and_then(|value| serde_json::to_string(&*value).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn restore_markers_json(&self, snapshot: &str) -> bool {
        let Ok(candidate) = serde_json::from_str::<Vec<MarkerContract>>(snapshot) else {
            return false;
        };
        let mut ids = HashSet::with_capacity(candidate.len());
        if candidate.len() > 65_536
            || candidate
                .iter()
                .any(|marker| marker.validate().is_err() || !ids.insert(marker.id))
        {
            return false;
        }
        let Ok(mut current) = self.markers.lock() else {
            return false;
        };
        *current = candidate;
        true
    }

    pub fn upsert_marker(&self, id: u32, label: &str, beat: f64, color: &str) -> bool {
        let marker = MarkerContract {
            id,
            label: label.to_owned(),
            beat,
            color: color.to_owned(),
        };
        if marker.validate().is_err() {
            return false;
        }
        let Ok(mut markers) = self.markers.lock() else {
            return false;
        };
        if let Some(existing) = markers.iter_mut().find(|item| item.id == id) {
            *existing = marker;
        } else if markers.len() >= 65_536 {
            return false;
        } else {
            markers.push(marker);
        }
        markers.sort_by(|a, b| a.beat.total_cmp(&b.beat).then_with(|| a.id.cmp(&b.id)));
        drop(markers);
        self.publish_production_event(crate::production_events::ProductionEvent::MarkerChanged {
            marker_id: id,
        });
        true
    }

    pub fn delete_marker(&self, id: u32) -> bool {
        let Ok(mut markers) = self.markers.lock() else {
            return false;
        };
        let before = markers.len();
        markers.retain(|marker| marker.id != id);
        let changed = markers.len() != before;
        drop(markers);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::MarkerChanged { marker_id: id },
            );
        }
        changed
    }

    pub fn reset_markers(&self) {
        if let Ok(mut markers) = self.markers.lock() {
            markers.clear();
            markers.push(MarkerContract {
                id: 1,
                label: "START".to_owned(),
                beat: 0.0,
                color: "#646496".to_owned(),
            });
        }
    }

    pub fn restore_track_stacks_json(&self, snapshot: &str) -> bool {
        let Ok(candidate) = serde_json::from_str::<Vec<TrackStackContract>>(snapshot) else {
            return false;
        };
        let mut ids = HashSet::new();
        if candidate.iter().any(|stack| {
            stack.id == 0
                || !ids.insert(stack.id)
                || stack.name.trim().is_empty()
                || stack.name.len() > 256
                || !stack.master_gain.is_finite()
                || !(0.0..=2.0).contains(&stack.master_gain)
                || {
                    let mut members = HashSet::new();
                    stack
                        .member_track_ids
                        .iter()
                        .any(|id| *id == 0 || !members.insert(*id))
                }
        }) {
            return false;
        }
        let Ok(mut current) = self.track_stacks.lock() else {
            return false;
        };
        *current = candidate;
        drop(current);
        if let Ok(mut bases) = self.track_stack_base_volumes.lock() {
            bases.clear();
        }
        self.apply_track_stack_gains()
    }

    /// Recomputes native member faders from their unscaled project values.
    /// Stack gain is therefore idempotent and overlapping stacks remain
    /// deterministic instead of multiplying an already-scaled fader.
    fn apply_track_stack_gains(&self) -> bool {
        let Ok(stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Ok(mut bases) = self.track_stack_base_volumes.lock() else {
            return false;
        };
        let Some(engine) = self.engine_opt() else {
            return false;
        };
        let mut multipliers = bases
            .keys()
            .copied()
            .map(|track_id| (track_id, 1.0))
            .collect::<HashMap<_, _>>();
        for stack in stacks.iter() {
            for track_id in &stack.member_track_ids {
                multipliers
                    .entry(*track_id)
                    .and_modify(|value| *value *= stack.master_gain)
                    .or_insert(stack.master_gain);
                let entry = bases
                    .entry(*track_id)
                    .or_insert_with(|| engine.get_track_volume(*track_id));
                if !entry.is_finite() {
                    return false;
                }
            }
        }
        multipliers.into_iter().all(|(track_id, multiplier)| {
            let Some(base) = bases.get(&track_id).copied() else {
                return false;
            };
            engine.set_track_volume(track_id, (base * multiplier).clamp(0.0, 2.0))
        })
    }

    /// Applies a user-facing member fader edit while preserving the stack
    /// multiplier. This prevents the next stack edit from snapping the fader
    /// back to an older baseline.
    pub fn set_track_volume_with_stack(&self, track_id: u32, value: f32) -> bool {
        if !value.is_finite() || !(0.0..=2.0).contains(&value) {
            return false;
        }
        let (has_stack, multiplier) = self
            .track_stacks
            .lock()
            .ok()
            .map(|stacks| {
                let matching = stacks
                    .iter()
                    .filter(|stack| stack.member_track_ids.contains(&track_id))
                    .collect::<Vec<_>>();
                (
                    !matching.is_empty(),
                    matching
                        .iter()
                        .fold(1.0f32, |product, stack| product * stack.master_gain),
                )
            })
            .unwrap_or((false, 1.0));
        if !has_stack {
            return self
                .engine_opt()
                .is_some_and(|engine| engine.set_track_volume(track_id, value));
        }
        let Ok(mut bases) = self.track_stack_base_volumes.lock() else {
            return false;
        };
        let base = if multiplier > f32::EPSILON {
            value / multiplier
        } else {
            // A zero stack gain is effectively a mute. Preserve the user's
            // member-fader edit as the future baseline instead of losing it.
            value
        };
        if !base.is_finite() {
            return false;
        }
        let base = base.clamp(0.0, 2.0);
        bases.insert(track_id, base);
        let applied = (base * multiplier).clamp(0.0, 2.0);
        self.engine_opt()
            .is_some_and(|engine| engine.set_track_volume(track_id, applied))
    }

    /// Creates or replaces one Track Stack atomically on the control plane.
    /// Native member faders are republished from their stable baselines after
    /// the metadata change, so callers never observe a half-written stack.
    pub fn upsert_track_stack(
        &self,
        id: u32,
        name: &str,
        member_track_ids: &[u32],
        master_gain: f32,
        collapsed: bool,
    ) -> bool {
        if id == 0
            || name.trim().is_empty()
            || name.len() > 256
            || !master_gain.is_finite()
            || !(0.0..=2.0).contains(&master_gain)
            || member_track_ids.is_empty()
        {
            return false;
        }
        let mut members = HashSet::with_capacity(member_track_ids.len());
        if member_track_ids
            .iter()
            .any(|track_id| *track_id == 0 || !members.insert(*track_id))
        {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let previous = stacks.clone();
        let candidate = TrackStackContract {
            id,
            name: name.trim().chars().take(256).collect(),
            member_track_ids: {
                let mut ids = member_track_ids.to_vec();
                ids.sort_unstable();
                ids
            },
            master_gain,
            collapsed,
        };
        if let Some(existing) = stacks.iter_mut().find(|stack| stack.id == id) {
            *existing = candidate;
        } else {
            stacks.push(candidate);
            stacks.sort_by_key(|stack| stack.id);
        }
        drop(stacks);
        if let (Ok(mut bases), Some(engine)) =
            (self.track_stack_base_volumes.lock(), self.engine_opt())
        {
            for track_id in member_track_ids {
                bases
                    .entry(*track_id)
                    .or_insert_with(|| engine.get_track_volume(*track_id));
            }
        }
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            *stacks = previous;
        }
        let _ = self.apply_track_stack_gains();
        false
    }

    pub fn set_track_stack_gain(&self, id: u32, master_gain: f32) -> bool {
        if !master_gain.is_finite() || !(0.0..=2.0).contains(&master_gain) {
            return false;
        }
        let previous_gain = {
            let Ok(mut stacks) = self.track_stacks.lock() else {
                return false;
            };
            let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) else {
                return false;
            };
            let previous_gain = stack.master_gain;
            stack.master_gain = master_gain;
            previous_gain
        };
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            if let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) {
                stack.master_gain = previous_gain;
            }
        }
        let _ = self.apply_track_stack_gains();
        false
    }

    pub fn set_track_stack_collapsed(&self, id: u32, collapsed: bool) -> bool {
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) else {
            return false;
        };
        stack.collapsed = collapsed;
        true
    }

    pub fn delete_track_stack(&self, id: u32) -> bool {
        if id == 0 {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let before = stacks.len();
        stacks.retain(|stack| stack.id != id);
        if stacks.len() == before {
            return false;
        }
        drop(stacks);
        self.apply_track_stack_gains()
    }

    pub fn add_track_stack_member(&self, stack_id: u32, track_id: u32) -> bool {
        if stack_id == 0 || track_id == 0 {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Some(stack) = stacks.iter_mut().find(|stack| stack.id == stack_id) else {
            return false;
        };
        if stack.member_track_ids.contains(&track_id) {
            return true;
        }
        stack.member_track_ids.push(track_id);
        stack.member_track_ids.sort_unstable();
        drop(stacks);
        if let (Ok(mut bases), Some(engine)) =
            (self.track_stack_base_volumes.lock(), self.engine_opt())
        {
            bases
                .entry(track_id)
                .or_insert_with(|| engine.get_track_volume(track_id));
        }
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            if let Some(stack) = stacks.iter_mut().find(|stack| stack.id == stack_id) {
                stack.member_track_ids.retain(|member| *member != track_id);
            }
        }
        false
    }
}
