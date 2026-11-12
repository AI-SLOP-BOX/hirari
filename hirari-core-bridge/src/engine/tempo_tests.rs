//! Tests split out of tempo.rs.
    use super::{TempoEvent, TempoOrchestrator};

    #[test]
    fn normalizes_unsorted_duplicate_and_invalid_events() {
        let mut tempo = TempoOrchestrator {
            events: vec![
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 90.0,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 0,
                    bpm: f64::NAN,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 100.0,
                    ramp: true,
                    world_beats: 0.0,
                },
            ],
            tap_tempo: super::TapTempo::default(),
        };
        tempo.recalculate_integrated_time(44_100.0);
        assert!(tempo.audit_tempo());
        assert_eq!(tempo.events.len(), 2);
        assert_eq!(tempo.events[0].sample_pos, 0);
        assert_eq!(tempo.events[1].sample_pos, 44_100);
        assert_eq!(tempo.events[1].bpm, 100.0);
    }

    #[test]
    fn tap_updates_origin_tempo() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(0).is_none());
        assert_eq!(tempo.tap_and_set_tempo(500), Some(120.0));
        assert_eq!(tempo.events[0].bpm, 120.0);
    }

    #[test]
    fn non_monotonic_taps_are_ignored() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(1000).is_none());
        assert!(tempo.tap_and_set_tempo(900).is_none());
        assert_eq!(tempo.tap_and_set_tempo(1500), Some(120.0));
    }

    #[test]
    fn time_warp_moves_nodes_without_collisions() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 48_000,
            bpm: 100.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 96_000,
            bpm: 110.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.move_event(96_000, 72_000));
        assert!(!tempo.move_event(72_000, 48_000));
        assert!(tempo.set_ramp(72_000, true));
        assert!(tempo.audit_tempo());
    }
