use std::collections::{HashMap, VecDeque};

pub struct PdcNodeRust {
    pub id: u32,
    pub own_latency: u32,
    pub total_latency: u32,
    pub downstream: Vec<u32>,
}

pub struct PdcOrchestrator {
    pub global_project_latency: u32,
    pub has_cycle: bool,
}

impl Default for PdcOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl PdcOrchestrator {
    pub fn new() -> Self {
        Self {
            global_project_latency: 0,
            has_cycle: false,
        }
    }

    /// INDUSTRIAL: Solves the latency graph with absolute topological precision and Kahn's Algorithm.
    pub fn solve(&mut self, nodes: &mut HashMap<u32, PdcNodeRust>) {
        self.global_project_latency = 0;
        self.has_cycle = false;

        // INDUSTRIAL: Implementation of high-performance topological sorting.
        // Rust's KahnSortEngine ensures bit-accurate routing distribution without recursion.
        let mut in_degrees = HashMap::new();
        let mut invalid_graph = false;
        for node in nodes.values() {
            for &dest in &node.downstream {
                if !nodes.contains_key(&dest) {
                    invalid_graph = true;
                    continue;
                }
                *in_degrees.entry(dest).or_insert(0) += 1;
            }
        }

        if invalid_graph {
            self.has_cycle = true;
            for node in nodes.values_mut() {
                node.total_latency = 0;
            }
            return;
        }

        let mut queue = VecDeque::new();
        for &id in nodes.keys() {
            if *in_degrees.get(&id).unwrap_or(&0) == 0 {
                queue.push_back(id);
            }
        }

        let mut sorted = Vec::new();
        while let Some(u) = queue.pop_front() {
            sorted.push(u);
            if let Some(node) = nodes.get(&u) {
                for &v in &node.downstream {
                    if let Some(degree) = in_degrees.get_mut(&v) {
                        if *degree == 0 {
                            invalid_graph = true;
                        } else {
                            *degree -= 1;
                            if *degree == 0 {
                                queue.push_back(v);
                            }
                        }
                    } else {
                        invalid_graph = true;
                    }
                }
            }
        }

        if invalid_graph || sorted.len() != nodes.len() {
            self.has_cycle = true;
            for node in nodes.values_mut() {
                node.total_latency = 0;
            }
            return;
        }

        // --- FORWARD PASS: CALCULATE LATENCIES ---
        let mut global_max = 0;
        for &id in &sorted {
            let mut current_max = 0;
            if let Some(node) = nodes.get(&id) {
                current_max = node.own_latency;
                for &dep_id in &node.downstream {
                    if let Some(dep_node) = nodes.get(&dep_id) {
                        let Some(latency) = dep_node.total_latency.checked_add(node.own_latency)
                        else {
                            invalid_graph = true;
                            break;
                        };
                        current_max = current_max.max(latency);
                    }
                }
            }
            if let Some(node) = nodes.get_mut(&id) {
                node.total_latency = current_max;
                global_max = global_max.max(current_max);
            }
        }

        if invalid_graph {
            self.global_project_latency = 0;
            self.has_cycle = true;
            for node in nodes.values_mut() {
                node.total_latency = 0;
            }
            return;
        }

        self.global_project_latency = global_max;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_pdc_graph_solver(&self, nodes: &HashMap<u32, PdcNodeRust>) -> bool {
        if self.has_cycle || nodes.is_empty() {
            return nodes.is_empty() && !self.has_cycle && self.global_project_latency == 0;
        }
        let computed_global = nodes
            .values()
            .map(|node| node.total_latency)
            .max()
            .unwrap_or(0);
        if computed_global != self.global_project_latency {
            return false;
        }
        nodes.iter().all(|(id, node)| {
            if node.id != *id || node.total_latency < node.own_latency {
                return false;
            }
            let mut expected = node.own_latency;
            for downstream_id in &node.downstream {
                if downstream_id == id {
                    return false;
                }
                let Some(downstream) = nodes.get(downstream_id) else {
                    return false;
                };
                let Some(path) = downstream.total_latency.checked_add(node.own_latency) else {
                    return false;
                };
                expected = expected.max(path);
            }
            expected == node.total_latency
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PdcNodeRust, PdcOrchestrator};
    use std::collections::HashMap;

    fn node(id: u32, own_latency: u32, total_latency: u32, downstream: Vec<u32>) -> PdcNodeRust {
        PdcNodeRust {
            id,
            own_latency,
            total_latency,
            downstream,
        }
    }

    #[test]
    fn audit_rejects_cycle_and_inconsistent_global_latency() {
        let mut solver = PdcOrchestrator::new();
        let mut nodes = HashMap::from([
            (1, node(1, 128, 128, vec![2])),
            (2, node(2, 64, 64, Vec::new())),
        ]);
        solver.solve(&mut nodes);
        assert!(solver.audit_pdc_graph_solver(&nodes));

        solver.global_project_latency = 1;
        assert!(!solver.audit_pdc_graph_solver(&nodes));
        solver.has_cycle = true;
        assert!(!solver.audit_pdc_graph_solver(&nodes));
    }

    #[test]
    fn audit_rejects_missing_downstream_and_self_routes() {
        let solver = PdcOrchestrator {
            global_project_latency: 32,
            has_cycle: false,
        };
        let missing = HashMap::from([(1, node(1, 32, 32, vec![9]))]);
        assert!(!solver.audit_pdc_graph_solver(&missing));
        let self_route = HashMap::from([(1, node(1, 32, 32, vec![1]))]);
        assert!(!solver.audit_pdc_graph_solver(&self_route));
    }
}
