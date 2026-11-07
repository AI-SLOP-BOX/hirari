use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq)]
pub struct PluginSnapshotState {
    pub track_id: u32,
    pub plugin_index: u32,
    pub parameters: Vec<f32>,
    pub bypassed: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MixSnapshot {
    pub name: String,
    pub parameter_states: HashMap<u32, f32>,
    #[serde(default)]
    pub plugin_states: Vec<PluginSnapshotState>,
    #[serde(default)]
    pub routing_state: String,
}

pub struct SnapshotOrchestrator {
    pub snapshots: Vec<MixSnapshot>,
}

impl Default for SnapshotOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotOrchestrator {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
        }
    }

    /// INDUSTRIAL: Captures the current mixer state.
    pub fn take_snapshot(&mut self, name: &str, states: HashMap<u32, f32>) {
        if name.trim().is_empty()
            || name.len() > 128
            || states.len() > 65_536
            || states.values().any(|value| !value.is_finite())
        {
            return;
        }
        if let Some(existing) = self
            .snapshots
            .iter_mut()
            .find(|snapshot| snapshot.name.eq_ignore_ascii_case(name.trim()))
        {
            existing.parameter_states = states;
            return;
        }
        self.snapshots.push(MixSnapshot {
            name: name.trim().to_owned(),
            parameter_states: states,
            plugin_states: Vec::new(),
            routing_state: String::new(),
        });
    }

    pub fn take_snapshot_with_plugins(
        &mut self,
        name: &str,
        states: HashMap<u32, f32>,
        plugin_states: Vec<PluginSnapshotState>,
    ) {
        if plugin_states.len() > 65_536
            || plugin_states
                .iter()
                .any(|state| !validate_plugin_state(state))
        {
            return;
        }
        self.take_snapshot(name, states);
        if let Some(snapshot) = self
            .snapshots
            .iter_mut()
            .find(|snapshot| snapshot.name.eq_ignore_ascii_case(name.trim()))
        {
            snapshot.plugin_states = plugin_states;
        }
    }

    pub fn take_snapshot_with_plugins_and_routing(
        &mut self,
        name: &str,
        states: HashMap<u32, f32>,
        plugin_states: Vec<PluginSnapshotState>,
        routing_state: String,
    ) {
        if routing_state.len() > 1_048_576 || routing_state.contains('\0') {
            return;
        }
        self.take_snapshot_with_plugins(name, states, plugin_states);
        if let Some(snapshot) = self
            .snapshots
            .iter_mut()
            .find(|snapshot| snapshot.name.eq_ignore_ascii_case(name.trim()))
        {
            snapshot.routing_state = routing_state;
        }
    }

    /// INDUSTRIAL: Compares two snapshots and returns the diff.
    pub fn diff_snapshots(&self, idx1: usize, idx2: usize) -> Vec<(u32, f32, f32)> {
        if idx1 >= self.snapshots.len() || idx2 >= self.snapshots.len() {
            return Vec::new();
        }

        let s1 = &self.snapshots[idx1];
        let s2 = &self.snapshots[idx2];
        let mut diff = Vec::new();

        let mut ids = s1
            .parameter_states
            .keys()
            .chain(s2.parameter_states.keys())
            .copied()
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let val1 = s1.parameter_states.get(&id).copied().unwrap_or(0.0);
            let val2 = s2.parameter_states.get(&id).copied().unwrap_or(0.0);
            if (val1 - val2).abs() > 1e-6 {
                diff.push((id, val1, val2));
            }
        }

        diff
    }

    /// Returns named snapshots in deterministic order for A/B comparison UI.
    pub fn list_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .snapshots
            .iter()
            .map(|snapshot| snapshot.name.clone())
            .collect();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names
    }

    pub fn rename_snapshot(&mut self, old: &str, new: &str) -> bool {
        let new = new.trim();
        if new.is_empty() || new.len() > 128 || new.contains('\0') {
            return false;
        }
        let Some(index) = self
            .snapshots
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(old.trim()))
        else {
            return false;
        };
        if self.snapshots[index].name.eq_ignore_ascii_case(new) {
            return true;
        }
        if self
            .snapshots
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(new))
        {
            return false;
        }
        self.snapshots[index].name = new.to_owned();
        true
    }

    pub fn remove_snapshot(&mut self, name: &str) -> bool {
        let before = self.snapshots.len();
        self.snapshots
            .retain(|snapshot| !snapshot.name.eq_ignore_ascii_case(name.trim()));
        before != self.snapshots.len()
    }

    /// Restores a named mixer snapshot into caller-owned state without
    /// exposing internal mutable references.
    pub fn restore_snapshot(
        &self,
        name: &str,
    ) -> Option<(HashMap<u32, f32>, Vec<PluginSnapshotState>, String)> {
        let snapshot = self
            .snapshots
            .iter()
            .find(|snapshot| snapshot.name.eq_ignore_ascii_case(name.trim()))?;
        Some((
            snapshot.parameter_states.clone(),
            snapshot.plugin_states.clone(),
            snapshot.routing_state.clone(),
        ))
    }

    /// Compares plugin bypass/parameter state between two snapshots.
    pub fn diff_plugins(&self, idx1: usize, idx2: usize) -> Vec<PluginSnapshotState> {
        let (Some(left), Some(right)) = (self.snapshots.get(idx1), self.snapshots.get(idx2)) else {
            return Vec::new();
        };
        let mut keys: Vec<_> = left
            .plugin_states
            .iter()
            .map(|s| (s.track_id, s.plugin_index))
            .chain(
                right
                    .plugin_states
                    .iter()
                    .map(|s| (s.track_id, s.plugin_index)),
            )
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys.into_iter()
            .filter_map(|(track_id, plugin_index)| {
                let a = left
                    .plugin_states
                    .iter()
                    .find(|s| s.track_id == track_id && s.plugin_index == plugin_index);
                let b = right
                    .plugin_states
                    .iter()
                    .find(|s| s.track_id == track_id && s.plugin_index == plugin_index);
                if a == b {
                    None
                } else {
                    b.or(a).cloned()
                }
            })
            .collect()
    }

    pub fn routing_changed(&self, idx1: usize, idx2: usize) -> Option<bool> {
        let left = self.snapshots.get(idx1)?;
        let right = self.snapshots.get(idx2)?;
        Some(left.routing_state != right.routing_state)
    }

    pub fn audit(&self) -> bool {
        self.snapshots.len() <= 65_536
            && self.snapshots.iter().enumerate().all(|(i, snapshot)| {
                !snapshot.name.trim().is_empty()
                    && snapshot.name.len() <= 128
                    && !snapshot.name.contains('\0')
                    && snapshot.parameter_states.len() <= 65_536
                    && snapshot.parameter_states.keys().all(|id| *id != 0)
                    && snapshot.parameter_states.values().all(|v| v.is_finite())
                    && snapshot.plugin_states.len() <= 65_536
                    && snapshot.plugin_states.iter().all(validate_plugin_state)
                    && snapshot.routing_state.len() <= 1_048_576
                    && !snapshot.routing_state.contains('\0')
                    && self.snapshots[..i]
                        .iter()
                        .all(|previous| !previous.name.eq_ignore_ascii_case(&snapshot.name))
            })
    }
}

fn validate_plugin_state(state: &PluginSnapshotState) -> bool {
    state.track_id != 0
        && state.parameters.len() <= 65_536
        && state.parameters.iter().all(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::{PluginSnapshotState, SnapshotOrchestrator};
    use std::collections::HashMap;

    #[test]
    fn replacing_name_is_atomic_and_diff_includes_removed_parameters() {
        let mut snapshots = SnapshotOrchestrator::new();
        snapshots.take_snapshot("A", HashMap::from([(1, 0.5), (2, 1.0)]));
        snapshots.take_snapshot("A", HashMap::from([(1, 0.25)]));
        snapshots.take_snapshot("B", HashMap::from([(1, 0.25), (3, 0.8)]));
        assert_eq!(snapshots.snapshots.len(), 2);
        assert_eq!(snapshots.diff_snapshots(0, 1), vec![(3, 0.0, 0.8)]);
    }

    #[test]
    fn rejects_non_finite_state_without_creating_snapshot() {
        let mut snapshots = SnapshotOrchestrator::new();
        snapshots.take_snapshot("bad", HashMap::from([(1, f32::NAN)]));
        assert!(snapshots.snapshots.is_empty());
    }

    #[test]
    fn plugin_parameter_and_bypass_state_survives_named_replacement() {
        let mut snapshots = SnapshotOrchestrator::new();
        snapshots.take_snapshot_with_plugins(
            "mix",
            HashMap::from([(4, 0.75)]),
            vec![PluginSnapshotState {
                track_id: 1,
                plugin_index: 0,
                parameters: vec![0.2, 0.8],
                bypassed: true,
            }],
        );
        snapshots.take_snapshot_with_plugins(
            "mix",
            HashMap::from([(4, 0.9)]),
            vec![PluginSnapshotState {
                track_id: 1,
                plugin_index: 0,
                parameters: vec![0.4, 0.6],
                bypassed: false,
            }],
        );
        let state = &snapshots.snapshots[0].plugin_states[0];
        assert_eq!(state.parameters, vec![0.4, 0.6]);
        assert!(!state.bypassed);
    }
}
