pub struct PdcNode {
    pub id: u32,
    pub latency: u32,
    pub dest_id: u32,
}

pub struct PdcOrchestrator {
    pub nodes: Vec<PdcNode>,
    pub has_cycle: bool,
}

impl PdcOrchestrator {
    pub fn new() -> Self {
        Self { nodes: Vec::new(), has_cycle: false }
    }

    /// INDUSTRIAL: Performs topological analysis and cycle detection with absolute precision and PDC sovereignty.
    pub fn recalculate_pdc(&mut self, low_latency: bool) -> Vec<u32> {
        self.has_cycle = false;
        // A low-latency monitor path deliberately bypasses compensation.
        if low_latency {
            return vec![0; self.nodes.len()];
        }

        // `dest_id` is the downstream edge.  Compute the longest upstream
        // path without recursion so malformed graphs cannot overflow the
        // stack.  Unknown destinations are simply ignored.
        let mut incoming = vec![0usize; self.nodes.len()];
        for node in &self.nodes {
            if let Some(dest) = self.nodes.iter().position(|candidate| candidate.id == node.dest_id) {
                incoming[dest] += 1;
            }
        }
        let mut queue: Vec<usize> = incoming.iter().enumerate()
            .filter_map(|(index, &degree)| (degree == 0).then_some(index))
            .collect();
        let mut path_latency = vec![0u32; self.nodes.len()];
        let mut processed = 0usize;
        let mut cursor = 0;

        while cursor < queue.len() {
            let index = queue[cursor];
            cursor += 1;
            processed += 1;
            let node_latency = self.nodes[index].latency;
            if let Some(dest) = self.nodes.iter().position(|candidate| candidate.id == self.nodes[index].dest_id) {
                path_latency[dest] = path_latency[dest].max(path_latency[index].saturating_add(node_latency));
                incoming[dest] -= 1;
                if incoming[dest] == 0 {
                    queue.push(dest);
                }
            }
        }

        // Cycles have no well-defined delay.  Leave their compensation at
        // zero rather than inventing a value or panicking.
        if processed != self.nodes.len() {
            self.has_cycle = true;
            return vec![0; self.nodes.len()];
        }
        let project_latency = path_latency.iter().copied().max().unwrap_or(0);
        path_latency.into_iter().map(|latency| project_latency.saturating_sub(latency)).collect()
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide PDC synchronization graph.
    pub fn audit_pdc(&self) -> bool {
        if self.has_cycle {
            return false;
        }
        let ids: std::collections::HashSet<u32> = self.nodes.iter().map(|node| node.id).collect();
        if ids.len() != self.nodes.len() || self.nodes.iter().any(|node| node.id == node.dest_id) {
            return false;
        }
        if self.nodes.iter().any(|node| node.dest_id != 0 && !ids.contains(&node.dest_id)) {
            return false;
        }
        // A valid acyclic single-destination graph must have at least one
        // root. Re-run Kahn's count without mutating latency state.
        let mut incoming = vec![0usize; self.nodes.len()];
        for node in &self.nodes {
            if let Some(dest) = self.nodes.iter().position(|candidate| candidate.id == node.dest_id) {
                incoming[dest] += 1;
            }
        }
        let mut queue: std::collections::VecDeque<usize> = incoming.iter().enumerate()
            .filter_map(|(index, &degree)| (degree == 0).then_some(index))
            .collect();
        let mut visited = 0usize;
        while let Some(index) = queue.pop_front() {
            visited += 1;
            if let Some(dest) = self.nodes.iter().position(|candidate| candidate.id == self.nodes[index].dest_id) {
                incoming[dest] -= 1;
                if incoming[dest] == 0 { queue.push_back(dest); }
            }
        }
        visited == self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{PdcNode, PdcOrchestrator};

    #[test]
    fn audit_rejects_cycle_and_unknown_route() {
        let mut pdc = PdcOrchestrator {
            nodes: vec![PdcNode { id: 1, latency: 32, dest_id: 2 }, PdcNode { id: 2, latency: 64, dest_id: 1 }],
            has_cycle: false,
        };
        assert_eq!(pdc.recalculate_pdc(false), vec![0, 0]);
        assert!(!pdc.audit_pdc());

        let invalid = PdcOrchestrator {
            nodes: vec![PdcNode { id: 1, latency: 32, dest_id: 99 }],
            has_cycle: false,
        };
        assert!(!invalid.audit_pdc());
    }

    #[test]
    fn audit_accepts_a_valid_chain() {
        let mut pdc = PdcOrchestrator {
            nodes: vec![PdcNode { id: 1, latency: 32, dest_id: 2 }, PdcNode { id: 2, latency: 64, dest_id: 0 }],
            has_cycle: false,
        };
        let compensation = pdc.recalculate_pdc(false);
        assert_eq!(compensation.len(), 2);
        assert!(pdc.audit_pdc());
    }
}
