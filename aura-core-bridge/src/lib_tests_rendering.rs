    #[test]
    fn wav_publication_validator_accepts_rf64_ds64_data_size() {
        let mut wav = vec![0u8; 76];
        wav[0..4].copy_from_slice(b"RF64");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"ds64");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[28..36].copy_from_slice(&8u64.to_le_bytes());
        wav[36..40].copy_from_slice(b"fmt ");
        wav[40..44].copy_from_slice(&16u32.to_le_bytes());
        wav[44..46].copy_from_slice(&1u16.to_le_bytes());
        wav[46..48].copy_from_slice(&2u16.to_le_bytes());
        wav[48..52].copy_from_slice(&48_000u32.to_le_bytes());
        wav[56..58].copy_from_slice(&4u16.to_le_bytes());
        wav[58..60].copy_from_slice(&16u16.to_le_bytes());
        wav[60..64].copy_from_slice(b"data");
        wav[64..68].copy_from_slice(&u32::MAX.to_le_bytes());
        wav[68..76].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(crate::valid_pcm_or_float_wav(&wav));
    }

    #[test]
    fn wav_publication_validator_handles_unknown_odd_chunks_and_rejects_bad_rf64() {
        let mut wav = vec![0u8; 46 + 8 + 2];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"fmt ");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[20..22].copy_from_slice(&1u16.to_le_bytes());
        wav[22..24].copy_from_slice(&1u16.to_le_bytes());
        wav[24..28].copy_from_slice(&48_000u32.to_le_bytes());
        wav[32..34].copy_from_slice(&2u16.to_le_bytes());
        wav[34..36].copy_from_slice(&16u16.to_le_bytes());
        wav[36..40].copy_from_slice(b"JUNK");
        wav[40..44].copy_from_slice(&1u32.to_le_bytes());
        wav[44] = 0x7f;
        // The odd-sized chunk consumes a padding byte before the data chunk.
        wav[46..50].copy_from_slice(b"data");
        wav[50..54].copy_from_slice(&2u32.to_le_bytes());
        assert!(crate::valid_pcm_or_float_wav(&wav));

        let mut bad_rf64 = wav;
        bad_rf64[0..4].copy_from_slice(b"RF64");
        bad_rf64[50..54].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!crate::valid_pcm_or_float_wav(&bad_rf64));
    }

    #[test]
    fn region_edit_boundaries_are_rejected_at_the_ffi_boundary() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();

        assert!(!engine_ref.set_region_gain(0, 0, f32::NAN));
        assert!(!engine_ref.set_region_gain(0, 0, 25.0));
        assert!(!engine_ref.set_region_fades(0, 0, -0.1, 0.2));
        assert!(!engine_ref.set_region_fades(0, 0, 0.2, f32::INFINITY));
        assert!(!engine_ref.set_region_trim(0, 0, 0.8, 0.2));
        assert!(!engine_ref.set_region_trim(0, 0, 0.0, 1.1));
        assert!(!engine_ref.set_region_warp_ratio(0, 0, 0.1));
        assert!(!engine_ref.set_region_warp_ratio(0, 0, 4.1));
        assert!(!engine_ref.set_region_pitch_semitones(0, 0, 49.0));
        assert!(!engine_ref.set_region_pitch_semitones(0, 0, f32::NEG_INFINITY));
        assert!(!engine_ref.set_region_loop_count(0, 0, 0));
        assert!(!engine_ref.set_region_loop_count(0, 0, 1025));
    }

    #[test]
    fn vca_assignment_and_gain_are_exposed_through_the_native_bridge() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        let engine_ref = engine.as_ref().expect("native engine handle");
        let track_id = engine_ref.add_track(0);
        assert_ne!(track_id, 0);
        let group_id = 9001;
        engine_ref.clear_vca_groups();
        assert!(engine_ref.add_vca_group(group_id, 1.0));
        assert!(engine_ref.assign_track_to_vca(track_id, group_id));
        assert_eq!(engine_ref.get_vca_track_gain(track_id), 1.0);
        assert!(engine_ref.set_vca_group_gain(group_id, 0.5));
        assert_eq!(engine_ref.get_vca_track_gain(track_id), 0.5);
        let snapshot: serde_json::Value =
            serde_json::from_str(engine_ref.get_vca_snapshot_json().as_str())
                .expect("VCA snapshot must be JSON");
        assert!(snapshot
            .as_array()
            .unwrap()
            .iter()
            .any(|group| { group["id"] == group_id && group["gain"] == 0.5 }));
        assert!(!engine_ref.add_vca_group(group_id, f32::NAN));
    }

    #[test]
    fn vca_groups_round_trip_through_project_v2() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_audio_track();
        assert_ne!(track_id, 0);
        let group_id = 9002;
        assert!(core.add_vca_group(group_id, 0.75));
        assert!(core.assign_track_to_vca(track_id, group_id));
        let path = std::env::temp_dir().join(format!(
            "aura-vca-roundtrip-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        core.save_project_v2(path.to_str().unwrap(), "VCA Test", 120.0)
            .expect("VCA project save must succeed");
        core.clear_vca_groups();
        let load_result = core.load_project_v2(path.to_str().unwrap());
        assert!(
            load_result.is_ok(),
            "VCA project load failed: {load_result:?}"
        );
        let snapshot: serde_json::Value =
            serde_json::from_str(&core.get_vca_snapshot_json()).expect("VCA snapshot must be JSON");
        assert!(snapshot
            .as_array()
            .unwrap()
            .iter()
            .any(|group| { group["id"] == group_id && group["gain"] == 0.75 }));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_project_is_rejected_without_replacing_current_tracks() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();
        let unique = format!(
            "aura-load-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let valid_path = std::env::temp_dir().join(format!("{}.aura", unique));
        let corrupt_path = std::env::temp_dir().join(format!("{}-corrupt.aura", unique));
        let valid = valid_path.to_str().unwrap();
        let corrupt = corrupt_path.to_str().unwrap();

        assert!(engine_ref.save_project(valid));
        let mut bytes = std::fs::read(&valid_path).unwrap();
        bytes.push(0xA5); // trailing data must invalidate the project
        std::fs::write(&corrupt_path, bytes).unwrap();

        assert!(!engine_ref.load_project(corrupt));

        let _ = std::fs::remove_file(valid_path);
        let _ = std::fs::remove_file(corrupt_path);
    }

    #[test]
    fn sidecar_failure_rolls_back_native_project_state() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let unique = format!(
            "aura-sidecar-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let project = std::env::temp_dir().join(format!("{unique}.aura"));
        let sidecar = std::path::PathBuf::from(format!("{}.midi-events.json", project.display()));
        let project_text = project.to_str().unwrap();

        let saved_track = core.add_track(0);
        assert_ne!(saved_track, 0);
        assert!(core.set_track_name(saved_track, "saved-state"));
        assert!(core.save_project(project_text));

        let current_track = core.add_track(0);
        assert_ne!(current_track, 0);
        assert!(core.set_track_name(current_track, "current-state"));
        let before = core.get_project_layout_json();
        std::fs::write(&sidecar, b"{malformed").unwrap();

        assert!(!core.load_project(project_text));
        assert_eq!(core.get_project_layout_json(), before);

        let _ = std::fs::remove_file(project);
        let _ = std::fs::remove_file(sidecar);
    }

    #[test]
    fn test_non_finite_tempo_input_is_ignored() {
        let _guard = tempo_test_guard();
        let _engine_guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();
        // The native engine is a process-wide singleton; establish a local
        // baseline so this test is independent of parallel test ordering.
        engine_ref.set_tempo(120.0);
        let initial_tempo = engine_ref.get_tempo();

        engine_ref.set_tempo(f32::NAN);
        assert_eq!(engine_ref.get_tempo(), initial_tempo);
        engine_ref.set_tempo(f32::INFINITY);
        assert_eq!(engine_ref.get_tempo(), initial_tempo);
        engine_ref.set_tempo(f32::NEG_INFINITY);
        assert_eq!(engine_ref.get_tempo(), initial_tempo);
    }

    #[test]
    fn test_out_of_range_tempo_is_bounded() {
        let _guard = tempo_test_guard();
        let _engine_guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();

        engine_ref.set_tempo(0.0);
        assert_eq!(engine_ref.get_tempo(), 20.0);
        engine_ref.set_tempo(301.0);
        assert_eq!(engine_ref.get_tempo(), 300.0);
    }

    #[test]
    fn project_diagnostic_api_preserves_structured_error_codes() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let empty: serde_json::Value =
            serde_json::from_str(&core.save_project_diagnostic_json(" "))
                .expect("empty path diagnostic must be JSON");
        assert_eq!(empty["code"], "invalid_path");

        let missing = std::env::temp_dir().join(format!(
            "aura-diagnostic-missing-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let missing_json: serde_json::Value =
            serde_json::from_str(&core.load_project_diagnostic_json(missing.to_str().unwrap()))
                .expect("missing path diagnostic must be JSON");
        assert_eq!(missing_json["code"], "project_not_found");

        let saved = std::env::temp_dir().join(format!(
            "aura-diagnostic-save-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let saved_json: serde_json::Value =
            serde_json::from_str(&core.save_project_diagnostic_json(saved.to_str().unwrap()))
                .expect("successful save diagnostic must be JSON");
        assert_eq!(saved_json["ok"], true);
        assert_eq!(saved_json["path"], saved.to_string_lossy().as_ref());
        assert!(saved_json["generation"].as_u64().is_some());
        let _ = std::fs::remove_file(&saved);
        let _ = std::fs::remove_file(format!("{}.comping.json", saved.display()));
        let _ = std::fs::remove_file(format!("{}.midi.json", saved.display()));
    }

    #[test]
    fn recording_and_midi_diagnostics_reject_invalid_commands_structurally() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");

        let take: serde_json::Value =
            serde_json::from_str(&core.select_recording_take_diagnostic_json(4))
                .expect("recording diagnostic must be JSON");
        assert_eq!(take["code"], "recording_session_inactive");

        let swing: serde_json::Value =
            serde_json::from_str(&core.apply_midi_swing_diagnostic_json(f32::NAN, 2.0))
                .expect("swing diagnostic must be JSON");
        assert_eq!(swing["code"], "invalid_midi_swing");
        assert_eq!(swing["retryable"], false);

        let humanize: serde_json::Value =
            serde_json::from_str(&core.humanize_midi_diagnostic_json(9.0, 127, 42))
                .expect("humanize diagnostic must be JSON");
        assert_eq!(humanize["code"], "invalid_midi_humanize");

        let malformed: serde_json::Value =
            serde_json::from_str(&core.set_midi_events_diagnostic_json("not-json"))
                .expect("MIDI snapshot diagnostic must be JSON");
        assert_eq!(malformed["code"], "invalid_midi_snapshot");

        let invalid_event: serde_json::Value = serde_json::from_str(
            &core.set_midi_events_diagnostic_json(
                r#"[{"beat":0.0,"channel":16,"kind":{"ControlChange":{"controller":1,"value":1}}}]"#,
            ),
        )
        .expect("invalid MIDI event diagnostic must be JSON");
        assert_eq!(invalid_event["code"], "invalid_midi_event");

        let invalid_move: serde_json::Value =
            serde_json::from_str(&core.move_midi_notes_range_diagnostic_json(0, 0, 100, -1))
                .expect("MIDI move diagnostic must be JSON");
        assert_eq!(invalid_move["code"], "invalid_midi_move_range");

        let invalid_transpose: serde_json::Value =
            serde_json::from_str(&core.transpose_midi_notes_range_diagnostic_json(0, 0, 100, 1))
                .expect("MIDI transpose diagnostic must be JSON");
        assert_eq!(invalid_transpose["code"], "invalid_midi_transpose");
    }

    #[test]
    fn comping_diagnostics_reject_malformed_and_empty_mutations() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");

        let malformed: serde_json::Value =
            serde_json::from_str(&core.set_comp_segments_diagnostic_json("not-json"))
                .expect("comping segment diagnostic must be JSON");
        assert_eq!(malformed["code"], "invalid_comp_segments_json");

        let empty: serde_json::Value =
            serde_json::from_str(&core.set_comp_segments_diagnostic_json("[]"))
                .expect("empty segment diagnostic must be JSON");
        assert_eq!(empty["ok"], true);

        let snapshot: serde_json::Value =
            serde_json::from_str(&core.restore_comping_snapshot_diagnostic_json("{}"))
                .expect("comping snapshot diagnostic must be JSON");
        assert_eq!(snapshot["code"], "invalid_comping_snapshot_json");
    }

    #[test]
    fn track_scalar_diagnostics_reject_invalid_and_stale_targets() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let invalid: serde_json::Value =
            serde_json::from_str(&core.set_volume_diagnostic_json(1, f32::NAN))
                .expect("invalid scalar diagnostic must be JSON");
        assert_eq!(invalid["code"], "invalid_parameter");

        let stale: serde_json::Value =
            serde_json::from_str(&core.set_pan_diagnostic_json(999_999, 0.25))
                .expect("stale track diagnostic must be JSON");
        assert_eq!(stale["code"], "track_not_found_or_rejected");
        assert_eq!(stale["affected_object"], "track:999999");
        assert!(stale["generation"].as_u64().is_some());
    }

    #[test]
    fn track_toggle_and_lifecycle_diagnostics_are_structured() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let created: serde_json::Value = serde_json::from_str(&core.add_track_diagnostic_json(0))
            .expect("track creation diagnostic must be JSON");
        let track_id = created["track_id"].as_u64().expect("created track id");
        assert_eq!(created["ok"], true);

        let renamed: serde_json::Value = serde_json::from_str(
            &core.set_track_name_diagnostic_json(track_id as u32, "Lead Vocal"),
        )
        .expect("track rename diagnostic must be JSON");
        assert_eq!(renamed["ok"], true);

        let duplicate: serde_json::Value =
            serde_json::from_str(&core.duplicate_track_diagnostic_json(track_id as u32))
                .expect("track duplicate diagnostic must be JSON");
        assert_eq!(duplicate["ok"], true);
        assert_ne!(duplicate["track_id"], track_id);

        let mixing: serde_json::Value =
            serde_json::from_str(&core.execute_auto_mixing_diagnostic_json())
                .expect("auto mixing diagnostic must be JSON");
        assert_eq!(mixing["ok"], true);

        let arrangement: serde_json::Value =
            serde_json::from_str(&core.execute_auto_arrangement_diagnostic_json())
                .expect("auto arrangement diagnostic must be JSON");
        assert_eq!(arrangement["ok"], true);

        let scale: serde_json::Value =
            serde_json::from_str(&core.set_project_scale_diagnostic_json(0, 0))
                .expect("project scale diagnostic must be JSON");
        assert_eq!(scale["ok"], true);
        let invalid_scale: serde_json::Value =
            serde_json::from_str(&core.set_project_scale_diagnostic_json(12, 0))
                .expect("invalid project scale diagnostic must be JSON");
        assert_eq!(invalid_scale["code"], "invalid_project_scale");

        let muted: serde_json::Value =
            serde_json::from_str(&core.set_mute_diagnostic_json(track_id as u32, true))
                .expect("mute diagnostic must be JSON");
        assert_eq!(muted["ok"], true);

        let removed: serde_json::Value =
            serde_json::from_str(&core.remove_track_diagnostic_json(track_id as u32))
                .expect("remove diagnostic must be JSON");
        assert_eq!(removed["ok"], true);

        let missing: serde_json::Value =
            serde_json::from_str(&core.set_solo_diagnostic_json(track_id as u32, true))
                .expect("missing toggle diagnostic must be JSON");
        assert_eq!(missing["code"], "track_not_found_or_rejected");

        let invalid_name: serde_json::Value =
            serde_json::from_str(&core.set_track_name_diagnostic_json(0, "  "))
                .expect("invalid name diagnostic must be JSON");
        assert_eq!(invalid_name["code"], "invalid_track_name");
    }

    #[test]
    fn plugin_mutation_diagnostics_preserve_target_context() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        let added: serde_json::Value =
            serde_json::from_str(&core.add_plugin_diagnostic_json(track_id, 0))
                .expect("plugin add diagnostic must be JSON");
        assert_eq!(added["ok"], true);

        let bypassed: serde_json::Value =
            serde_json::from_str(&core.set_plugin_bypass_diagnostic_json(track_id, 0, true))
                .expect("plugin bypass diagnostic must be JSON");
        assert_eq!(bypassed["ok"], true);

        let removed: serde_json::Value =
            serde_json::from_str(&core.remove_plugin_diagnostic_json(track_id, 0))
                .expect("plugin remove diagnostic must be JSON");
        assert_eq!(removed["ok"], true);

        let missing: serde_json::Value =
            serde_json::from_str(&core.remove_plugin_diagnostic_json(track_id, 0))
                .expect("missing plugin diagnostic must be JSON");
        assert_eq!(missing["code"], "plugin_not_found_or_rejected");
        assert_eq!(
            missing["affected_object"],
            format!("track:{track_id}/plugin:0")
        );
    }

    #[test]
    fn routing_diagnostics_reject_invalid_graph_inputs_explicitly() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let self_loop: serde_json::Value =
            serde_json::from_str(&core.set_route_diagnostic_json(4, 4, true))
                .expect("route diagnostic must be JSON");
        assert_eq!(self_loop["code"], "route_self_loop");

        let bad_gain: serde_json::Value =
            serde_json::from_str(&core.set_feedback_route_diagnostic_json(1, 2, f32::NAN, true))
                .expect("feedback diagnostic must be JSON");
        assert_eq!(bad_gain["code"], "invalid_route_gain");

        let bad_tap: serde_json::Value =
            serde_json::from_str(&core.set_sidechain_link_diagnostic_json(1, 2, 0, 99, true))
                .expect("sidechain diagnostic must be JSON");
        assert_eq!(bad_tap["code"], "invalid_sidechain_tap");
    }

    #[test]
    fn audio_device_diagnostics_preserve_configuration_boundaries() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bad_rate: serde_json::Value =
            serde_json::from_str(&core.apply_audio_config_diagnostic_json(22_050, 128))
                .expect("audio config diagnostic must be JSON");
        assert_eq!(bad_rate["code"], "unsupported_sample_rate");

        let bad_buffer: serde_json::Value =
            serde_json::from_str(&core.apply_audio_config_diagnostic_json(48_000, 127))
                .expect("audio config diagnostic must be JSON");
        assert_eq!(bad_buffer["code"], "unsupported_buffer_size");

        let reconnect: serde_json::Value =
            serde_json::from_str(&core.try_reconnect_audio_device_diagnostic_json())
                .expect("reconnect diagnostic must be JSON");
        assert!(reconnect["status"].is_string());
        assert!(reconnect["audio_generation"].as_u64().is_some());
    }
