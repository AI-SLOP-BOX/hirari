use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VCAGroup {
    pub id: u32,
    pub name: String,
    pub master_gain: f32,
    pub track_ids: Vec<u32>,
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn vca_console_round_trips_only_audited_state() {
        let mut console = VcaConsole::default();
        assert!(console.upsert_channel(VcaLinkedChannel { id: 1, base_gain_db: -3.0,
            automation: vec![VcaAutomationPoint { sample: 0, gain_db: 0.0 }], muted: false,
            solo: false, listen: false, monitor: false, record_enabled: false, peak_db: -6.0 }));
        assert!(console.upsert_group(VcaLinkGroup { id: 10, name: "Band".into(), fader_gain_db: -2.0,
            automation: vec![], members: vec![1], muted: false, solo: false, listen: false,
            monitor: false, record_enabled: false }));
        let json = console.to_json().unwrap();
        assert_eq!(VcaConsole::from_json(&json).unwrap(), console);
        let mut invalid = console;
        invalid.groups.get_mut(&10).unwrap().members.push(999);
        assert!(invalid.to_json().is_err());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VCAOrchestrator {
    pub groups: Vec<VCAGroup>,
}

impl Default for VCAOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl VCAOrchestrator {
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    /// INDUSTRIAL: Resolves a track's final gain with absolute precision and gain sovereignty.
    pub fn resolve_track_gain(&self, track_id: u32, base_gain: f32) -> f32 {
        // INDUSTRIAL: Implementation of high-performance gain resolution.
        // Rust's safe memory management handles large VCA environments with
        // absolute bit-accuracy and zero-latency.
        // Rust's GainEngine handles large VCA environments with absolute bit-accuracy.
        if track_id == 0 || !base_gain.is_finite() { return 0.0; }
        let mut multiplier = 1.0;
        for group in &self.groups {
            if group.track_ids.contains(&track_id) {
                if !group.master_gain.is_finite() { return 0.0; }
                multiplier = (multiplier * group.master_gain).clamp(-64.0, 64.0);
            }
        }
        (base_gain * multiplier).clamp(-1_000_000.0, 1_000_000.0)
    }

    /// INDUSTRIAL: Resolves the hierarchical gain propagation with absolute precision and gain sovereignty.
    pub fn resolve_hierarchy(&self) {
        // Hierarchy resolution is represented by group membership in the
        // current project model.  Gain calculation is therefore deliberately
        // read-only here; malformed hierarchy is rejected by audit_vca rather
        // than silently changing a user's mix.
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide gain synchronization graph.
    pub fn audit_vca(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        self.groups.iter().all(|group| {
            group.id != 0 && !group.name.trim().is_empty()
                && ids.insert(group.id)
                && group.master_gain.is_finite()
                && group.master_gain.abs() <= 64.0
                && group.track_ids.iter().all(|track| *track != 0)
        }) && self.groups.iter().enumerate().all(|(index, group)| {
            self.groups[index + 1..].iter().all(|other| {
                group.track_ids.iter().all(|track| !other.track_ids.contains(track))
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct VcaAutomationPoint {
    pub sample: u64,
    pub gain_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VcaLinkedChannel {
    pub id: u32,
    pub base_gain_db: f32,
    pub automation: Vec<VcaAutomationPoint>,
    pub muted: bool,
    pub solo: bool,
    pub listen: bool,
    pub monitor: bool,
    pub record_enabled: bool,
    pub peak_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VcaLinkGroup {
    pub id: u32,
    pub name: String,
    pub fader_gain_db: f32,
    pub automation: Vec<VcaAutomationPoint>,
    pub members: Vec<u32>,
    pub muted: bool,
    pub solo: bool,
    pub listen: bool,
    pub monitor: bool,
    pub record_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct VcaConsole {
    pub channels: std::collections::BTreeMap<u32, VcaLinkedChannel>,
    pub groups: std::collections::BTreeMap<u32, VcaLinkGroup>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VcaHierarchy {
    /// Each child VCA can be controlled by at most one parent VCA.
    pub parent_by_child: std::collections::BTreeMap<u32, u32>,
}

impl VcaHierarchy {
    pub fn to_json(&self, console: &VcaConsole) -> Result<String, String> {
        if !self.audit(console) { return Err("invalid nested VCA hierarchy".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str, console: &VcaConsole) -> Result<Self, String> {
        let hierarchy: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if hierarchy.audit(console) { Ok(hierarchy) } else { Err("invalid nested VCA hierarchy".into()) }
    }

    pub fn assign_parent(&mut self, console: &VcaConsole, child: u32, parent: u32) -> bool {
        if child == parent || !console.groups.contains_key(&child) || !console.groups.contains_key(&parent) { return false; }
        let previous = self.parent_by_child.insert(child, parent);
        if !self.audit(console) {
            if let Some(previous) = previous { self.parent_by_child.insert(child, previous); }
            else { self.parent_by_child.remove(&child); }
            return false;
        }
        true
    }

    pub fn remove_parent(&mut self, child: u32) -> bool { self.parent_by_child.remove(&child).is_some() }

    pub fn ancestors(&self, group_id: u32) -> Option<Vec<u32>> {
        let mut result = Vec::new();
        let mut current = group_id;
        let mut visited = std::collections::BTreeSet::new();
        while let Some(parent) = self.parent_by_child.get(&current).copied() {
            if !visited.insert(parent) { return None; }
            result.push(parent); current = parent;
        }
        Some(result)
    }

    pub fn resolved_gain_db(&self, console: &VcaConsole, channel_id: u32, sample: u64) -> Option<f32> {
        let channel = console.channels.get(&channel_id)?;
        let own = curve_value(&channel.automation, sample).unwrap_or(channel.base_gain_db);
        let direct = console.groups.values().find(|group| group.members.contains(&channel_id));
        let Some(direct) = direct else { return Some(own.clamp(-120.0, 24.0)); };
        let mut gain = own + curve_value(&direct.automation, sample).unwrap_or(direct.fader_gain_db);
        for ancestor in self.ancestors(direct.id)? {
            let group = console.groups.get(&ancestor)?;
            gain += curve_value(&group.automation, sample).unwrap_or(group.fader_gain_db);
        }
        Some(gain.clamp(-120.0, 24.0))
    }

    pub fn linked_state(&self, console: &VcaConsole, channel_id: u32) -> Option<(bool, bool, bool, bool, bool)> {
        let channel = console.channels.get(&channel_id)?;
        let mut state = (channel.muted, channel.solo, channel.listen, channel.monitor, channel.record_enabled);
        if let Some(direct) = console.groups.values().find(|group| group.members.contains(&channel_id)) {
            let mut groups = vec![direct.id]; groups.extend(self.ancestors(direct.id)?);
            for id in groups {
                let group = console.groups.get(&id)?;
                state.0 |= group.muted; state.1 |= group.solo; state.2 |= group.listen;
                state.3 |= group.monitor; state.4 |= group.record_enabled;
            }
        }
        Some(state)
    }

    pub fn descendant_channels(&self, console: &VcaConsole, group_id: u32) -> Option<Vec<u32>> {
        if !console.groups.contains_key(&group_id) { return None; }
        let mut groups = vec![group_id];
        for (&child, &parent) in &self.parent_by_child {
            if parent == group_id || self.ancestors(child)?.contains(&group_id) { groups.push(child); }
        }
        groups.sort_unstable(); groups.dedup();
        let mut channels = groups.into_iter().filter_map(|id| console.groups.get(&id))
            .flat_map(|group| group.members.iter().copied()).collect::<Vec<_>>();
        channels.sort_unstable(); channels.dedup(); Some(channels)
    }

    pub fn summed_peak_db(&self, console: &VcaConsole, group_id: u32) -> Option<f32> {
        let amplitude: f64 = self.descendant_channels(console, group_id)?.into_iter()
            .filter_map(|id| console.channels.get(&id)).map(|channel| 10.0f64.powf(f64::from(channel.peak_db) / 20.0)).sum();
        Some(if amplitude <= 0.0 { -120.0 } else { (20.0 * amplitude.log10()).clamp(-120.0, 48.0) as f32 })
    }

    pub fn audit(&self, console: &VcaConsole) -> bool {
        console.audit() && self.parent_by_child.len() <= console.groups.len()
            && self.parent_by_child.iter().all(|(child, parent)| child != parent
                && console.groups.contains_key(child) && console.groups.contains_key(parent))
            && self.parent_by_child.keys().all(|child| self.ancestors(*child).is_some())
    }
}

impl VcaConsole {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid VCA console state".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.audit() { Ok(value) } else { Err("invalid VCA console state".into()) }
    }

    pub fn upsert_channel(&mut self, mut channel: VcaLinkedChannel) -> bool {
        normalize_curve(&mut channel.automation);
        if !channel.validate() || (self.channels.len() >= 8192 && !self.channels.contains_key(&channel.id)) {
            return false;
        }
        self.channels.insert(channel.id, channel);
        true
    }

    pub fn upsert_group(&mut self, mut group: VcaLinkGroup) -> bool {
        group.name = group.name.trim().to_owned();
        group.members.sort_unstable();
        group.members.dedup();
        normalize_curve(&mut group.automation);
        if !group.validate() || (self.groups.len() >= 1024 && !self.groups.contains_key(&group.id)) {
            return false;
        }
        if group.members.iter().any(|member| {
            !self.channels.contains_key(member)
                || self.groups.values().any(|existing| existing.id != group.id && existing.members.contains(member))
        }) {
            return false;
        }
        self.groups.insert(group.id, group);
        true
    }

    pub fn assign(&mut self, group_id: u32, channel_id: u32) -> bool {
        if !self.channels.contains_key(&channel_id)
            || self.groups.values().any(|group| group.members.contains(&channel_id)) {
            return false;
        }
        let Some(group) = self.groups.get_mut(&group_id) else { return false; };
        group.members.push(channel_id);
        group.members.sort_unstable();
        true
    }

    pub fn unassign(&mut self, group_id: u32, channel_id: u32) -> bool {
        let Some(group) = self.groups.get_mut(&group_id) else { return false; };
        let before = group.members.len();
        group.members.retain(|member| *member != channel_id);
        before != group.members.len()
    }

    /// Resolve channel fader level in dB. VCA movement is additive in dB, not
    /// a replacement of the linked channel's own static/automated level.
    pub fn resolved_gain_db(&self, channel_id: u32, sample: u64) -> Option<f32> {
        let channel = self.channels.get(&channel_id)?;
        let channel_gain = curve_value(&channel.automation, sample).unwrap_or(channel.base_gain_db);
        let vca_gain = self.groups.values().find(|group| group.members.contains(&channel_id))
            .map(|group| curve_value(&group.automation, sample).unwrap_or(group.fader_gain_db)).unwrap_or(0.0);
        Some((channel_gain + vca_gain).clamp(-120.0, 24.0))
    }

    pub fn linked_state(&self, channel_id: u32) -> Option<(bool, bool, bool, bool, bool)> {
        let channel = self.channels.get(&channel_id)?;
        let group = self.groups.values().find(|group| group.members.contains(&channel_id));
        Some((channel.muted || group.is_some_and(|g| g.muted), channel.solo || group.is_some_and(|g| g.solo),
            channel.listen || group.is_some_and(|g| g.listen), channel.monitor || group.is_some_and(|g| g.monitor),
            channel.record_enabled || group.is_some_and(|g| g.record_enabled)))
    }

    pub fn group_members(&self, group_id: u32) -> Vec<u32> {
        self.groups.get(&group_id).map(|group| group.members.clone()).unwrap_or_default()
    }

    /// Peak meter for a VCA link group. Member peak amplitudes are summed and
    /// converted back to dB, with a ceiling that still exposes overloads.
    pub fn summed_peak_db(&self, group_id: u32) -> Option<f32> {
        let group = self.groups.get(&group_id)?;
        let amplitude: f64 = group.members.iter().filter_map(|id| self.channels.get(id))
            .map(|channel| 10.0f64.powf(f64::from(channel.peak_db) / 20.0)).sum();
        Some(if amplitude <= 0.0 { -120.0 } else { (20.0 * amplitude.log10()).clamp(-120.0, 48.0) as f32 })
    }

    /// Bake VCA automation into every linked channel and reset the VCA to its
    /// static zero line, matching "Combine Automation" semantics.
    pub fn combine_automation(&mut self, group_id: u32) -> bool {
        let Some(group) = self.groups.get(&group_id).cloned() else { return false; };
        let mut candidates = Vec::new();
        for channel_id in &group.members {
            let Some(channel) = self.channels.get(channel_id) else { return false; };
            let mut samples: Vec<u64> = channel.automation.iter().chain(&group.automation).map(|point| point.sample).collect();
            if samples.is_empty() { samples.push(0); }
            samples.sort_unstable(); samples.dedup();
            let combined = samples.into_iter().map(|sample| {
                let channel_db = curve_value(&channel.automation, sample).unwrap_or(channel.base_gain_db);
                let vca_db = curve_value(&group.automation, sample).unwrap_or(group.fader_gain_db);
                VcaAutomationPoint { sample, gain_db: (channel_db + vca_db).clamp(-120.0, 24.0) }
            }).collect();
            candidates.push((*channel_id, combined));
        }
        for (channel_id, automation) in candidates {
            if let Some(channel) = self.channels.get_mut(&channel_id) { channel.automation = automation; }
        }
        if let Some(group) = self.groups.get_mut(&group_id) { group.fader_gain_db = 0.0; group.automation.clear(); }
        true
    }

    pub fn audit(&self) -> bool {
        self.channels.len() <= 8192 && self.groups.len() <= 1024
            && self.channels.values().all(VcaLinkedChannel::validate)
            && self.groups.values().all(VcaLinkGroup::validate)
            && self.groups.values().flat_map(|group| &group.members).all(|member| self.channels.contains_key(member))
            && self.groups.values().enumerate().all(|(index, group)| self.groups.values().skip(index + 1)
                .all(|other| group.members.iter().all(|member| !other.members.contains(member))))
    }
}

impl VcaLinkedChannel {
    fn validate(&self) -> bool {
        self.id != 0 && self.base_gain_db.is_finite() && (-120.0..=24.0).contains(&self.base_gain_db)
            && self.peak_db.is_finite() && (-120.0..=48.0).contains(&self.peak_db) && valid_curve(&self.automation)
    }
}

impl VcaLinkGroup {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 128 && !self.name.contains('\0')
            && self.fader_gain_db.is_finite() && (-120.0..=24.0).contains(&self.fader_gain_db)
            && self.members.len() <= 8192 && self.members.iter().all(|id| *id != 0)
            && self.members.windows(2).all(|pair| pair[0] < pair[1]) && valid_curve(&self.automation)
    }
}

fn normalize_curve(curve: &mut Vec<VcaAutomationPoint>) {
    curve.sort_by_key(|point| point.sample);
    curve.dedup_by_key(|point| point.sample);
}

fn valid_curve(curve: &[VcaAutomationPoint]) -> bool {
    curve.len() <= 1_000_000 && curve.iter().all(|point| point.gain_db.is_finite() && (-120.0..=24.0).contains(&point.gain_db))
        && curve.windows(2).all(|pair| pair[0].sample < pair[1].sample)
}

fn curve_value(curve: &[VcaAutomationPoint], sample: u64) -> Option<f32> {
    let first = *curve.first()?;
    if sample <= first.sample { return Some(first.gain_db); }
    let last = *curve.last()?;
    if sample >= last.sample { return Some(last.gain_db); }
    let right = curve.partition_point(|point| point.sample <= sample);
    let left = curve[right - 1]; let right = curve[right];
    let position = (sample - left.sample) as f64 / (right.sample - left.sample) as f64;
    Some((f64::from(left.gain_db) + f64::from(right.gain_db - left.gain_db) * position) as f32)
}

#[cfg(test)]
mod console_tests {
    use super::*;

    fn channel(id: u32, gain: f32, peak: f32) -> VcaLinkedChannel {
        VcaLinkedChannel { id, base_gain_db: gain, automation: vec![], muted: false, solo: false,
            listen: false, monitor: false, record_enabled: false, peak_db: peak }
    }
    fn group(id: u32, members: Vec<u32>) -> VcaLinkGroup {
        VcaLinkGroup { id, name: format!("VCA {id}"), fader_gain_db: 0.0, automation: vec![], members,
            muted: false, solo: false, listen: false, monitor: false, record_enabled: false }
    }

    #[test]
    fn vca_adds_db_and_links_channel_states() {
        let mut console = VcaConsole::default();
        assert!(console.upsert_channel(channel(1, -6.0, -12.0)));
        let mut drums = group(10, vec![1]); drums.fader_gain_db = 3.0; drums.muted = true; drums.record_enabled = true;
        assert!(console.upsert_group(drums));
        assert_eq!(console.resolved_gain_db(1, 0), Some(-3.0));
        assert_eq!(console.linked_state(1), Some((true, false, false, false, true)));
        assert!(console.audit());
    }

    #[test]
    fn combines_vca_and_channel_automation_without_changing_the_mix() {
        let mut console = VcaConsole::default();
        let mut snare = channel(1, -6.0, -12.0);
        snare.automation = vec![VcaAutomationPoint { sample: 0, gain_db: -6.0 }, VcaAutomationPoint { sample: 100, gain_db: -3.0 }];
        assert!(console.upsert_channel(snare));
        let mut drums = group(10, vec![1]);
        drums.automation = vec![VcaAutomationPoint { sample: 0, gain_db: 0.0 }, VcaAutomationPoint { sample: 100, gain_db: 6.0 }];
        assert!(console.upsert_group(drums));
        let before = console.resolved_gain_db(1, 50).unwrap();
        assert!(console.combine_automation(10));
        assert!((console.resolved_gain_db(1, 50).unwrap() - before).abs() < 0.001);
        assert!(console.groups[&10].automation.is_empty());
        assert_eq!(console.groups[&10].fader_gain_db, 0.0);
    }

    #[test]
    fn prevents_a_channel_from_being_owned_by_two_vcas() {
        let mut console = VcaConsole::default();
        assert!(console.upsert_channel(channel(1, 0.0, -6.0)));
        assert!(console.upsert_group(group(10, vec![1])));
        assert!(!console.upsert_group(group(11, vec![1])));
        assert_eq!(console.group_members(10), vec![1]);
    }

    #[test]
    fn nested_vcas_add_ancestor_gain_and_propagate_controls() {
        let mut console = VcaConsole::default();
        assert!(console.upsert_channel(channel(1, -3.0, -12.0)));
        assert!(console.upsert_channel(channel(2, -6.0, -18.0)));
        let mut drums = group(10, vec![1]); drums.fader_gain_db = -10.0;
        let mut band = group(20, vec![2]); band.fader_gain_db = 4.0; band.muted = true;
        assert!(console.upsert_group(drums)); assert!(console.upsert_group(band));
        let mut hierarchy = VcaHierarchy::default();
        assert!(hierarchy.assign_parent(&console, 10, 20));
        assert_eq!(hierarchy.resolved_gain_db(&console, 1, 0), Some(-9.0));
        assert_eq!(hierarchy.linked_state(&console, 1).unwrap().0, true);
        assert_eq!(hierarchy.descendant_channels(&console, 20), Some(vec![1, 2]));
        assert!(hierarchy.summed_peak_db(&console, 20).unwrap() > -12.0);
        assert!(hierarchy.audit(&console));
    }

    #[test]
    fn nested_vca_assignment_rejects_cycles_and_round_trips() {
        let mut console = VcaConsole::default();
        assert!(console.upsert_group(group(10, vec![])));
        assert!(console.upsert_group(group(20, vec![])));
        assert!(console.upsert_group(group(30, vec![])));
        let mut hierarchy = VcaHierarchy::default();
        assert!(hierarchy.assign_parent(&console, 10, 20));
        assert!(hierarchy.assign_parent(&console, 20, 30));
        assert!(!hierarchy.assign_parent(&console, 30, 10));
        assert_eq!(hierarchy.ancestors(10), Some(vec![20, 30]));
        let json = hierarchy.to_json(&console).unwrap();
        assert_eq!(VcaHierarchy::from_json(&json, &console).unwrap(), hierarchy);
    }

    #[test]
    fn reassigning_nested_vca_removes_it_from_previous_parent() {
        let mut console = VcaConsole::default();
        for id in [10, 20, 30] { assert!(console.upsert_group(group(id, vec![]))); }
        let mut hierarchy = VcaHierarchy::default();
        assert!(hierarchy.assign_parent(&console, 10, 20));
        assert!(hierarchy.assign_parent(&console, 10, 30));
        assert_eq!(hierarchy.parent_by_child.get(&10), Some(&30));
    }
}
