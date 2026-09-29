pub struct RoutingLevelRust {
    pub nodes: Vec<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_rejects_cycles() {
        let mut r = RoutingOrchestrator::new();
        assert!(r.connect(1, 2));
        assert!(r.connect(1, 3));
        assert_eq!(r.destinations(1), &[2, 3]);
        assert!(!r.connect(2, 1));
        assert!(r.disconnect(1, 2));
        assert_eq!(r.destinations(1), &[3]);
        assert!(r.audit_bus_router());
    }

    #[test]
    fn direct_routes_fan_out_with_independent_gains() {
        let mut router = RoutingOrchestrator::new();
        assert!(router.connect(1, 2));
        assert!(router.connect(1, 3));
        assert!(router.set_route_gain(1, 2, 0.5));
        assert!(router.set_route_gain(1, 3, -1.0));
        let mut outputs = std::collections::HashMap::new();
        assert!(router.mix_direct_routes(1, &[2.0, -2.0], &mut outputs));
        assert_eq!(outputs.get(&2).unwrap(), &[1.0, -1.0]);
        assert_eq!(outputs.get(&3).unwrap(), &[-2.0, 2.0]);
        assert!(!router.set_route_gain(1, 4, 1.0));
        assert!(router.audit_bus_router());
    }

    #[test]
    fn direct_route_fanout_rejects_mismatched_buffers_atomically() {
        let mut router = RoutingOrchestrator::new();
        assert!(router.connect(1, 2));
        assert!(router.connect(1, 3));
        let mut outputs = std::collections::HashMap::from([(2, vec![9.0])]);
        assert!(!router.mix_direct_routes(1, &[1.0, 2.0], &mut outputs));
        assert_eq!(outputs.get(&2).unwrap(), &[9.0]);
        assert!(!outputs.contains_key(&3));
    }
}

pub struct RoutingOrchestrator {
    pub adj: Vec<Vec<u32>>,
    pub all_nodes: std::collections::HashSet<u32>,
    pub cached_levels: Vec<RoutingLevelRust>,
    /// Linear gain for each direct route.  Keeping this beside the topology
    /// lets one source feed several buses without duplicating audio buffers.
    pub route_gains: std::collections::HashMap<(u32, u32), f32>,
}

impl Default for RoutingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingOrchestrator {
    pub fn new() -> Self {
        Self {
            adj: Vec::new(),
            all_nodes: std::collections::HashSet::new(),
            cached_levels: Vec::new(),
            route_gains: std::collections::HashMap::new(),
        }
    }
    pub fn connect(&mut self, source: u32, destination: u32) -> bool {
        if source == destination {
            return false;
        }
        let index = source as usize;
        if self.adj.len() <= index {
            self.adj.resize_with(index + 1, Vec::new);
        }
        if self.adj[index].contains(&destination) {
            return false;
        }
        self.adj[index].push(destination);
        self.all_nodes.insert(source);
        self.all_nodes.insert(destination);
        self.route_gains.insert((source, destination), 1.0);
        self.rebuild_graph();
        if self
            .cached_levels
            .iter()
            .map(|level| level.nodes.len())
            .sum::<usize>()
            >= self.all_nodes.len()
        {
            true
        } else {
            self.adj[index].retain(|id| *id != destination);
            self.route_gains.remove(&(source, destination));
            self.rebuild_graph();
            false
        }
    }
    pub fn disconnect(&mut self, source: u32, destination: u32) -> bool {
        let Some(edges) = self.adj.get_mut(source as usize) else {
            return false;
        };
        let before = edges.len();
        edges.retain(|id| *id != destination);
        let removed = before != edges.len();
        if removed {
            self.route_gains.remove(&(source, destination));
            self.rebuild_graph();
        }
        removed
    }
    pub fn remove_node(&mut self, node: u32) -> bool {
        if !self.all_nodes.remove(&node) {
            return false;
        }
        if let Some(edges) = self.adj.get_mut(node as usize) {
            edges.clear();
        }
        for edges in &mut self.adj {
            edges.retain(|destination| *destination != node);
        }
        self.route_gains
            .retain(|(source, destination), _| *source != node && *destination != node);
        self.rebuild_graph();
        true
    }
    pub fn destinations(&self, source: u32) -> &[u32] {
        self.adj
            .get(source as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Set a direct-route gain in linear amplitude.  A route must already
    /// exist, preventing orphan routing metadata from surviving edits.
    pub fn set_route_gain(&mut self, source: u32, destination: u32, gain: f32) -> bool {
        if !gain.is_finite()
            || !(-8.0..=8.0).contains(&gain)
            || !self.destinations(source).contains(&destination)
        {
            return false;
        }
        self.route_gains.insert((source, destination), gain);
        true
    }

    pub fn route_gain(&self, source: u32, destination: u32) -> Option<f32> {
        self.route_gains.get(&(source, destination)).copied()
    }

    /// Fan out an interleaved source block to every direct destination.
    /// Output buffers are accumulated (never cleared), matching a mix-bus
    /// graph where several sources can feed the same destination.
    pub fn mix_direct_routes(
        &self,
        source: u32,
        input: &[f32],
        outputs: &mut std::collections::HashMap<u32, Vec<f32>>,
    ) -> bool {
        if input.is_empty()
            || input.len() > 16_000_000
            || input.iter().any(|sample| !sample.is_finite())
        {
            return false;
        }
        let destinations = self.destinations(source);
        if destinations.is_empty() {
            return false;
        }
        // Validate every pre-existing destination before touching any output,
        // preserving transactionality if one buffer belongs to another block.
        if destinations.iter().any(|destination| {
            outputs
                .get(destination)
                .is_some_and(|output| output.len() != input.len())
        }) {
            return false;
        }
        for &destination in destinations {
            let gain = self.route_gain(source, destination).unwrap_or(1.0);
            let output = outputs
                .entry(destination)
                .or_insert_with(|| vec![0.0; input.len()]);
            for (slot, sample) in output.iter_mut().zip(input) {
                *slot += *sample * gain;
            }
        }
        true
    }

    /// INDUSTRIAL: Rebuilds the signal flow graph with absolute graph precision and signal sovereignty.
    pub fn rebuild_graph(&mut self) {
        use std::collections::{HashMap, VecDeque};
        self.cached_levels.clear();
        let mut nodes = self.all_nodes.clone();
        for (source, destinations) in self.adj.iter().enumerate() {
            nodes.insert(source as u32);
            nodes.extend(destinations.iter().copied());
        }
        let mut indegree = nodes
            .iter()
            .map(|id| (*id, 0usize))
            .collect::<HashMap<_, _>>();
        for destinations in &self.adj {
            for destination in destinations {
                if let Some(value) = indegree.get_mut(destination) {
                    *value += 1;
                }
            }
        }
        let mut queue = VecDeque::new();
        for (&id, &degree) in &indegree {
            if degree == 0 {
                queue.push_back(id);
            }
        }
        while !queue.is_empty() {
            let level = queue.len();
            let mut current = Vec::with_capacity(level);
            for _ in 0..level {
                let Some(id) = queue.pop_front() else {
                    break;
                };
                current.push(id);
                let destinations = self.adj.get(id as usize).cloned().unwrap_or_default();
                for destination in destinations {
                    if let Some(value) = indegree.get_mut(&destination) {
                        *value = value.saturating_sub(1);
                        if *value == 0 {
                            queue.push_back(destination);
                        }
                    }
                }
            }
            if !current.is_empty() {
                self.cached_levels.push(RoutingLevelRust { nodes: current });
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal routing state.
    pub fn audit_bus_router(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic routing auditing logic.
        let mut seen = std::collections::HashSet::new();
        self.cached_levels.iter().all(|level| {
            level.nodes.windows(2).all(|pair| pair[0] != pair[1])
                && level.nodes.iter().all(|id| {
                    (*id == 0 || self.all_nodes.is_empty() || self.all_nodes.contains(id))
                        && (*id == 0 || seen.insert(*id))
                })
        }) && seen.len() == self.all_nodes.len()
            && self
                .route_gains
                .iter()
                .all(|((source, destination), gain)| {
                    self.destinations(*source).contains(destination)
                        && gain.is_finite()
                        && (-8.0..=8.0).contains(gain)
                })
    }
}
