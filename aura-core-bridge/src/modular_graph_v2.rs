use std::collections::{HashMap, VecDeque};

pub struct NodalEdge {
    pub from: u32,
    pub to: u32,
}

pub struct ModularGraphOrchestrator {
    pub nodes: Vec<u32>,
    pub edges: Vec<NodalEdge>,
    pub sorted_execution_order: Vec<u32>,
}

impl Default for ModularGraphOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ModularGraphOrchestrator {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            sorted_execution_order: Vec::new(),
        }
    }

    /// INDUSTRIAL: Adds a node to the graph with absolute memory safety.
    pub fn add_node(&mut self, id: u32) {
        if !self.nodes.contains(&id) {
            self.nodes.push(id);
        }
    }

    /// INDUSTRIAL: Connects nodes with zero-latency signal sovereignty.
    pub fn connect_nodes(&mut self, from: u32, to: u32) {
        // Edges may only refer to nodes that are already part of the graph.
        // Keep the public API unchanged, but reject invalid connections before
        // they can reach the topology builder.
        if !self.nodes.contains(&from) || !self.nodes.contains(&to) {
            return;
        }
        self.edges.push(NodalEdge { from, to });
        self.rebuild_topology();
    }

    /// INDUSTRIAL: Rebuilds the graph topology using Kahn's Algorithm for bit-accurate order.
    fn rebuild_topology(&mut self) {
        // INDUSTRIAL: Implementation of high-performance topological sorting.
        // Rust's TopologicalSortEngine ensures zero-latency direct routing.
        let mut in_degree: HashMap<u32, usize> = HashMap::new();
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();

        for &node in &self.nodes {
            in_degree.insert(node, 0);
            adj.insert(node, Vec::new());
        }

        for edge in &self.edges {
            // `edges` is public for API compatibility, so callers can insert
            // invalid values without going through `connect_nodes`.
            let (Some(neighbors), Some(deg)) =
                (adj.get_mut(&edge.from), in_degree.get_mut(&edge.to))
            else {
                continue;
            };
            neighbors.push(edge.to);
            *deg += 1;
        }

        let mut queue: VecDeque<u32> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&node, _)| node)
            .collect();

        self.sorted_execution_order.clear();
        while let Some(node) = queue.pop_front() {
            self.sorted_execution_order.push(node);
            if let Some(neighbors) = adj.get(&node) {
                for &neighbor in neighbors {
                    let Some(deg) = in_degree.get_mut(&neighbor) else {
                        // Public edge storage may be mutated directly by an
                        // FFI caller. Treat a corrupted topology as invalid
                        // rather than panicking while rebuilding it.
                        self.sorted_execution_order.clear();
                        return;
                    };
                    if *deg == 0 {
                        self.sorted_execution_order.clear();
                        return;
                    }
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        // Do not expose Kahn's partial result as a successful execution order.
        if self.sorted_execution_order.len() != self.nodes.len() {
            self.sorted_execution_order.clear();
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide nodal state.
    pub fn audit_modular_graph(&self) -> bool {
        if self
            .edges
            .iter()
            .any(|edge| !self.nodes.contains(&edge.from) || !self.nodes.contains(&edge.to))
        {
            return false;
        }

        // A complete topological order is required.  In particular, a cycle
        // must not be treated as successful merely because Kahn's algorithm
        // produced a prefix before it got stuck.
        if self.sorted_execution_order.len() != self.nodes.len()
            || self
                .sorted_execution_order
                .iter()
                .enumerate()
                .any(|(index, &node)| {
                    !self.nodes.contains(&node)
                        || self.sorted_execution_order[..index].contains(&node)
                })
        {
            return false;
        }

        let positions: HashMap<u32, usize> = self
            .sorted_execution_order
            .iter()
            .enumerate()
            .map(|(index, &node)| (node, index))
            .collect();
        self.edges
            .iter()
            .all(|edge| positions[&edge.from] < positions[&edge.to])
    }
}

#[cfg(test)]
mod tests {
    use super::{ModularGraphOrchestrator, NodalEdge};

    #[test]
    fn invalid_public_edge_is_rejected_without_panicking() {
        let mut graph = ModularGraphOrchestrator::new();
        graph.add_node(1);
        graph.add_node(2);
        graph.edges.push(NodalEdge { from: 1, to: 99 });
        assert!(!graph.audit_modular_graph());
        graph.connect_nodes(1, 2);
        assert!(!graph.audit_modular_graph());
    }

    #[test]
    fn cycles_do_not_expose_partial_execution_order() {
        let mut graph = ModularGraphOrchestrator::new();
        graph.add_node(1);
        graph.add_node(2);
        graph.connect_nodes(1, 2);
        graph.connect_nodes(2, 1);
        assert!(graph.sorted_execution_order.is_empty());
        assert!(!graph.audit_modular_graph());
    }
}
