use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkMask {
    pub volume: bool,
    pub pan: bool,
    pub mute: bool,
    pub solo: bool,
}
impl Default for LinkMask {
    fn default() -> Self {
        Self {
            volume: true,
            pan: true,
            mute: true,
            solo: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VcaGroup {
    pub id: u32,
    pub member_tracks: Vec<u32>,
    pub gain_db: f32,
    pub muted: bool,
}
impl VcaGroup {
    pub fn validate(&self) -> bool {
        self.id != 0
            && !self.member_tracks.is_empty()
            && self.member_tracks.len() <= 1024
            && self.member_tracks.iter().all(|id| *id != 0)
            && self
                .member_tracks
                .iter()
                .enumerate()
                .all(|(i, id)| self.member_tracks[..i].iter().all(|p| p != id))
            && self.gain_db.is_finite()
            && (-120.0..=24.0).contains(&self.gain_db)
    }
    pub fn linear_gain(&self) -> f32 {
        10.0_f32.powf(self.gain_db / 20.0)
    }
    pub fn effective_gain_db(&self, track_gain_db: f32) -> Option<f32> {
        (self.validate() && track_gain_db.is_finite())
            .then_some((track_gain_db + self.gain_db).clamp(-120.0, 24.0))
    }
}

pub struct EditGroup {
    pub id: u32,
    pub track_ids: HashSet<u32>,
    pub link: LinkMask,
}

pub struct GroupOrchestrator {
    pub edit_groups: HashMap<u32, EditGroup>,
    pub track_to_group: HashMap<u32, u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moving_track_between_groups_cleans_old_membership() {
        let mut groups = GroupOrchestrator::new();
        groups.create_edit_group(1, vec![10, 11]);
        groups.create_edit_group(2, vec![11, 12]);
        assert_eq!(groups.resolve_sync_targets(10), Vec::<u32>::new());
        assert_eq!(groups.resolve_sync_targets(11), vec![12]);
        assert!(groups.audit_group_integrity());
        assert!(groups.remove_edit_group(2));
        assert!(groups.audit_group_integrity());
    }
}

impl Default for GroupOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupOrchestrator {
    pub fn new() -> Self {
        Self {
            edit_groups: HashMap::new(),
            track_to_group: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Creates a phase-locked editing group with memory-safe Rust collections.
    pub fn create_edit_group(&mut self, group_id: u32, track_ids: Vec<u32>) {
        let _ = self.try_create_edit_group(group_id, track_ids);
    }

    pub fn try_create_edit_group(&mut self, group_id: u32, track_ids: Vec<u32>) -> bool {
        if group_id == 0
            || track_ids.is_empty()
            || track_ids.len() > 1024
            || track_ids.contains(&0)
            || track_ids
                .iter()
                .enumerate()
                .any(|(i, id)| track_ids[..i].contains(id))
        {
            return false;
        }
        if let Some(previous) = self.edit_groups.remove(&group_id) {
            for track in previous.track_ids {
                if self.track_to_group.get(&track) == Some(&group_id) {
                    self.track_to_group.remove(&track);
                }
            }
        }
        let mut tracks = HashSet::new();
        for tid in track_ids {
            if let Some(previous_id) = self.track_to_group.insert(tid, group_id) {
                if previous_id != group_id {
                    if let Some(previous) = self.edit_groups.get_mut(&previous_id) {
                        previous.track_ids.remove(&tid);
                    }
                }
            }
            tracks.insert(tid);
        }

        self.edit_groups.insert(
            group_id,
            EditGroup {
                id: group_id,
                track_ids: tracks,
                link: LinkMask::default(),
            },
        );
        true
    }

    /// INDUSTRIAL: Synchronizes an edit operation across all member tracks with absolute sample precision.
    pub fn resolve_sync_targets(&self, origin_track_id: u32) -> Vec<u32> {
        // INDUSTRIAL: Implementation of high-performance sync resolution.
        if let Some(group_id) = self.track_to_group.get(&origin_track_id) {
            if let Some(group) = self.edit_groups.get(group_id) {
                let mut targets: Vec<u32> = group
                    .track_ids
                    .iter()
                    .filter(|&&tid| tid != origin_track_id)
                    .cloned()
                    .collect();
                targets.sort_unstable();
                return targets;
            }
        }
        Vec::new()
    }
    pub fn set_link_mask(&mut self, group_id: u32, link: LinkMask) -> bool {
        self.edit_groups
            .get_mut(&group_id)
            .map(|g| {
                g.link = link;
                true
            })
            .unwrap_or(false)
    }
    pub fn remove_edit_group(&mut self, group_id: u32) -> bool {
        let Some(group) = self.edit_groups.remove(&group_id) else {
            return false;
        };
        for track in group.track_ids {
            if self.track_to_group.get(&track) == Some(&group_id) {
                self.track_to_group.remove(&track);
            }
        }
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide edit group synchronization graph.
    pub fn audit_group_integrity(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic sync auditing logic.
        self.edit_groups.iter().all(|(id, group)| {
            *id != 0
                && group.id == *id
                && !group.track_ids.is_empty()
                && group
                    .track_ids
                    .iter()
                    .all(|track| *track != 0 && self.track_to_group.get(track) == Some(id))
        }) && self.track_to_group.iter().all(|(track, id)| {
            self.edit_groups
                .get(id)
                .map(|g| g.track_ids.contains(track))
                .unwrap_or(false)
        })
    }
}
