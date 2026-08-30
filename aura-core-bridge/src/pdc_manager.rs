pub struct PdcOrchestrator {
    pub track_latencies: Vec<u32>,
    pub bus_latencies: Vec<u32>,
    pub track_offsets: Vec<u32>,
    pub bus_offsets: Vec<u32>,
}

impl Default for PdcOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl PdcOrchestrator {
    pub fn new() -> Self {
        Self {
            track_latencies: vec![0; 512],
            bus_latencies: vec![0; 128],
            track_offsets: vec![0; 512],
            bus_offsets: vec![0; 128],
        }
    }

    /// INDUSTRIAL: Recalculates the PDC offsets with absolute graph precision and timing sovereignty.
    pub fn recalculate(&mut self) {
        let max_track = self.track_latencies.iter().copied().max().unwrap_or(0);
        let max_bus = self.bus_latencies.iter().copied().max().unwrap_or(0);
        for (offset, latency) in self.track_offsets.iter_mut().zip(&self.track_latencies) {
            *offset = max_track.saturating_sub(*latency);
        }
        for (offset, latency) in self.bus_offsets.iter_mut().zip(&self.bus_latencies) {
            *offset = max_bus.saturating_sub(*latency);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide PDC state.
    pub fn audit_pdc_manager(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic latency auditing logic.
        self.track_latencies.len() == self.track_offsets.len()
            && self.bus_latencies.len() == self.bus_offsets.len()
            && self
                .track_offsets
                .iter()
                .zip(&self.track_latencies)
                .all(|(offset, latency)| *offset <= u32::MAX - *latency)
            && self
                .bus_offsets
                .iter()
                .zip(&self.bus_latencies)
                .all(|(offset, latency)| *offset <= u32::MAX - *latency)
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum PdcChannelKind {
    AudioTrack,
    Instrument,
    Group,
    Output,
    Effect,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PluginDelayState {
    pub plugin_id: String,
    pub reported_latency: u32,
    pub low_latency_samples: u32,
    pub supports_low_latency: bool,
    pub enabled: bool,
    pub low_latency_active: bool,
    pub bypassed_by_constraint: bool,
}

impl PluginDelayState {
    fn validate(&self) -> bool {
        !self.plugin_id.trim().is_empty()
            && self.plugin_id.len() <= 256
            && !self.plugin_id.contains('\0')
            && self.low_latency_samples <= self.reported_latency
            && (!self.low_latency_active || self.supports_low_latency)
            && !(self.low_latency_active && self.bypassed_by_constraint)
    }

    pub fn effective_latency(&self) -> u32 {
        if !self.enabled || self.bypassed_by_constraint {
            0
        } else if self.low_latency_active {
            self.low_latency_samples
        } else {
            self.reported_latency
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PdcChannelState {
    pub channel_id: u32,
    pub kind: PdcChannelKind,
    pub record_enabled: bool,
    pub plugins: Vec<PluginDelayState>,
}

impl PdcChannelState {
    pub fn effective_latency(&self) -> u32 {
        self.plugins
            .iter()
            .fold(0u32, |total, plugin| total.saturating_add(plugin.effective_latency()))
    }

    fn validate(&self) -> bool {
        self.channel_id != 0
            && self.plugins.len() <= 256
            && self.plugins.iter().all(PluginDelayState::validate)
            && self.plugins.iter().enumerate().all(|(index, plugin)| {
                self.plugins[..index]
                    .iter()
                    .all(|previous| previous.plugin_id != plugin.plugin_id)
            })
    }
}

/// Models Cubase-style Constrain Delay Compensation without destroying the
/// user's plug-in enabled state. Effect channels remain fully compensated;
/// instrument, group, output, and record-enabled audio channels are constrained.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ConstrainDelayCompensation {
    pub enabled: bool,
    pub threshold_samples: u32,
    pub channels: Vec<PdcChannelState>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PdcRoutingNode {
    pub channel_id: u32,
    pub plugin_latency_samples: u32,
    pub external_latency_samples: u32,
    pub routes_to: Vec<u32>,
    pub output: bool,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PdcRoutingGraph { pub nodes: Vec<PdcRoutingNode> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdcEdgeCompensation { pub source: u32, pub destination: u32, pub delay_samples: u64 }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdcRoutingSchedule {
    pub output_latency_by_channel: std::collections::BTreeMap<u32, u64>,
    pub edge_compensation: Vec<PdcEdgeCompensation>,
    pub final_output_compensation: std::collections::BTreeMap<u32, u64>,
    pub maximum_output_latency: u64,
}

impl PdcRoutingGraph {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() { return Err("invalid PDC routing graph".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let graph: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if graph.validate() { Ok(graph) } else { Err("invalid PDC routing graph".into()) }
    }

    /// Build a sample-accurate compensation schedule for tracks, sends,
    /// groups, effects, and outputs. Every input is delayed to the slowest
    /// arrival at its destination before that destination's own latency.
    pub fn schedule(&self) -> Result<PdcRoutingSchedule, String> {
        if !self.validate() { return Err("invalid or cyclic PDC routing graph".into()); }
        let nodes = self.nodes.iter().map(|node| (node.channel_id, node)).collect::<std::collections::BTreeMap<_, _>>();
        let mut incoming = self.nodes.iter().map(|node| (node.channel_id, Vec::<u32>::new()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut indegree = self.nodes.iter().map(|node| (node.channel_id, 0usize))
            .collect::<std::collections::BTreeMap<_, _>>();
        for node in &self.nodes {
            for destination in &node.routes_to {
                incoming.get_mut(destination).expect("validated destination").push(node.channel_id);
                *indegree.get_mut(destination).expect("validated destination") += 1;
            }
        }
        let mut ready = indegree.iter().filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<std::collections::BTreeSet<_>>();
        let mut schedule = PdcRoutingSchedule::default();
        let mut processed = 0usize;
        while let Some(id) = ready.pop_first() {
            let node = nodes[&id];
            let sources = &incoming[&id];
            let slowest_input = sources.iter().filter_map(|source| schedule.output_latency_by_channel.get(source)).copied().max().unwrap_or(0);
            for source in sources {
                let source_latency = schedule.output_latency_by_channel[source];
                schedule.edge_compensation.push(PdcEdgeCompensation { source: *source, destination: id,
                    delay_samples: slowest_input - source_latency });
            }
            let own = u64::from(node.plugin_latency_samples) + u64::from(node.external_latency_samples);
            schedule.output_latency_by_channel.insert(id, slowest_input.checked_add(own)
                .ok_or_else(|| "PDC path latency overflow".to_owned())?);
            processed += 1;
            for destination in &node.routes_to {
                let degree = indegree.get_mut(destination).expect("validated destination");
                *degree -= 1; if *degree == 0 { ready.insert(*destination); }
            }
        }
        if processed != self.nodes.len() { return Err("PDC routing contains a cycle".into()); }
        let outputs = self.nodes.iter().filter(|node| node.output).map(|node| node.channel_id).collect::<Vec<_>>();
        schedule.maximum_output_latency = outputs.iter().filter_map(|id| schedule.output_latency_by_channel.get(id)).copied().max().unwrap_or(0);
        for output in outputs {
            schedule.final_output_compensation.insert(output,
                schedule.maximum_output_latency - schedule.output_latency_by_channel[&output]);
        }
        schedule.edge_compensation.sort_by_key(|edge| (edge.destination, edge.source));
        Ok(schedule)
    }

    pub fn validate(&self) -> bool {
        if self.nodes.is_empty() || self.nodes.len() > 65_536 { return false; }
        let ids = self.nodes.iter().map(|node| node.channel_id).collect::<std::collections::BTreeSet<_>>();
        ids.len() == self.nodes.len() && !ids.contains(&0) && self.nodes.iter().all(|node| {
            node.routes_to.len() <= 4096 && node.routes_to.iter().all(|destination| *destination != node.channel_id && ids.contains(destination))
                && node.routes_to.iter().enumerate().all(|(index, destination)| !node.routes_to[..index].contains(destination))
        }) && self.nodes.iter().any(|node| node.output)
            && self.is_acyclic()
    }

    fn is_acyclic(&self) -> bool {
        let mut indegree = self.nodes.iter().map(|node| (node.channel_id, 0usize)).collect::<std::collections::BTreeMap<_, _>>();
        for node in &self.nodes { for destination in &node.routes_to { let Some(value) = indegree.get_mut(destination) else { return false; }; *value += 1; } }
        let mut ready = indegree.iter().filter_map(|(id, degree)| (*degree == 0).then_some(*id)).collect::<Vec<_>>();
        let mut processed = 0usize;
        while let Some(id) = ready.pop() {
            processed += 1;
            let Some(node) = self.nodes.iter().find(|node| node.channel_id == id) else { return false; };
            for destination in &node.routes_to { let degree = indegree.get_mut(destination).expect("known destination");
                *degree -= 1; if *degree == 0 { ready.push(*destination); } }
        }
        processed == self.nodes.len()
    }
}

impl ConstrainDelayCompensation {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid delay compensation state".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.audit() { Ok(value) } else { Err("invalid delay compensation state".into()) }
    }

    pub fn set_threshold(&mut self, samples: u32) {
        self.threshold_samples = samples;
        if self.enabled {
            self.apply();
        }
    }

    pub fn upsert_channel(&mut self, channel: PdcChannelState) -> bool {
        if !channel.validate() {
            return false;
        }
        if let Some(existing) = self.channels.iter_mut().find(|item| item.channel_id == channel.channel_id) {
            *existing = channel;
        } else if self.channels.len() < 4096 {
            self.channels.push(channel);
        } else {
            return false;
        }
        self.channels.sort_by_key(|item| item.channel_id);
        self.apply();
        true
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.apply();
    }

    pub fn set_record_enabled(&mut self, channel_id: u32, enabled: bool) -> bool {
        let Some(channel) = self.channels.iter_mut().find(|item| item.channel_id == channel_id) else {
            return false;
        };
        channel.record_enabled = enabled;
        self.apply();
        true
    }

    pub fn latency_for(&self, channel_id: u32) -> Option<u32> {
        self.channels.iter().find(|item| item.channel_id == channel_id).map(PdcChannelState::effective_latency)
    }

    pub fn compensation_offsets(&self) -> Vec<(u32, u32)> {
        let maximum = self.channels.iter().map(PdcChannelState::effective_latency).max().unwrap_or(0);
        self.channels.iter().map(|channel| {
            (channel.channel_id, maximum.saturating_sub(channel.effective_latency()))
        }).collect()
    }

    fn apply(&mut self) {
        for channel in &mut self.channels {
            let constrain_channel = self.enabled && match channel.kind {
                PdcChannelKind::AudioTrack => channel.record_enabled,
                PdcChannelKind::Instrument | PdcChannelKind::Group | PdcChannelKind::Output => true,
                PdcChannelKind::Effect => false,
            };
            for plugin in &mut channel.plugins {
                plugin.low_latency_active = false;
                plugin.bypassed_by_constraint = false;
                if constrain_channel && plugin.enabled && plugin.reported_latency > self.threshold_samples {
                    if plugin.supports_low_latency {
                        plugin.low_latency_active = true;
                    } else {
                        plugin.bypassed_by_constraint = true;
                    }
                }
            }
        }
    }

    pub fn audit(&self) -> bool {
        self.channels.len() <= 4096
            && self.channels.iter().all(PdcChannelState::validate)
            && self.channels.windows(2).all(|pair| pair[0].channel_id < pair[1].channel_id)
            && self.channels.iter().all(|channel| {
                channel.plugins.iter().all(|plugin| {
                    if !self.enabled || channel.kind == PdcChannelKind::Effect {
                        !plugin.low_latency_active && !plugin.bypassed_by_constraint
                    } else {
                        true
                    }
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recalculates_track_and_bus_delay_offsets() {
        let mut pdc = PdcOrchestrator::new();
        pdc.track_latencies[0] = 128;
        pdc.track_latencies[1] = 512;
        pdc.bus_latencies[0] = 64;
        pdc.bus_latencies[1] = 256;

        pdc.recalculate();

        assert_eq!(pdc.track_offsets[0], 384);
        assert_eq!(pdc.track_offsets[1], 0);
        assert_eq!(pdc.bus_offsets[0], 192);
        assert_eq!(pdc.bus_offsets[1], 0);
        assert!(pdc.audit_pdc_manager());
    }

    fn plugin(id: &str, latency: u32, supports_low_latency: bool) -> PluginDelayState {
        PluginDelayState {
            plugin_id: id.into(),
            reported_latency: latency,
            low_latency_samples: if supports_low_latency { 8 } else { 0 },
            supports_low_latency,
            enabled: true,
            low_latency_active: false,
            bypassed_by_constraint: false,
        }
    }

    #[test]
    fn constrains_only_eligible_channels_and_restores_plugins() {
        let mut manager = ConstrainDelayCompensation { enabled: false, threshold_samples: 32, channels: vec![] };
        assert!(manager.upsert_channel(PdcChannelState { channel_id: 1, kind: PdcChannelKind::AudioTrack,
            record_enabled: true, plugins: vec![plugin("live", 128, true), plugin("legacy", 64, false)] }));
        assert!(manager.upsert_channel(PdcChannelState { channel_id: 2, kind: PdcChannelKind::Effect,
            record_enabled: false, plugins: vec![plugin("reverb", 256, false)] }));

        manager.set_enabled(true);

        assert_eq!(manager.latency_for(1), Some(8));
        assert_eq!(manager.latency_for(2), Some(256));
        assert!(manager.channels[0].plugins[0].low_latency_active);
        assert!(manager.channels[0].plugins[1].bypassed_by_constraint);
        assert!(!manager.channels[1].plugins[0].bypassed_by_constraint);

        manager.set_enabled(false);
        assert_eq!(manager.latency_for(1), Some(192));
        assert!(manager.channels.iter().flat_map(|channel| &channel.plugins)
            .all(|plugin| !plugin.low_latency_active && !plugin.bypassed_by_constraint));
        assert!(manager.audit());
    }

    #[test]
    fn record_arm_transition_recomputes_audio_track_constraint() {
        let mut manager = ConstrainDelayCompensation { enabled: true, threshold_samples: 0, channels: vec![] };
        assert!(manager.upsert_channel(PdcChannelState { channel_id: 7, kind: PdcChannelKind::AudioTrack,
            record_enabled: false, plugins: vec![plugin("lookahead", 512, false)] }));
        assert_eq!(manager.latency_for(7), Some(512));
        assert!(manager.set_record_enabled(7, true));
        assert_eq!(manager.latency_for(7), Some(0));
        assert!(manager.set_record_enabled(7, false));
        assert_eq!(manager.latency_for(7), Some(512));
    }

    #[test]
    fn routing_graph_compensates_tracks_sends_groups_and_outputs() {
        let graph = PdcRoutingGraph { nodes: vec![
            PdcRoutingNode { channel_id: 1, plugin_latency_samples: 100, external_latency_samples: 0,
                routes_to: vec![3, 6], output: false },
            PdcRoutingNode { channel_id: 2, plugin_latency_samples: 20, external_latency_samples: 0,
                routes_to: vec![3, 4], output: false },
            PdcRoutingNode { channel_id: 4, plugin_latency_samples: 200, external_latency_samples: 0,
                routes_to: vec![3], output: false },
            PdcRoutingNode { channel_id: 3, plugin_latency_samples: 30, external_latency_samples: 0,
                routes_to: vec![5], output: false },
            PdcRoutingNode { channel_id: 5, plugin_latency_samples: 10, external_latency_samples: 0,
                routes_to: vec![], output: true },
            PdcRoutingNode { channel_id: 6, plugin_latency_samples: 0, external_latency_samples: 0,
                routes_to: vec![], output: true },
        ] };
        let schedule = graph.schedule().unwrap();
        assert_eq!(schedule.output_latency_by_channel[&3], 250);
        assert_eq!(schedule.maximum_output_latency, 260);
        assert_eq!(schedule.final_output_compensation[&6], 160);
        assert!(schedule.edge_compensation.iter().any(|edge| edge.source == 1 && edge.destination == 3 && edge.delay_samples == 120));
        assert!(schedule.edge_compensation.iter().any(|edge| edge.source == 2 && edge.destination == 3 && edge.delay_samples == 200));
        let json = graph.to_json().unwrap();
        assert_eq!(PdcRoutingGraph::from_json(&json).unwrap(), graph);
    }

    #[test]
    fn routing_graph_rejects_feedback_cycles_and_unknown_destinations() {
        let cyclic = PdcRoutingGraph { nodes: vec![
            PdcRoutingNode { channel_id: 1, plugin_latency_samples: 0, external_latency_samples: 0,
                routes_to: vec![2], output: true },
            PdcRoutingNode { channel_id: 2, plugin_latency_samples: 0, external_latency_samples: 0,
                routes_to: vec![1], output: false },
        ] };
        assert!(!cyclic.validate());
        assert!(cyclic.schedule().is_err());
        let unknown = PdcRoutingGraph { nodes: vec![PdcRoutingNode { channel_id: 1,
            plugin_latency_samples: 0, external_latency_samples: 0, routes_to: vec![99], output: true }] };
        assert!(!unknown.validate());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn pdc_round_trip_preserves_original_and_constrained_plugin_state() {
        let mut manager = ConstrainDelayCompensation { enabled: true, threshold_samples: 64, channels: vec![] };
        assert!(manager.upsert_channel(PdcChannelState { channel_id: 1, kind: PdcChannelKind::Instrument,
            record_enabled: false, plugins: vec![PluginDelayState { plugin_id: "linear-phase".into(),
                reported_latency: 1024, low_latency_samples: 0, supports_low_latency: false,
                enabled: true, low_latency_active: false, bypassed_by_constraint: false }] }));
        let json = manager.to_json().unwrap();
        assert_eq!(ConstrainDelayCompensation::from_json(&json).unwrap(), manager);
        assert!(ConstrainDelayCompensation::from_json("{\"enabled\":true,\"threshold_samples\":0,\"channels\":null}").is_err());
    }
}
