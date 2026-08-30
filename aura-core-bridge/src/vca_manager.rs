pub struct VcaGroupRust {
    pub id: u32,
    pub gain: f32,
}

pub struct VcaOrchestrator {
    pub resolved_track_gains: Vec<f32>,
    pub vca_groups: std::collections::HashMap<u32, VcaGroupRust>,
    pub track_to_groups: std::collections::HashMap<u32, Vec<u32>>,
}

#[cfg(test)]
mod tests {
    use super::VcaOrchestrator;

    #[test]
    fn vca_gain_and_membership_are_editable_without_stale_assignments() {
        let mut vca = VcaOrchestrator::new();
        vca.add_group(1, 1.0);
        vca.assign_track_to_group(4, 1);
        assert_eq!(vca.group_members(1), vec![4]);
        assert!(vca.set_group_gain(1, 0.5));
        assert!((vca.resolved_track_gains[4] - 0.5).abs() < 1e-6);
        assert!(vca.unassign_track_from_group(4, 1));
        assert!(vca.group_members(1).is_empty());
        assert!(vca.audit_vca_manager());
    }

    #[test]
    fn resolved_vca_gain_is_applied_to_audio_block() {
        let mut vca = VcaOrchestrator::new();
        vca.add_group(1, 0.5);
        vca.assign_track_to_group(3, 1);
        let (left, right) = vca.apply_track_gain(3, &[1.0, -1.0], &[0.5, -0.5]).unwrap();
        assert_eq!(left, vec![0.5, -0.5]);
        assert_eq!(right, vec![0.25, -0.25]);
        assert!(vca.apply_track_gain(3, &[f32::NAN], &[0.0]).is_none());
    }
}

impl Default for VcaOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl VcaOrchestrator {
    pub fn new() -> Self {
        Self {
            resolved_track_gains: vec![1.0; 2048],
            vca_groups: std::collections::HashMap::new(),
            track_to_groups: std::collections::HashMap::new(),
        }
    }

    /// INDUSTRIAL: Resolves the hierarchical VCA gains with absolute gain precision and mixing sovereignty.
    pub fn resolve_hierarchy(&mut self) {
        self.resolved_track_gains.fill(1.0);
        for (track_id, groups) in &self.track_to_groups {
            let Some(slot) = self.resolved_track_gains.get_mut(*track_id as usize) else {
                continue;
            };
            let mut gain = 1.0f32;
            for group_id in groups {
                if let Some(group) = self.vca_groups.get(group_id) {
                    gain *= group.gain;
                }
            }
            *slot = if gain.is_finite() {
                gain.clamp(0.0, 8.0)
            } else {
                1.0
            };
        }
    }

    /// INDUSTRIAL: Adds a VCA group with absolute memory precision and mixing sovereignty.
    pub fn add_group(&mut self, id: u32, gain: f32) {
        if id == 0 || (self.vca_groups.len() >= 65_536 && !self.vca_groups.contains_key(&id)) {
            return;
        }
        let safe_gain = if gain.is_finite() {
            gain.clamp(0.0, 8.0)
        } else {
            1.0
        };
        self.vca_groups.insert(
            id,
            VcaGroupRust {
                id,
                gain: safe_gain,
            },
        );
    }

    pub fn remove_group(&mut self, id: u32) -> bool {
        if self.vca_groups.remove(&id).is_none() { return false; }
        for groups in self.track_to_groups.values_mut() { groups.retain(|group| *group != id); }
        self.resolve_hierarchy();
        true
    }

    pub fn set_group_gain(&mut self, id: u32, gain: f32) -> bool {
        if !gain.is_finite() || !(0.0..=8.0).contains(&gain) { return false; }
        let Some(group) = self.vca_groups.get_mut(&id) else { return false; };
        group.gain = gain;
        self.resolve_hierarchy();
        true
    }

    pub fn assign_track_to_group(&mut self, track_id: u32, group_id: u32) {
        if track_id as usize >= self.resolved_track_gains.len()
            || !self.vca_groups.contains_key(&group_id)
        {
            return;
        }
        let groups = self.track_to_groups.entry(track_id).or_default();
        if !groups.contains(&group_id) {
            groups.push(group_id);
            groups.sort_unstable();
        }
        self.resolve_hierarchy();
    }

    pub fn unassign_track_from_group(&mut self, track_id: u32, group_id: u32) -> bool {
        let Some(groups) = self.track_to_groups.get_mut(&track_id) else { return false; };
        let before = groups.len();
        groups.retain(|id| *id != group_id);
        let changed = before != groups.len();
        let empty = groups.is_empty();
        if empty { self.track_to_groups.remove(&track_id); }
        if changed { self.resolve_hierarchy(); }
        changed
    }

    pub fn group_members(&self, group_id: u32) -> Vec<u32> {
        let mut members: Vec<_> = self.track_to_groups.iter().filter(|(_, groups)| groups.contains(&group_id)).map(|(track, _)| *track).collect();
        members.sort_unstable();
        members
    }

    /// Applies the resolved VCA gain to a stereo block without mutating the
    /// caller's source buffers.  This is the audio-thread-facing companion to
    /// the control-plane hierarchy resolver.
    pub fn apply_track_gain(&self, track_id: u32, left: &[f32], right: &[f32]) -> Option<(Vec<f32>, Vec<f32>)> {
        if left.len() != right.len() || left.len() > 16_000_000 || left.iter().chain(right).any(|sample| !sample.is_finite()) { return None; }
        let gain = self.resolved_track_gains.get(track_id as usize).copied()?.clamp(0.0, 8.0);
        Some((left.iter().map(|sample| sample * gain).collect(), right.iter().map(|sample| sample * gain).collect()))
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide VCA state.
    pub fn audit_vca_manager(&self) -> bool {
        self.vca_groups.iter().all(|(id, group)| {
            *id == group.id && group.gain.is_finite() && (0.0..=8.0).contains(&group.gain)
        }) && self.track_to_groups.iter().all(|(track, groups)| {
            (*track as usize) < self.resolved_track_gains.len()
                && groups
                    .iter()
                    .all(|group| self.vca_groups.contains_key(group))
        }) && self
            .resolved_track_gains
            .iter()
            .all(|gain| gain.is_finite() && (0.0..=8.0).contains(gain))
            && self.track_to_groups.values().all(|groups| groups.windows(2).all(|pair| pair[0] < pair[1]))
    }
}
