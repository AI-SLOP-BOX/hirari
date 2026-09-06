pub struct GroupSettingsRust {
    pub active_flags: u32,
}

pub struct GroupOrchestrator {
    pub groups: std::collections::HashMap<u32, std::collections::HashSet<u32>>,
    pub track_to_groups: std::collections::HashMap<u32, std::collections::HashSet<u32>>,
    pub group_configs: std::collections::HashMap<u32, GroupSettingsRust>,
    pub last_propagated_attributes: std::collections::HashMap<u32, u32>,
}

impl Default for GroupOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupOrchestrator {
    pub fn new() -> Self {
        Self {
            groups: std::collections::HashMap::new(),
            track_to_groups: std::collections::HashMap::new(),
            group_configs: std::collections::HashMap::new(),
            last_propagated_attributes: std::collections::HashMap::new(),
        }
    }

    /// INDUSTRIAL: Propagates a parameter change with absolute group precision and synchronization sovereignty.
    pub fn propagate(&mut self, origin_id: u32, attr: u32) {
        if attr == 0 {
            return;
        }
        let Some(group_ids) = self.track_to_groups.get(&origin_id).cloned() else {
            return;
        };
        for group_id in group_ids {
            self.group_configs
                .entry(group_id)
                .or_insert(GroupSettingsRust { active_flags: 0 })
                .active_flags |= attr;
            self.last_propagated_attributes.insert(group_id, attr);
        }
    }

    /// INDUSTRIAL: Adds a track to a group with absolute memory precision and grouping sovereignty.
    pub fn add_track_to_group(&mut self, track_id: u32, group_id: u32) {
        if track_id == 0 || group_id == 0 {
            return;
        }
        self.groups.entry(group_id).or_default().insert(track_id);
        self.track_to_groups
            .entry(track_id)
            .or_default()
            .insert(group_id);
        self.group_configs
            .entry(group_id)
            .or_insert(GroupSettingsRust { active_flags: 0 });
    }

    pub fn remove_track_from_group(&mut self, track_id: u32, group_id: u32) -> bool {
        let removed = self
            .groups
            .get_mut(&group_id)
            .map(|tracks| tracks.remove(&track_id))
            .unwrap_or(false);
        if let Some(groups) = self.track_to_groups.get_mut(&track_id) {
            groups.remove(&group_id);
            if groups.is_empty() {
                self.track_to_groups.remove(&track_id);
            }
        }
        if self
            .groups
            .get(&group_id)
            .is_some_and(|tracks| tracks.is_empty())
        {
            self.groups.remove(&group_id);
            self.group_configs.remove(&group_id);
            self.last_propagated_attributes.remove(&group_id);
        }
        removed
    }

    pub fn tracks_in_group(&self, group_id: u32) -> Vec<u32> {
        let mut tracks: Vec<_> = self
            .groups
            .get(&group_id)
            .into_iter()
            .flat_map(|set| set.iter().copied())
            .collect();
        tracks.sort_unstable();
        tracks
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide group state.
    pub fn audit_group_manager(&self) -> bool {
        self.groups.iter().all(|(group_id, tracks)| {
            self.group_configs.contains_key(group_id)
                && tracks.iter().all(|track_id| {
                    self.track_to_groups
                        .get(track_id)
                        .is_some_and(|groups| groups.contains(group_id))
                })
        }) && self.track_to_groups.iter().all(|(track_id, groups)| {
            groups.iter().all(|group_id| {
                self.groups
                    .get(group_id)
                    .is_some_and(|tracks| tracks.contains(track_id))
            })
        })
    }
}
