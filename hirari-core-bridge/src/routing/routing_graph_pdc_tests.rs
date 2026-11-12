//! Tests split out of routing_graph_pdc.rs.

    use super::{
        hirari_routing_graph_add_connection, hirari_routing_graph_add_dependency,
        hirari_routing_graph_build_order, hirari_routing_graph_create,
        hirari_routing_graph_destroy, hirari_routing_graph_remove_connections_for_node,
        hirari_routing_graph_snapshot_copy, hirari_routing_graph_snapshot_count,
        hirari_routing_graph_snapshot_create, hirari_routing_graph_snapshot_destroy, AudioNodeRust,
        RoutingConnectionRecord, RoutingOrchestrator,
    };
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

    #[test]
    fn rust_owned_live_graph_rejects_cycles_and_publishes_deterministic_order() {
        let state = hirari_routing_graph_create();
        assert!(!state.is_null());
        assert!(unsafe { hirari_routing_graph_add_connection(state, 1, 2, 0.5, false, false) });
        assert!(unsafe { hirari_routing_graph_add_dependency(state, 2, 3) });
        assert!(!unsafe { hirari_routing_graph_add_dependency(state, 3, 1) });

        let mut order = [0_u32; 128];
        let count =
            unsafe { hirari_routing_graph_build_order(state, order.as_mut_ptr(), order.len()) };
        assert_eq!(count, 3);
        assert_eq!(&order[..count], &[1, 2, 3]);

        let snapshot = unsafe { hirari_routing_graph_snapshot_create(state) };
        assert_eq!(unsafe { hirari_routing_graph_snapshot_count(snapshot) }, 1);
        let mut connection = [RoutingConnectionRecord::default()];
        assert_eq!(
            unsafe {
                hirari_routing_graph_snapshot_copy(
                    snapshot,
                    connection.as_mut_ptr().cast(),
                    connection.len(),
                )
            },
            1
        );
        assert_eq!(connection[0].gain, 0.5);
        unsafe { hirari_routing_graph_snapshot_destroy(snapshot) };
        unsafe { hirari_routing_graph_destroy(state) };
    }

    #[test]
    fn node_removal_returns_the_edges_needed_for_undo() {
        let state = hirari_routing_graph_create();
        assert!(unsafe { hirari_routing_graph_add_connection(state, 1, 7, 0.75, false, false) });
        assert!(unsafe { hirari_routing_graph_add_connection(state, 2, 7, 0.25, true, true) });
        assert!(unsafe { hirari_routing_graph_add_connection(state, 3, 4, 1.0, false, false) });

        let removed = unsafe { hirari_routing_graph_remove_connections_for_node(state, 7) };
        assert_eq!(unsafe { hirari_routing_graph_snapshot_count(removed) }, 2);
        let mut connections = [RoutingConnectionRecord::default(); 2];
        assert_eq!(
            unsafe {
                hirari_routing_graph_snapshot_copy(
                    removed,
                    connections.as_mut_ptr().cast(),
                    connections.len(),
                )
            },
            2
        );
        assert!(connections
            .iter()
            .any(|connection| connection.send && connection.pre_fader));
        let mut order = [0_u32; 128];
        let count =
            unsafe { hirari_routing_graph_build_order(state, order.as_mut_ptr(), order.len()) };
        assert_eq!(count, 2);
        assert_eq!(&order[..count], &[3, 4]);

        unsafe { hirari_routing_graph_snapshot_destroy(removed) };
        unsafe { hirari_routing_graph_destroy(state) };
    }
