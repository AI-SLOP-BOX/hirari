/**
 * @struct GraphNode
 * @brief Industrial representation of a processing node in the DAW graph.
 */
pub struct GraphNode {
    pub id: u32,
    pub dependencies: Vec<u32>,
    pub cache_weight: u32, // INDUSTRIAL: Higher weight means more L3 pressure
}

/**
 * @struct ExecutionStage
 * @brief A group of nodes that can be executed in parallel.
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionStage {
    pub node_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphSolveError {
    DuplicateNodeId(u32),
    UnknownDependency { node_id: u32, dependency_id: u32 },
    CyclicDependency,
}

/**
 * @class GraphSolver
 * @brief Cache-aware parallel orchestrator for high-track-count DAW projects.
 * INDUSTRIAL: Groups nodes to maximize L3 cache locality and minimize context switching.
 */
pub struct GraphSolver;

impl GraphSolver {
    /**
     * @brief SOLVE: Computes the optimal execution order.
     */
    pub fn solve(nodes: &[GraphNode]) -> Result<Vec<ExecutionStage>, GraphSolveError> {
        let by_id: std::collections::HashMap<u32, &GraphNode> = nodes
            .iter()
            .map(|node| {
                if node.id == 0 {
                    return Err(GraphSolveError::DuplicateNodeId(node.id));
                }
                Ok((node.id, node))
            })
            .collect::<Result<_, _>>()?;
        if by_id.len() != nodes.len() {
            let mut ids = std::collections::HashSet::new();
            if let Some(duplicate) = nodes
                .iter()
                .find_map(|node| (!ids.insert(node.id)).then_some(node.id))
            {
                return Err(GraphSolveError::DuplicateNodeId(duplicate));
            }
        }
        for node in nodes {
            for dependency in &node.dependencies {
                if !by_id.contains_key(dependency) {
                    return Err(GraphSolveError::UnknownDependency {
                        node_id: node.id,
                        dependency_id: *dependency,
                    });
                }
            }
        }

        let mut stages = Vec::new();
        let mut processed = std::collections::HashSet::new();
        let mut remaining: Vec<&GraphNode> = nodes.iter().collect();

        while !remaining.is_empty() {
            let mut current_stage = Vec::new();
            let mut next_remaining = Vec::new();

            // INDUSTRIAL: Group nodes whose dependencies are already met.
            for node in remaining {
                let ready = node.dependencies.iter().all(|d| processed.contains(d));
                if ready {
                    current_stage.push(node.id);
                } else {
                    next_remaining.push(node);
                }
            }

            if current_stage.is_empty() {
                return Err(GraphSolveError::CyclicDependency);
            }

            // Keep high-pressure nodes together and deterministic within a
            // stage. This is a scheduling hint, not a replacement for the
            // dependency ordering above.
            current_stage.sort_by_key(|id| std::cmp::Reverse(by_id[id].cache_weight));
            for &id in &current_stage {
                processed.insert(id);
            }

            stages.push(ExecutionStage {
                node_ids: current_stage,
            });
            remaining = next_remaining;
        }

        Ok(stages)
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphNode, GraphSolveError, GraphSolver};

    #[test]
    fn solves_dependencies_and_orders_cache_weight() {
        let nodes = [
            GraphNode {
                id: 1,
                dependencies: vec![],
                cache_weight: 1,
            },
            GraphNode {
                id: 2,
                dependencies: vec![],
                cache_weight: 10,
            },
            GraphNode {
                id: 3,
                dependencies: vec![1, 2],
                cache_weight: 0,
            },
        ];
        let stages = GraphSolver::solve(&nodes).unwrap();
        assert_eq!(stages[0].node_ids, vec![2, 1]);
        assert_eq!(stages[1].node_ids, vec![3]);
    }

    #[test]
    fn rejects_cycles_and_unknown_dependencies() {
        let cycle = [
            GraphNode {
                id: 1,
                dependencies: vec![2],
                cache_weight: 0,
            },
            GraphNode {
                id: 2,
                dependencies: vec![1],
                cache_weight: 0,
            },
        ];
        assert_eq!(
            GraphSolver::solve(&cycle),
            Err(GraphSolveError::CyclicDependency)
        );

        let unknown = [GraphNode {
            id: 1,
            dependencies: vec![99],
            cache_weight: 0,
        }];
        assert_eq!(
            GraphSolver::solve(&unknown),
            Err(GraphSolveError::UnknownDependency {
                node_id: 1,
                dependency_id: 99
            })
        );
    }
}
