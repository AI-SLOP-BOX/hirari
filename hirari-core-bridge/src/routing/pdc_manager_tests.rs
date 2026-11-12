//! Tests split out of pdc_manager.rs.

    use super::*;

    #[test]
    fn recalculates_track_and_bus_delay_offsets() {
        let mut pdc = PdcOrchestrator::new();
        pdc.track_latencies[0] = 128;
        pdc.track_latencies[1] = 512;
        pdc.bus_latencies[0] = 64;
        pdc.bus_latencies[1] = 256;

        pdc.recalculate();

        assert_eq!(pdc.track_offsets[0], 384);
        assert_eq!(pdc.track_offsets[1], 0);
        assert_eq!(pdc.bus_offsets[0], 192);
        assert_eq!(pdc.bus_offsets[1], 0);
        assert!(pdc.audit_pdc_manager());
    }

    fn plugin(id: &str, latency: u32, supports_low_latency: bool) -> PluginDelayState {
        PluginDelayState {
            plugin_id: id.into(),
            reported_latency: latency,
            low_latency_samples: if supports_low_latency { 8 } else { 0 },
            supports_low_latency,
            enabled: true,
            low_latency_active: false,
            bypassed_by_constraint: false,
        }
    }

    #[test]
    fn constrains_only_eligible_channels_and_restores_plugins() {
        let mut manager = ConstrainDelayCompensation {
            enabled: false,
            threshold_samples: 32,
            channels: vec![],
        };
        assert!(manager.upsert_channel(PdcChannelState {
            channel_id: 1,
            kind: PdcChannelKind::AudioTrack,
            record_enabled: true,
            plugins: vec![plugin("live", 128, true), plugin("legacy", 64, false)]
        }));
        assert!(manager.upsert_channel(PdcChannelState {
            channel_id: 2,
            kind: PdcChannelKind::Effect,
            record_enabled: false,
            plugins: vec![plugin("reverb", 256, false)]
        }));

        manager.set_enabled(true);

        assert_eq!(manager.latency_for(1), Some(8));
        assert_eq!(manager.latency_for(2), Some(256));
        assert!(manager.channels[0].plugins[0].low_latency_active);
        assert!(manager.channels[0].plugins[1].bypassed_by_constraint);
        assert!(!manager.channels[1].plugins[0].bypassed_by_constraint);

        manager.set_enabled(false);
        assert_eq!(manager.latency_for(1), Some(192));
        assert!(manager
            .channels
            .iter()
            .flat_map(|channel| &channel.plugins)
            .all(|plugin| !plugin.low_latency_active && !plugin.bypassed_by_constraint));
        assert!(manager.audit());
    }

    #[test]
    fn record_arm_transition_recomputes_audio_track_constraint() {
        let mut manager = ConstrainDelayCompensation {
            enabled: true,
            threshold_samples: 0,
            channels: vec![],
        };
        assert!(manager.upsert_channel(PdcChannelState {
            channel_id: 7,
            kind: PdcChannelKind::AudioTrack,
            record_enabled: false,
            plugins: vec![plugin("lookahead", 512, false)]
        }));
        assert_eq!(manager.latency_for(7), Some(512));
        assert!(manager.set_record_enabled(7, true));
        assert_eq!(manager.latency_for(7), Some(0));
        assert!(manager.set_record_enabled(7, false));
        assert_eq!(manager.latency_for(7), Some(512));
    }

    #[test]
    fn routing_graph_compensates_tracks_sends_groups_and_outputs() {
        let graph = PdcRoutingGraph {
            nodes: vec![
                PdcRoutingNode {
                    channel_id: 1,
                    plugin_latency_samples: 100,
                    external_latency_samples: 0,
                    routes_to: vec![3, 6],
                    output: false,
                },
                PdcRoutingNode {
                    channel_id: 2,
                    plugin_latency_samples: 20,
                    external_latency_samples: 0,
                    routes_to: vec![3, 4],
                    output: false,
                },
                PdcRoutingNode {
                    channel_id: 4,
                    plugin_latency_samples: 200,
                    external_latency_samples: 0,
                    routes_to: vec![3],
                    output: false,
                },
                PdcRoutingNode {
                    channel_id: 3,
                    plugin_latency_samples: 30,
                    external_latency_samples: 0,
                    routes_to: vec![5],
                    output: false,
                },
                PdcRoutingNode {
                    channel_id: 5,
                    plugin_latency_samples: 10,
                    external_latency_samples: 0,
                    routes_to: vec![],
                    output: true,
                },
                PdcRoutingNode {
                    channel_id: 6,
                    plugin_latency_samples: 0,
                    external_latency_samples: 0,
                    routes_to: vec![],
                    output: true,
                },
            ],
        };
        let schedule = graph.schedule().unwrap();
        assert_eq!(schedule.output_latency_by_channel[&3], 250);
        assert_eq!(schedule.maximum_output_latency, 260);
        assert_eq!(schedule.final_output_compensation[&6], 160);
        assert!(schedule
            .edge_compensation
            .iter()
            .any(|edge| edge.source == 1 && edge.destination == 3 && edge.delay_samples == 120));
        assert!(schedule
            .edge_compensation
            .iter()
            .any(|edge| edge.source == 2 && edge.destination == 3 && edge.delay_samples == 200));
        let json = graph.to_json().unwrap();
        assert_eq!(PdcRoutingGraph::from_json(&json).unwrap(), graph);
    }

    #[test]
    fn routing_graph_rejects_feedback_cycles_and_unknown_destinations() {
        let cyclic = PdcRoutingGraph {
            nodes: vec![
                PdcRoutingNode {
                    channel_id: 1,
                    plugin_latency_samples: 0,
                    external_latency_samples: 0,
                    routes_to: vec![2],
                    output: true,
                },
                PdcRoutingNode {
                    channel_id: 2,
                    plugin_latency_samples: 0,
                    external_latency_samples: 0,
                    routes_to: vec![1],
                    output: false,
                },
            ],
        };
        assert!(!cyclic.validate());
        assert!(cyclic.schedule().is_err());
        let unknown = PdcRoutingGraph {
            nodes: vec![PdcRoutingNode {
                channel_id: 1,
                plugin_latency_samples: 0,
                external_latency_samples: 0,
                routes_to: vec![99],
                output: true,
            }],
        };
        assert!(!unknown.validate());
    }
