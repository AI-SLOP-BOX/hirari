//! Tests split out of pdc_manager.rs.

    use super::*;

    #[test]
    fn pdc_round_trip_preserves_original_and_constrained_plugin_state() {
        let mut manager = ConstrainDelayCompensation {
            enabled: true,
            threshold_samples: 64,
            channels: vec![],
        };
        assert!(manager.upsert_channel(PdcChannelState {
            channel_id: 1,
            kind: PdcChannelKind::Instrument,
            record_enabled: false,
            plugins: vec![PluginDelayState {
                plugin_id: "linear-phase".into(),
                reported_latency: 1024,
                low_latency_samples: 0,
                supports_low_latency: false,
                enabled: true,
                low_latency_active: false,
                bypassed_by_constraint: false
            }]
        }));
        let json = manager.to_json().unwrap();
        assert_eq!(
            ConstrainDelayCompensation::from_json(&json).unwrap(),
            manager
        );
        assert!(ConstrainDelayCompensation::from_json(
            "{\"enabled\":true,\"threshold_samples\":0,\"channels\":null}"
        )
        .is_err());
    }
