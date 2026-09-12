    #[test]
    fn automation_diagnostics_reject_malformed_points_and_nonfinite_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let odd: serde_json::Value =
            serde_json::from_str(&core.set_automation_data_diagnostic_json(1, 2, vec![0.0, 0.5]))
                .expect("automation diagnostic must be JSON");
        assert_eq!(odd["code"], "invalid_automation_points");

        let nan: serde_json::Value = serde_json::from_str(
            &core.set_automation_data_diagnostic_json(1, 2, vec![0.0, f64::NAN, 0.0]),
        )
        .expect("automation diagnostic must be JSON");
        assert_eq!(nan["code"], "non_finite_automation_points");

        let parameter: serde_json::Value =
            serde_json::from_str(&core.set_plugin_parameter_diagnostic_json(1, 0, 2, f32::NAN))
                .expect("parameter diagnostic must be JSON");
        assert_eq!(parameter["code"], "non_finite_parameter");
    }

    #[test]
    fn tempo_diagnostics_reject_invalid_control_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bpm: serde_json::Value =
            serde_json::from_str(&core.set_tempo_diagnostic_json(f32::NAN))
                .expect("tempo diagnostic must be JSON");
        assert_eq!(bpm["code"], "non_finite_tempo");

        let event: serde_json::Value =
            serde_json::from_str(&core.set_tempo_event_diagnostic_json(-1.0, 120.0, false))
                .expect("tempo event diagnostic must be JSON");
        assert_eq!(event["code"], "tempo_event_out_of_range");

        let missing: serde_json::Value =
            serde_json::from_str(&core.remove_tempo_event_diagnostic_json(999.0))
                .expect("tempo removal diagnostic must be JSON");
        assert_eq!(missing["code"], "tempo_event_not_found");

        let macro_value: serde_json::Value =
            serde_json::from_str(&core.set_macro_value_diagnostic_json(128, 0.5))
                .expect("macro diagnostic must be JSON");
        assert_eq!(macro_value["code"], "invalid_macro_value");

        let synth: serde_json::Value =
            serde_json::from_str(&core.set_preview_synth_engine_diagnostic_json(3))
                .expect("preview synth diagnostic must be JSON");
        assert_eq!(synth["code"], "unsupported_preview_synth");

        let pad: serde_json::Value =
            serde_json::from_str(&core.assign_preview_drum_pad_diagnostic_json(16, None))
                .expect("preview pad diagnostic must be JSON");
        assert_eq!(pad["code"], "invalid_preview_pad");

        let scan: serde_json::Value = serde_json::from_str(
            &core.scan_preview_audio_diagnostic_json("/definitely/missing-preview-directory"),
        )
        .expect("preview scan diagnostic must be JSON");
        assert_eq!(scan["code"], "preview_directory_not_found");

        let preload: serde_json::Value =
            serde_json::from_str(&core.preload_preview_audio_diagnostic_json(99_999))
                .expect("preview preload diagnostic must be JSON");
        assert_eq!(preload["code"], "preview_asset_not_found");

        let trigger: serde_json::Value =
            serde_json::from_str(&core.trigger_preview_drum_pad_diagnostic_json(0))
                .expect("preview trigger diagnostic must be JSON");
        assert_eq!(trigger["code"], "preview_pad_unavailable");
    }

    #[test]
    fn region_and_video_diagnostics_reject_invalid_control_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bad_target: serde_json::Value =
            serde_json::from_str(&core.move_region_diagnostic_json(0, 0, 1.0))
                .expect("region diagnostic must be JSON");
        assert_eq!(bad_target["code"], "invalid_region_target");

        let bad_value: serde_json::Value =
            serde_json::from_str(&core.set_region_gain_diagnostic_json(1, 1, f32::NAN))
                .expect("region diagnostic must be JSON");
        assert_eq!(bad_value["code"], "invalid_region_value");

        let stale: serde_json::Value =
            serde_json::from_str(&core.set_region_trim_diagnostic_json(999_999, 999_999, 0.0, 1.0))
                .expect("region diagnostic must be JSON");
        assert_eq!(stale["code"], "region_not_found_or_rejected");
        assert_eq!(stale["affected_object"], "track:999999/region:999999");

        let bad_position: serde_json::Value =
            serde_json::from_str(&core.request_video_frame_diagnostic_json(f64::NAN))
                .expect("video diagnostic must be JSON");
        assert_eq!(bad_position["code"], "invalid_video_position");

        let missing: serde_json::Value = serde_json::from_str(
            &core.load_video_diagnostic_json("/definitely/missing/aura-video.mov"),
        )
        .expect("video diagnostic must be JSON");
        assert_eq!(missing["code"], "video_not_found");

        let render: serde_json::Value =
            serde_json::from_str(&core.start_render_diagnostic_json("/tmp/aura-output.mp3"))
                .expect("render diagnostic must be JSON");
        assert_eq!(render["code"], "invalid_render_path");

        let spatial: serde_json::Value =
            serde_json::from_str(&core.set_spatial_position_diagnostic_json(1, f32::NAN, 0.0, 0.0))
                .expect("spatial diagnostic must be JSON");
        assert_eq!(spatial["code"], "invalid_track_command");

        let phase: serde_json::Value =
            serde_json::from_str(&core.set_phase_invert_diagnostic_json(999_999, true))
                .expect("phase diagnostic must be JSON");
        assert_eq!(phase["code"], "track_not_found_or_rejected");

        let eq: serde_json::Value = serde_json::from_str(&core.set_track_eq_diagnostic_json(
            1,
            f32::INFINITY,
            0.0,
            0.0,
            1.0,
        ))
        .expect("EQ diagnostic must be JSON");
        assert_eq!(eq["code"], "invalid_track_command");

        let missing_eq: serde_json::Value =
            serde_json::from_str(&core.set_track_eq_diagnostic_json(999_999, 0.0, 0.0, 0.0, 1.0))
                .expect("missing EQ target diagnostic must be JSON");
        assert_eq!(missing_eq["code"], "track_not_found_or_rejected");

        let reverse: serde_json::Value =
            serde_json::from_str(&core.set_region_reverse_diagnostic_json(0, 1, true))
                .expect("reverse diagnostic must be JSON");
        assert_eq!(reverse["code"], "invalid_region_target");

        let vocal: serde_json::Value =
            serde_json::from_str(&core.execute_vocal_remover_diagnostic_json(999_999))
                .expect("vocal remover diagnostic must be JSON");
        assert_eq!(vocal["code"], "track_not_found_or_rejected");

        let articulation: serde_json::Value =
            serde_json::from_str(&core.set_articulation_map_diagnostic_json(999_999, "legato"))
                .expect("articulation diagnostic must be JSON");
        assert_eq!(articulation["code"], "track_not_found_or_rejected");
    }

    #[test]
    fn native_plugin_state_diagnostic_rejects_oversize_payload() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let diagnostic = core.set_plugin_state_diagnostic(1, 0, &vec![0u8; 4 * 1024 * 1024 + 1]);
        assert!(!diagnostic.ok);
        assert_eq!(diagnostic.code, "state_oversize");
    }

    #[test]
    fn plugin_state_read_diagnostic_distinguishes_missing_state() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.add_plugin(track_id, 0));
        let value: serde_json::Value = serde_json::from_str(
            &core.plugin_state_diagnostic_json(track_id, 99),
        )
        .expect("plugin state diagnostic must be JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "plugin_state_unavailable");
        let _ = core.remove_track(track_id);
    }

    #[test]
    fn sandbox_state_diagnostic_rejects_oversize_payload_without_touching_native() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let diagnostic =
            core.sandbox_plugin_state_diagnostic(1, 0, &vec![0u8; 4 * 1024 * 1024 + 1]);
        assert!(!diagnostic.ok);
        assert_eq!(diagnostic.code, 1);
        assert_eq!(diagnostic.message, "state-oversize");
    }

    #[test]
    fn plugin_state_mutation_corpus_never_panics_or_reports_success_without_storage() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.add_plugin(track_id, 0));

        let mut state = 0x1234_5678_u32;
        for length in 1..=256usize {
            let mut payload = vec![0u8; length];
            for byte in &mut payload {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let diagnostic = core.set_plugin_state_diagnostic(track_id, 0, &payload);
            if diagnostic.ok {
                // Native processors may normalize a state payload to their
                // fixed public representation; success must still leave a
                // readable, bounded state rather than claiming arbitrary
                // bytes were preserved verbatim.
                let stored = core.plugin_state(track_id, 0);
                assert!(!stored.is_empty() && stored.len() <= 4 * 1024 * 1024);
            }
        }
        let _ = core.remove_track(track_id);
    }

    #[test]
    fn mixing_advice_diagnostic_rejects_empty_title() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value =
            serde_json::from_str(&core.execute_mixing_advice_diagnostic_json("  ".to_owned()))
                .expect("diagnostic JSON");
        assert_eq!(value["code"], "invalid_mixing_advice_title");
    }

    #[test]
    fn render_validation_diagnostic_preserves_read_errors() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value =
            serde_json::from_str(&core.validate_render_output_diagnostic_json(
                "/tmp/aura-missing-validation-output.wav",
                false,
            ))
            .expect("diagnostic JSON");
        assert_eq!(value["code"], "render_output_unreadable");
        assert_eq!(value["retryable"], true);
    }

    #[test]
    fn native_wav_diagnostic_preserves_missing_file_reason() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value = serde_json::from_str(
            &core.read_wav_diagnostic_json("/tmp/aura-missing-native-wave64.w64", 1),
        )
        .expect("diagnostic JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["format"], "WAVE64");
        assert!(value["error"]
            .as_str()
            .is_some_and(|error| error.contains("not found")));
    }

    #[test]
    fn undo_and_redo_diagnostics_report_empty_history() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let undo: serde_json::Value =
            serde_json::from_str(&core.undo_diagnostic_json()).expect("undo diagnostic JSON");
        let redo: serde_json::Value =
            serde_json::from_str(&core.redo_diagnostic_json()).expect("redo diagnostic JSON");
        assert_eq!(undo["code"], "undo_history_empty");
        assert_eq!(redo["code"], "redo_history_empty");
    }

    #[test]
    fn low_latency_mode_is_exposed_through_the_core_boundary() {
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(!core.low_latency_mode());
        assert!(core.set_low_latency_mode(true));
        assert!(core.low_latency_mode());
        assert!(core.set_low_latency_mode(false));
        assert!(!core.low_latency_mode());
    }

    #[test]
    fn tonal_and_codepad_apis_work_through_core_boundary() {
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(core.set_tonal_scale(0, 0));
        assert!(core.is_note_in_tonal_scale(60));
        assert!(!core.is_note_in_tonal_scale(61));
        assert_eq!(core.generate_chord_notes(0, 4, 0), vec![48, 52, 55]);
        assert!(core.generate_chord_notes(0, 4, 99).is_empty());
    }

    #[test]
    fn transport_diagnostics_report_applied_state() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let playhead: serde_json::Value =
            serde_json::from_str(&core.set_playhead_diagnostic_json(1234))
                .expect("playhead diagnostic JSON");
        assert_eq!(playhead["ok"], true);
        assert_eq!(playhead["playhead"], 1234);
        let playing: serde_json::Value =
            serde_json::from_str(&core.set_playing_diagnostic_json(false))
                .expect("playing diagnostic JSON");
        assert_eq!(playing["ok"], true);
        assert_eq!(playing["playing"], false);
    }

    #[test]
    fn transport_and_midi_void_compatibility_diagnostics_are_structured() {
        let core = crate::AuraCore::new().expect("core must initialize");
        for value in [
            core.set_loop_diagnostic_json(true),
            core.set_test_tone_diagnostic_json(false),
            core.clear_midi_notes_diagnostic_json(),
        ] {
            let json: serde_json::Value = serde_json::from_str(&value).expect("diagnostic JSON");
            assert_eq!(json["ok"], true);
        }
        let invalid: serde_json::Value =
            serde_json::from_str(&core.set_midi_note_diagnostic_json(0, 128, 128, 0, 0))
                .expect("MIDI note diagnostic JSON");
        assert_eq!(invalid["code"], "invalid_midi_note");
        let preset: serde_json::Value =
            serde_json::from_str(&core.load_plugin_preset_diagnostic_json(0, 0, ""))
                .expect("preset diagnostic JSON");
        assert_eq!(preset["code"], "invalid_plugin_preset_target");
    }

    #[test]
    fn project_v2_hydration_diagnostic_is_structured_for_invalid_input() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value =
            serde_json::from_str(&core.load_project_v2_diagnostic_json(""))
                .expect("project hydration diagnostic JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "project_hydration_failed");
        assert!(value["error"]["generation"].as_u64().is_some());
    }

    #[test]
    fn external_sync_ui_boundary_updates_real_transport_state() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let configured: serde_json::Value =
            serde_json::from_str(&core.configure_external_sync_json("mtc", "midi:1", true))
                .expect("sync configuration JSON");
        assert_eq!(configured["ok"], true);
        assert_eq!(configured["enabled"], true);

        let disabled_mmc: serde_json::Value = serde_json::from_str(
            &crate::AuraCore::new()
                .expect("second core")
                .external_sync_mmc_json(&[0xf0, 0x7f, 0x00, 0x06, 0x02, 0xf7]),
        )
        .expect("MMC diagnostic JSON");
        assert_eq!(disabled_mmc["ok"], false);

        let mtc: serde_json::Value =
            serde_json::from_str(&core.external_sync_mtc_json(1, 2, 3, 4, 25))
                .expect("MTC diagnostic JSON");
        assert_eq!(mtc["ok"], true);
        assert_eq!(mtc["timecode"]["fps"], 25);
        let status: serde_json::Value =
            serde_json::from_str(&core.external_sync_status_json()).expect("sync status JSON");
        assert_eq!(status["running"], true);
    }
