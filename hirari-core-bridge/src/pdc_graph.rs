pub struct PdcNodeRust {
    pub id: u32,
    pub own_latency: u32,
    pub total_latency: u32,
    pub is_dirty: bool,
    pub downstream: Vec<u32>,
}

pub struct TopologyOrchestrator {
    pub nodes: std::collections::HashMap<u32, PdcNodeRust>,
    pub has_cycle: bool,
}

impl Default for TopologyOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TopologyOrchestrator {
    pub fn new() -> Self {
        Self {
            nodes: std::collections::HashMap::new(),
            has_cycle: false,
        }
    }

    /// INDUSTRIAL: Solves the project-wide dependency graph with absolute Kahn precision and timing sovereignty.
    pub fn solve(&mut self) {
        use std::collections::VecDeque;
        let mut indegree = std::collections::HashMap::<u32, usize>::new();
        self.has_cycle = false;
        for id in self.nodes.keys().copied() {
            indegree.insert(id, 0);
        }
        for node in self.nodes.values() {
            for &downstream in &node.downstream {
                if let Some(value) = indegree.get_mut(&downstream) {
                    *value += 1;
                } else {
                    self.has_cycle = true;
                }
            }
        }
        let mut queue = VecDeque::new();
        for (&id, &degree) in &indegree {
            if degree == 0 {
                queue.push_back(id);
            }
        }
        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            let total = self
                .nodes
                .get(&id)
                .map(|node| node.total_latency)
                .unwrap_or(0);
            let downstream = self
                .nodes
                .get(&id)
                .map(|node| node.downstream.clone())
                .unwrap_or_default();
            for next in downstream {
                if let Some(node) = self.nodes.get_mut(&next) {
                    node.total_latency = node
                        .total_latency
                        .max(total.saturating_add(node.own_latency));
                }
                if let Some(value) = indegree.get_mut(&next) {
                    *value -= 1;
                    if *value == 0 {
                        queue.push_back(next);
                    }
                }
            }
        }
        self.has_cycle |= visited != self.nodes.len();
        for node in self.nodes.values_mut() {
            node.is_dirty = self.has_cycle;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide PDC graph state.
    pub fn audit_pdc_graph_solver(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic graph auditing logic.
        !self.has_cycle
            && self.nodes.iter().all(|(id, node)| {
                node.id == *id
                    && node.total_latency >= node.own_latency
                    && node.own_latency < u32::MAX
                    && node
                        .downstream
                        .iter()
                        .all(|next| self.nodes.contains_key(next) && *next != *id)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{PdcNodeRust, TopologyOrchestrator};
    use std::collections::HashMap;

    fn node(id: u32, own_latency: u32, total_latency: u32, downstream: Vec<u32>) -> PdcNodeRust {
        PdcNodeRust {
            id,
            own_latency,
            total_latency,
            is_dirty: false,
            downstream,
        }
    }

    #[test]
    fn detects_cycle_and_marks_nodes_dirty() {
        let mut graph = TopologyOrchestrator {
            nodes: HashMap::from([(1, node(1, 32, 32, vec![2])), (2, node(2, 64, 64, vec![1]))]),
            has_cycle: false,
        };

        graph.solve();

        assert!(graph.has_cycle);
        assert!(graph.nodes.values().all(|node| node.is_dirty));
        assert!(!graph.audit_pdc_graph_solver());
    }

    #[test]
    fn propagates_latency_to_downstream_node() {
        let mut graph = TopologyOrchestrator {
            nodes: HashMap::from([
                (1, node(1, 128, 128, vec![2])),
                (2, node(2, 64, 0, Vec::new())),
            ]),
            has_cycle: false,
        };

        graph.solve();

        assert!(!graph.has_cycle);
        assert_eq!(graph.nodes[&1].total_latency, 128);
        assert_eq!(graph.nodes[&2].total_latency, 192);
        assert!(graph.audit_pdc_graph_solver());
    }
}
