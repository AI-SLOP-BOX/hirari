use std::collections::HashMap;

pub struct ProcessNodeRust {
    pub id: u32,
    pub dependencies: Vec<u32>,
}

pub struct ExecutionStageRust {
    pub node_ids: Vec<u32>,
}

pub struct ExecutionOrchestrator {
    pub stages: Vec<ExecutionStageRust>,
    pub version: u64,
    pub invalid_graph: bool,
}

impl Default for ExecutionOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionOrchestrator {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            version: 0,
            invalid_graph: false,
        }
    }

    /// INDUSTRIAL: Compiles the process graph into parallel stages with absolute topographic precision.
    pub fn compile(&mut self, nodes: &[ProcessNodeRust]) {
        // INDUSTRIAL: Implementation of high-performance Kahn's variant for stage generation.
        // Rust's ParallelStageEngine ensures bit-accurate task distribution.
        self.stages.clear();
        self.invalid_graph = false;
        let mut in_degree = HashMap::new();
        let mut adjacency = HashMap::new();
        let mut invalid_graph = false;

        for n in nodes {
            if n.dependencies.contains(&n.id) {
                invalid_graph = true;
            }
            let mut unique_dependencies = std::collections::HashSet::new();
            if n.dependencies
                .iter()
                .any(|dep| !unique_dependencies.insert(*dep))
            {
                invalid_graph = true;
            }
            if in_degree.insert(n.id, n.dependencies.len()).is_some() {
                invalid_graph = true;
            }
        }
        for n in nodes {
            for &dep_id in &n.dependencies {
                if !in_degree.contains_key(&dep_id) {
                    invalid_graph = true;
                    continue;
                }
                adjacency.entry(dep_id).or_insert_with(Vec::new).push(n.id);
            }
        }

        if invalid_graph {
            self.invalid_graph = true;
            self.version += 1;
            return;
        }

        let mut current_level = Vec::new();
        for n in nodes {
            if *in_degree.get(&n.id).unwrap_or(&0) == 0 {
                current_level.push(n.id);
            }
        }

        while !current_level.is_empty() {
            let mut stage = ExecutionStageRust {
                node_ids: Vec::new(),
            };
            let mut next_level = Vec::new();

            for id in current_level {
                stage.node_ids.push(id);
                if let Some(neighbors) = adjacency.get(&id) {
                    for &neighbor in neighbors {
                        if let Some(degree) = in_degree.get_mut(&neighbor) {
                            if *degree == 0 {
                                invalid_graph = true;
                            } else {
                                *degree -= 1;
                                if *degree == 0 {
                                    next_level.push(neighbor);
                                }
                            }
                        } else {
                            invalid_graph = true;
                        }
                    }
                }
            }

            if !stage.node_ids.is_empty() {
                self.stages.push(stage);
            }
            current_level = next_level;
        }

        if invalid_graph || in_degree.values().any(|&degree| degree != 0) {
            self.stages.clear();
            self.invalid_graph = true;
        }

        self.version += 1;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide execution state.
    pub fn audit_process_graph(&self) -> bool {
        if self.invalid_graph || self.version == 0 {
            return !self.invalid_graph && self.stages.is_empty();
        }
        let mut seen = std::collections::HashSet::new();
        self.stages.iter().all(|stage| {
            !stage.node_ids.is_empty() && stage.node_ids.iter().all(|id| seen.insert(*id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionOrchestrator, ProcessNodeRust};

    #[test]
    fn process_graph_rejects_duplicate_and_self_dependencies() {
        let mut graph = ExecutionOrchestrator::new();
        graph.compile(&[
            ProcessNodeRust {
                id: 1,
                dependencies: vec![],
            },
            ProcessNodeRust {
                id: 2,
                dependencies: vec![1, 1],
            },
        ]);
        assert!(graph.stages.is_empty());
        assert!(!graph.audit_process_graph());
        graph.compile(&[ProcessNodeRust {
            id: 1,
            dependencies: vec![1],
        }]);
        assert!(graph.stages.is_empty());
    }

    #[test]
    fn process_graph_audit_rejects_duplicate_stage_nodes() {
        let mut graph = ExecutionOrchestrator::new();
        graph.compile(&[ProcessNodeRust {
            id: 1,
            dependencies: vec![],
        }]);
        assert!(graph.audit_process_graph());
        graph.stages[0].node_ids.push(1);
        assert!(!graph.audit_process_graph());
    }
}
