use std::collections::{HashMap, VecDeque};

pub struct AudioNodeRust {
    pub id: u32,
    pub processing_latency: u32,
    pub cumulative_delay: u32,
    pub outgoing_edges: Vec<u32>,
    pub in_degree: u32,
}

pub struct RoutingOrchestrator {
    pub nodes: HashMap<u32, AudioNodeRust>,
    pub execution_order: Vec<u32>,
}

impl Default for RoutingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingOrchestrator {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            execution_order: Vec::new(),
        }
    }

    /// INDUSTRIAL: Compiles the routing graph with absolute topological precision and Kahn's Algorithm.
    pub fn compile_graph(&mut self) -> bool {
        // INDUSTRIAL: Implementation of high-performance topological sorting.
        // Rust's DependencySortEngine ensures bit-accurate routing distribution.
        self.execution_order.clear();
        let mut in_degrees = HashMap::new();
        let mut no_incoming = VecDeque::new();

        // Reject dangling edges before the topological sort.  Apart from being
        // an invalid graph, such an edge has no entry in `in_degrees` and must
        // not be allowed to turn the error path into a panic.
        for node in self.nodes.values() {
            if node
                .outgoing_edges
                .iter()
                .any(|neighbor| !self.nodes.contains_key(neighbor))
            {
                return false;
            }
        }

        // Derive indegrees from the edge list rather than trusting a stale
        // serialized field. This keeps graph compilation deterministic after
        // edits and also repairs the cached node metadata.
        for &id in self.nodes.keys() {
            in_degrees.insert(id, 0_u32);
        }
        for node in self.nodes.values() {
            for &neighbor in &node.outgoing_edges {
                let Some(degree) = in_degrees.get_mut(&neighbor) else {
                    return false;
                };
                let Some(next) = degree.checked_add(1) else {
                    return false;
                };
                *degree = next;
            }
        }
        for (&id, degree) in &in_degrees {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.in_degree = *degree;
            }
            if *degree == 0 {
                no_incoming.push_back(id);
            }
        }

        while let Some(curr) = no_incoming.pop_front() {
            self.execution_order.push(curr);
            if let Some(node) = self.nodes.get(&curr) {
                for &neighbor in &node.outgoing_edges {
                    let Some(degree) = in_degrees.get_mut(&neighbor) else {
                        return false;
                    };
                    let Some(next_degree) = degree.checked_sub(1) else {
                        return false;
                    };
                    *degree = next_degree;
                    if *degree == 0 {
                        no_incoming.push_back(neighbor);
                    }
                }
            }
        }

        if self.execution_order.len() != self.nodes.len() {
            return false;
        }

        // --- PDC: DELAY COMPENSATION CALCULATION ---
        let mut path_delays = HashMap::new();
        for &curr in self.execution_order.iter().rev() {
            let mut max_child_path = 0;
            if let Some(node) = self.nodes.get(&curr) {
                for &neighbor in &node.outgoing_edges {
                    max_child_path = max_child_path.max(*path_delays.get(&neighbor).unwrap_or(&0));
                }
                let Some(path_delay) = node.processing_latency.checked_add(max_child_path) else {
                    return false;
                };
                path_delays.insert(curr, path_delay);
            }
        }

        let mut branch_delays = HashMap::<u32, u32>::new();
        for (&_curr, node) in &self.nodes {
            let mut max_child_path = 0;
            for &neighbor in &node.outgoing_edges {
                max_child_path = max_child_path.max(*path_delays.get(&neighbor).unwrap_or(&0));
            }

            for &neighbor in &node.outgoing_edges {
                let neighbor_path = *path_delays.get(&neighbor).unwrap_or(&0);
                let _diff = max_child_path.saturating_sub(neighbor_path);
                // Store the delay needed to align this branch with the
                // longest downstream path. This is the value consumed by the
                // realtime PDC scheduler.
                let entry = branch_delays.entry(neighbor).or_insert(0);
                *entry = (*entry).max(_diff);
            }
        }
        for (id, delay) in branch_delays {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.cumulative_delay = delay;
            }
        }

        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_routing_graph_pdc(&self) -> bool {
        let mut incoming = HashMap::<u32, u32>::new();
        for (&id, node) in &self.nodes {
            if node.id != id || node.processing_latency > 1_000_000 {
                return false;
            }
            if node.cumulative_delay > 1_000_000
                || node
                    .outgoing_edges
                    .iter()
                    .any(|edge| !self.nodes.contains_key(edge))
            {
                return false;
            }
            for &edge in &node.outgoing_edges {
                if edge == id
                    || node
                        .outgoing_edges
                        .iter()
                        .filter(|candidate| **candidate == edge)
                        .count()
                        > 1
                {
                    return false;
                }
                let count = incoming.entry(edge).or_insert(0);
                *count = count.saturating_add(1);
            }
        }
        if self
            .nodes
            .iter()
            .any(|(id, node)| node.in_degree != incoming.get(id).copied().unwrap_or(0))
        {
            return false;
        }
        let mut seen = std::collections::HashSet::new();
        self.execution_order.len() == self.nodes.len()
            && self
                .execution_order
                .iter()
                .all(|id| self.nodes.contains_key(id) && seen.insert(*id))
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioNodeRust, RoutingOrchestrator};
    use std::collections::HashMap;

    #[test]
    fn routing_audit_accepts_compiled_graph_and_rejects_bad_degree() {
        let mut graph = RoutingOrchestrator {
            nodes: HashMap::new(),
            execution_order: Vec::new(),
        };
        graph.nodes.insert(
            1,
            AudioNodeRust {
                id: 1,
                processing_latency: 32,
                cumulative_delay: 0,
                outgoing_edges: vec![2],
                in_degree: 0,
            },
        );
        graph.nodes.insert(
            2,
            AudioNodeRust {
                id: 2,
                processing_latency: 64,
                cumulative_delay: 0,
                outgoing_edges: vec![],
                in_degree: 1,
            },
        );
        assert!(graph.compile_graph());
        assert!(graph.audit_routing_graph_pdc());
        graph.nodes.get_mut(&2).unwrap().in_degree = 0;
        assert!(!graph.audit_routing_graph_pdc());
    }
}
