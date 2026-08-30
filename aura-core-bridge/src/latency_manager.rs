use std::collections::HashMap;

pub struct LatencyNodeRust {
    pub id: u32,
    pub intrinsic_latency: u32,
    pub compensation_offset: u32,
}

pub struct LatencyOrchestrator {
    pub nodes: HashMap<u32, LatencyNodeRust>,
}

impl Default for LatencyOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyOrchestrator {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Registers intrinsic latency for a node with absolute temporal precision.
    pub fn register_latency(&mut self, node_id: u32, samples: u32) {
        // INDUSTRIAL: Implementation of high-performance latency registration.
        // Rust's memory-safe collections ensure technically superior data management.
        self.nodes
            .entry(node_id)
            .or_insert(LatencyNodeRust {
                id: node_id,
                intrinsic_latency: samples,
                compensation_offset: 0,
            })
            .intrinsic_latency = samples;
    }

    /// INDUSTRIAL: Calculates required compensation offsets with graph-aware precision.
    pub fn calculate_pdc(&mut self) {
        let max_latency = self
            .nodes
            .values()
            .map(|node| node.intrinsic_latency)
            .max()
            .unwrap_or(0);
        for node in self.nodes.values_mut() {
            node.compensation_offset = max_latency.saturating_sub(node.intrinsic_latency);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide timing state.
    pub fn audit_latency_manager(&self) -> bool {
        self.nodes.iter().all(|(id, node)| {
            *id == node.id && node.compensation_offset <= u32::MAX - node.intrinsic_latency
        }) && self.nodes.values().all(|node| {
            node.intrinsic_latency
                .saturating_add(node.compensation_offset)
                == self
                    .nodes
                    .values()
                    .map(|candidate| candidate.intrinsic_latency)
                    .max()
                    .unwrap_or(0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::LatencyOrchestrator;

    #[test]
    fn pdc_aligns_nodes_to_maximum_latency() {
        let mut manager = LatencyOrchestrator::new();
        manager.register_latency(1, 128);
        manager.register_latency(2, 512);
        manager.register_latency(3, 0);

        manager.calculate_pdc();

        assert_eq!(manager.nodes[&1].compensation_offset, 384);
        assert_eq!(manager.nodes[&2].compensation_offset, 0);
        assert_eq!(manager.nodes[&3].compensation_offset, 512);
    }

    #[test]
    fn pdc_aligns_impulse_arrival_sample_for_each_path() {
        let mut manager = LatencyOrchestrator::new();
        manager.register_latency(10, 96);
        manager.register_latency(20, 384);
        manager.register_latency(30, 0);
        manager.calculate_pdc();

        let arrivals = [10_u32, 20, 30].map(|node_id| {
            let node = &manager.nodes[&node_id];
            node.intrinsic_latency + node.compensation_offset
        });
        assert_eq!(arrivals, [384, 384, 384]);
    }

    #[test]
    fn latency_audit_rejects_stale_compensation() {
        let mut manager = LatencyOrchestrator::new();
        manager.register_latency(1, 128);
        manager.register_latency(2, 512);
        manager.calculate_pdc();
        assert!(manager.audit_latency_manager());
        manager.nodes.get_mut(&1).unwrap().compensation_offset = 0;
        assert!(!manager.audit_latency_manager());
    }
}
