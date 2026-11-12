//! Tests split out of tempo.rs.
    use super::{
        hirari_tempo_map_add_tempo, hirari_tempo_map_add_time_signature,
        hirari_tempo_map_beats_to_samples, hirari_tempo_map_clear, hirari_tempo_map_copy_events,
        hirari_tempo_map_copy_signatures, hirari_tempo_map_create, hirari_tempo_map_destroy,
        hirari_tempo_map_get_current_bpm, hirari_tempo_map_get_event_at,
        hirari_tempo_map_get_event_count, hirari_tempo_map_get_signature_at,
        hirari_tempo_map_get_signature_count, hirari_tempo_map_remove_time_signature,
        hirari_tempo_map_samples_to_beats, hirari_tempo_map_set_current_bpm, NativeTempoMapEvent,
        NativeTimeSignatureEvent,
    };

    #[test]
    fn rust_owned_tempo_map_matches_stateless_ffi_and_keeps_signature_snapshots() {
        let map = hirari_tempo_map_create();
        assert!(!map.is_null());
        unsafe {
            assert_eq!(hirari_tempo_map_get_current_bpm(map), 120.0);
            hirari_tempo_map_set_current_bpm(map, 137.5);
            assert_eq!(hirari_tempo_map_get_current_bpm(map), 137.5);
            assert!(hirari_tempo_map_clear(map, 48_000.0, 120.0));
            assert!(hirari_tempo_map_add_tempo(
                map, 96_000, 90.0, 48_000.0, true
            ));
            assert_eq!(hirari_tempo_map_get_event_count(map), 2);

            let mut events = [NativeTempoMapEvent {
                sample_pos: 0,
                bpm: 0.0,
                ramp: false,
                world_beats: 0.0,
            }; 2];
            assert_eq!(
                hirari_tempo_map_copy_events(map, events.as_mut_ptr().cast(), events.len()),
                2
            );
            assert_eq!(events[1].sample_pos, 96_000);
            assert!(events[1].ramp);
            assert_eq!(
                hirari_tempo_map_samples_to_beats(map, 48_000, 48_000.0),
                2.0
            );
            assert_eq!(
                hirari_tempo_map_beats_to_samples(map, 2.0, 48_000.0),
                48_000
            );

            let mut event = events[0];
            assert!(hirari_tempo_map_get_event_at(
                map,
                0,
                (&mut event as *mut NativeTempoMapEvent).cast::<std::ffi::c_void>(),
            ));
            assert_eq!(event.bpm, 120.0);

            assert!(hirari_tempo_map_add_time_signature(
                map, 4.0, 3, 4, 48_000.0
            ));
            assert_eq!(hirari_tempo_map_get_signature_count(map), 2);
            let mut signatures = [NativeTimeSignatureEvent {
                sample_pos: 0,
                numerator: 0,
                denominator: 0,
                beat: 0.0,
            }; 2];
            assert_eq!(
                hirari_tempo_map_copy_signatures(
                    map,
                    signatures.as_mut_ptr().cast(),
                    signatures.len(),
                ),
                2
            );
            assert_eq!(signatures[1].beat, 4.0);
            let mut signature = signatures[0];
            assert!(hirari_tempo_map_get_signature_at(
                map,
                4.0,
                (&mut signature as *mut NativeTimeSignatureEvent).cast::<std::ffi::c_void>(),
            ));
            assert_eq!((signature.numerator, signature.denominator), (3, 4));
            assert!(hirari_tempo_map_remove_time_signature(map, 4.0, 48_000.0));
            assert_eq!(hirari_tempo_map_get_signature_count(map), 1);
            hirari_tempo_map_destroy(map);
        }
    }
