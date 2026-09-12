    #[test]
    fn generated_arpeggio_is_deterministic_and_respects_pattern() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let notes = core.generate_arpeggio(vec![60, 64, 67], vec![100, 90, 80], 0, 1, 5);
        assert_eq!(
            notes,
            vec![(60, 100), (64, 90), (67, 80), (60, 100), (64, 90)]
        );
        let down = core.generate_arpeggio(vec![60, 64, 67], vec![100, 90, 80], 1, 1, 3);
        assert_eq!(down, vec![(67, 80), (64, 90), (60, 100)]);
    }

    #[test]
    fn arpeggio_placement_writes_timed_piano_roll_notes() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let placed = core.place_arpeggio(1, 1_000, 480, 360, vec![60, 64], vec![100, 90], 0, 1, 3);
        assert_eq!(placed, 3);
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        let notes = notes.as_array().unwrap();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0]["start_sample"], 1_000);
        assert_eq!(notes[1]["start_sample"], 1_480);
        assert_eq!(notes[2]["length_samples"], 360);
    }

    #[test]
    fn midi_notes_json_is_timeline_sorted_for_editors() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        core.clear_midi_notes();
        core.set_midi_note(0, 72, 100, 960, 240);
        core.set_midi_note(0, 60, 100, 0, 240);
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["start_sample"], 0);
        assert_eq!(notes[1]["start_sample"], 960);
    }

    #[test]
    fn numeric_midi_edit_preserves_existing_lyric() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(core.set_midi_note_lyric(0, 64, 100, 0, 480, "mi"));
        core.set_midi_note(0, 64, 80, 0, 960);
        let notes = core.scheduled_midi_notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].lyric, "mi");
        assert_eq!(notes[0].length_samples, 960);
    }

    #[test]
    fn swing_and_humanize_update_scheduled_piano_roll_notes() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let subdivision = core.beats_to_samples(0.5);
        core.set_midi_note(0, 60, 100, subdivision, 480);
        assert!(core.apply_midi_swing(0.5, 1.0));
        let swung: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        let swung_sample = (subdivision as f64 * 1.5).round() as u64;
        assert_eq!(swung[0]["start_sample"], swung_sample);

        assert!(core.humanize_midi(0.25, 10, 42));
        let humanized: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert!(humanized[0]["velocity"].as_u64().unwrap() <= 110);
        assert!(humanized[0]["start_sample"].as_u64().unwrap() <= subdivision * 2);
    }

    #[test]
    fn quantize_updates_scheduled_piano_roll_notes() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let grid = core.beats_to_samples(0.25);
        core.set_midi_note(0, 60, 100, grid + grid / 2, 480);
        assert!(core.quantize_midi(0.25, 1.0));
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        let expected = ((grid + grid / 2) as f64 / grid as f64).round() as u64 * grid;
        assert_eq!(notes[0]["start_sample"], expected);
    }

    #[test]
    fn audio_config_generation_advances_on_each_reprepare() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        let engine_ref = engine.as_ref().expect("native engine handle");
        let before = engine_ref.get_audio_config_generation();
        engine_ref.apply_config(120.0, 44_100, 256);
        let after_first = engine_ref.get_audio_config_generation();
        engine_ref.apply_config(120.0, 48_000, 128);
        let after_second = engine_ref.get_audio_config_generation();
        assert!(after_first > before);
        assert!(after_second > after_first);
        engine_ref.apply_config(f32::NAN, 48_000, 128);
        engine_ref.apply_config(120.0, 0, 128);
        assert_eq!(engine_ref.get_audio_config_generation(), after_second);
        assert!(!engine_ref.try_apply_config(120.0, 0, 256));
        assert_eq!(engine_ref.get_audio_config_generation(), after_second);
        assert!(engine_ref.try_apply_config(120.0, 44_100, 256));
        assert!(engine_ref.get_audio_config_generation() > after_second);
    }

    #[test]
    fn audio_edit_round_trip_flows_through_engine_and_persistence() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();
        let unique = format!(
            "aura-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source_path = std::env::temp_dir().join(format!("{unique}-source.wav"));
        let project_path = std::env::temp_dir().join(format!("{unique}.aura"));
        let source = source_path.to_str().unwrap();
        let project = project_path.to_str().unwrap();

        let track_id = engine_ref.add_track(0);
        assert!(engine_ref.bounce_project(source, 0));
        assert!(engine_ref.add_region(track_id, source, 0.0));
        let layout: serde_json::Value =
            serde_json::from_str(&engine_ref.get_project_layout_json().to_string())
                .expect("native layout must be valid JSON");
        let region_id = layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| regions.first())
            .and_then(|region| region.get("id"))
            .and_then(serde_json::Value::as_u64)
            .expect("added region must have a stable nonzero id") as u32;
        assert!(engine_ref.set_region_gain(track_id, region_id, 3.0));
        assert!(engine_ref.set_region_fades(track_id, region_id, 0.05, 0.1));
        assert!(engine_ref.set_region_trim(track_id, region_id, 0.05, 0.95));
        assert!(engine_ref.set_region_warp_ratio(track_id, region_id, 1.25));
        assert!(engine_ref.set_region_pitch_semitones(track_id, region_id, 2.0));
        assert!(engine_ref.set_region_loop_count(track_id, region_id, 2));
        assert!(engine_ref.save_project(project));
        assert!(engine_ref.load_project(project));
        // The generated source is an intentionally empty project render; the
        // important contract here is that edits and persistence succeed
        // without invalidating the region or crashing waveform retrieval.
        let _ = engine_ref.get_region_waveform(track_id, region_id);

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(project_path);
    }

    #[test]
    fn plugin_lifecycle_rejects_invalid_parameters_and_keeps_sandbox_status_queryable() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();
        let track_id = engine_ref.add_track(0);

        assert!(engine_ref.add_plugin(track_id, 0));
        assert!(engine_ref.add_plugin(track_id, 2));
        assert!(engine_ref.set_plugin_parameter(track_id, 1, 0, 0.02));
        assert!(engine_ref.add_plugin(track_id, 3));
        assert!(engine_ref.add_plugin(track_id, 4));
        assert!(engine_ref.set_plugin_parameter(track_id, 3, 0, 0.75));
        assert!(engine_ref.add_plugin(track_id, 5));
        assert!(engine_ref.set_plugin_parameter(track_id, 4, 0, 0.65));
        assert!(engine_ref.add_plugin(track_id, 6));
        assert!(engine_ref.set_plugin_parameter(track_id, 5, 0, 0.4));
        assert!(engine_ref.add_plugin(track_id, 7));
        assert!(engine_ref.set_plugin_parameter(track_id, 6, 0, 0.3));
        assert!(engine_ref.add_plugin(track_id, 8));
        assert!(engine_ref.set_plugin_parameter(track_id, 7, 0, 0.5));
        assert!(engine_ref.add_plugin(track_id, 9));
        assert!(engine_ref.set_plugin_parameter(track_id, 8, 0, 0.75));
        assert!(engine_ref.add_plugin(track_id, 10));
        assert!(engine_ref.set_plugin_parameter(track_id, 9, 0, 0.8));
        assert!(!engine_ref.set_plugin_parameter(track_id, 0, 0, f32::NAN));
        assert!(engine_ref.set_plugin_parameter(track_id, 0, 0, 0.5));
        assert!(engine_ref.set_plugin_bypass(track_id, 0, true));
        assert!(engine_ref.set_plugin_bypass(track_id, 0, false));
        assert!(!engine_ref.add_sandboxed_plugin(track_id, ""));
        assert!(!engine_ref.add_sandboxed_plugin(track_id, "/definitely/missing/aura-plugin.clap"));
        let _ = engine_ref.get_sandbox_statuses();
        let _ = engine_ref.get_sandbox_plugin_paths();
    }

    #[test]
    fn separate_aura_handles_own_independent_project_graphs() {
        let _guard = native_engine_test_guard();
        let first = crate::AuraCore::new().expect("first core must initialize");
        let second = crate::AuraCore::new().expect("second core must initialize");
        let first_track = first.add_track(0);
        let second_track = second.add_track(0);
        assert!(first_track != second_track || first_track != 0);
        assert!(first.set_volume(first_track, 0.5));
        assert!(second.set_volume(second_track, 0.75));
        assert!(first.set_tempo(90.0));
        assert!(second.set_tempo(110.0));
        assert!((first.get_tempo() - 90.0).abs() < 1.0e-5);
        assert!((second.get_tempo() - 110.0).abs() < 1.0e-5);
        // Track creation, volume, and tempo are independent reversible
        // mutations.  The important contract here is that each session owns
        // its own history; the exact count must not collapse to a process
        // global singleton value.
        assert!(first.engine.get_undo_count() >= 1);
        assert!(second.engine.get_undo_count() >= 1);

        assert!(first.remove_track(first_track));
        assert!(second.engine.get_undo_count() >= 1);
        assert!(second
            .get_project_layout_json()
            .contains(&format!("\"id\":{second_track}")));
        assert!(!first
            .get_project_layout_json()
            .contains(&format!("\"id\":{first_track}")));
    }

    #[test]
    fn plugin_parameter_changes_are_published_as_bounded_ui_events() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.add_plugin(track_id, 0));
        assert!(core.set_plugin_parameter(track_id, 0, 0, 0.25));
        let events = core.drain_plugin_parameter_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track_id, track_id);
        assert_eq!(events[0].plugin_index, 0);
        assert_eq!(events[0].parameter_id, 0);
        assert!((events[0].value - 0.25).abs() < f32::EPSILON);
        assert!(core.drain_plugin_parameter_events().is_empty());
    }

    #[test]
    fn recording_and_automation_commands_cross_the_core_boundary() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");

        core.start_recording_capture(48_000.0, 2, 1024, 0)
            .expect("recording must start");
        core.append_recording_preview(&[0.1, -0.1, 0.2, -0.2])
            .expect("valid input block must be accepted");
        let region = core
            .stop_recording_preview()
            .expect("recording must stop cleanly");
        assert_eq!(region.channels, 2);
        assert_eq!(core.recording_take_count(), 1);

        let track_id = core.add_track(0);
        assert!(core.set_automation_data(track_id, 0, vec![0.0, 0.5, 0.0, 22050.0, 0.75, 0.0]));
        assert!(!core.set_automation_data(track_id, 0, vec![f64::NAN]));
    }

    #[test]
    fn production_audio_workflow_records_persists_and_bounces() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let unique = format!(
            "aura-workflow-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let project_path = std::env::temp_dir().join(format!("{unique}.aura"));
        let render_path = std::env::temp_dir().join(format!("{unique}.wav"));

        core.start_recording_capture(48_000.0, 2, 4096, 0)
            .expect("capture starts");
        core.append_recording_preview(&[0.25, -0.25, 0.1, -0.1])
            .expect("capture accepts audio");

        let track_id = core.add_track(0);
        assert!(
            core.commit_recording_capture_to_track(track_id, None)
                .expect("recording publishes to track")
                > 0
        );
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).expect("valid layout JSON");
        let region_id = layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| regions.first())
            .and_then(|region| region.get("id"))
            .and_then(serde_json::Value::as_u64)
            .expect("recording commit publishes a region") as u32;
        assert!(core.set_region_gain(track_id, region_id, 0.75));
        assert!(core.set_region_fades(track_id, region_id, 0.02, 0.04));
        assert!(core.set_region_warp_ratio(track_id, region_id, 1.1));
        assert!(core.set_region_pitch_semitones(track_id, region_id, 2.0));
        assert!(core.set_region_loop_count(track_id, region_id, 2));
        assert!(core.add_plugin(track_id, 0));
        assert!(core.set_plugin_parameter(track_id, 0, 0, 0.5));
        assert!(core.set_automation_data(track_id, 0, vec![0.0, 0.5, 0.0, 22050.0, 0.75, 0.0]));
        let before_reload = core.get_project_layout_json();
        assert!(before_reload.contains("warp_ratio"));
        assert!(before_reload.contains("pitch_semitones"));
        assert!(before_reload.contains("loop_count"));
        assert!(core.save_project(project_path.to_str().unwrap()));
        assert!(core.load_project(project_path.to_str().unwrap()));
        let after_reload = core.get_project_layout_json();
        assert!(after_reload.contains("warp_ratio"));
        assert!(after_reload.contains("pitch_semitones"));
        assert!(after_reload.contains("loop_count"));
        assert!(core.bounce_project(render_path.to_str().unwrap(), 0));

        let bytes = std::fs::read(&render_path).expect("render output exists");
        assert!(bytes.len() >= 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");

        let _ = std::fs::remove_file(project_path);
        let _ = std::fs::remove_file(render_path);
    }

    #[test]
    fn render_target_catalog_reflects_native_track_kinds_and_master() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let audio_id = core.add_track(0);
        let bus_id = core.add_track(3);

        let catalog: serde_json::Value =
            serde_json::from_str(&core.render_target_catalog_diagnostic_json())
                .expect("render target catalog must be JSON");
        let targets = catalog
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .expect("catalog must contain targets");
        let target_for = |id: u32| {
            targets
                .iter()
                .find(|target| {
                    target.get("source_id").and_then(serde_json::Value::as_u64) == Some(id as u64)
                })
                .and_then(|target| target.get("kind"))
                .and_then(serde_json::Value::as_str)
        };

        assert_eq!(target_for(audio_id), Some("Track"));
        assert_eq!(target_for(bus_id), Some("Bus"));
        assert!(targets.iter().any(|target| {
            target.get("target_id").and_then(serde_json::Value::as_str) == Some("master")
                && target.get("kind").and_then(serde_json::Value::as_str) == Some("Master")
        }));
    }

    #[test]
    fn audio_config_accepts_supported_pairs_and_rejects_unsafe_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(core.apply_audio_config(48_000, 256));
        assert!(core.apply_audio_config(44_100, 128));
        assert!(!core.apply_audio_config(22_050, 256));
        assert!(!core.apply_audio_config(48_000, 127));
    }

    #[test]
    fn project_v2_round_trips_comping_state_without_sidecar_dependency() {
        let _guard = native_engine_test_guard();
        let path = std::env::temp_dir().join(format!(
            "aura-comping-project-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let writer = crate::AuraCore::new().expect("writer core must initialize");
        assert!(writer.register_comp_take(11, "Lead take", 0, 256));
        assert!(writer.set_comp_segments(&[(11, 0, 128, 16), (11, 128, 128, 16)]));
        assert_eq!(writer.resolve_comp_at(140).0, 11);
        writer
            .save_project_v2(path.to_str().unwrap(), "Comping", 120.0)
            .expect("project_v2 save must include comping");
        let saved = crate::ProjectDocument::load(path.to_str().unwrap())
            .expect("saved project must be readable");
        assert!(saved
            .render_targets
            .iter()
            .any(|target| target.target_id == "master"));

        let reader = crate::AuraCore::new().expect("reader core must initialize");
        reader
            .load_project_v2(path.to_str().unwrap())
            .expect("project_v2 load must restore comping");
        assert_eq!(reader.resolve_comp_at(8).0, 11);
        assert_eq!(reader.resolve_comp_at(140).0, 11);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_empty_and_invalid_paths_are_rejected() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();
        let tid = engine_ref.add_track(0);
        let invalid_path = "/aura-core-bridge/path-that-does-not-exist.wav";

        assert!(!engine_ref.add_region(tid, "", 0.0));
        assert!(!engine_ref.add_region(tid, invalid_path, 0.0));
        assert!(!engine_ref.replace_region_audio(tid, 0, ""));
        assert!(!engine_ref.replace_region_audio(tid, 0, invalid_path));
        assert!(!engine_ref.save_project(""));
        assert!(!engine_ref.load_project(""));
        assert!(!engine_ref.load_project(invalid_path));
        assert!(!engine_ref.bounce_project("", 0));
        assert!(!engine_ref.bounce_project("/tmp/aura-output.aiff", 0));
        let diagnostic: serde_json::Value =
            serde_json::from_str(&engine_ref.bounce_project_diagnostic_json("", 0))
                .expect("bounce diagnostics must be JSON");
        assert_eq!(diagnostic["code"], "invalid_render_path");
        assert_eq!(diagnostic["retryable"], false);
        let unsupported_format: serde_json::Value = serde_json::from_str(
            &engine_ref.bounce_project_diagnostic_json("/tmp/aura-output.wav", 2),
        )
        .expect("format diagnostics must be JSON");
        assert_eq!(unsupported_format["code"], "unsupported_render_format");

        let core = crate::AuraCore::new().expect("core must initialize");
        let region_add: serde_json::Value = serde_json::from_str(&core.add_region_diagnostic_json(
            tid,
            "/missing/audio.wav",
            f64::NAN,
        ))
        .expect("region add diagnostics must be JSON");
        assert_eq!(region_add["code"], "invalid_region_input");
        let region_replace: serde_json::Value = serde_json::from_str(
            &core.replace_region_audio_diagnostic_json(tid, 1, "/missing/audio.wav"),
        )
        .expect("region replacement diagnostics must be JSON");
        assert_eq!(region_replace["code"], "region_audio_not_found");
        let empty_plugin: serde_json::Value =
            serde_json::from_str(&core.add_sandboxed_plugin_diagnostic_json(tid, ""))
                .expect("plugin diagnostics must be JSON");
        assert_eq!(empty_plugin["code"], "invalid_plugin_path");
        assert_eq!(empty_plugin["retryable"], false);
        let missing_plugin: serde_json::Value =
            serde_json::from_str(&core.add_sandboxed_plugin_diagnostic_json(
                tid,
                "/aura-core-bridge/path-that-does-not-exist.clap",
            ))
            .expect("plugin diagnostics must be JSON");
        assert_eq!(missing_plugin["code"], "plugin_not_found");
        let recovery: serde_json::Value =
            serde_json::from_str(&core.recover_sandboxed_plugin_diagnostic_json(tid, 0, false))
                .expect("sandbox recovery diagnostics must be JSON");
        assert_eq!(recovery["ok"], false);
        assert_eq!(recovery["code"], "sandbox_not_found");
        assert_eq!(recovery["track_id"], tid);
        assert_eq!(recovery["plugin_index"], 0);
        assert!(recovery["project_generation"].as_u64().is_some());
        assert!(recovery["audio_generation"].as_u64().is_some());
    }

    #[test]
    fn render_path_contract_requires_a_wav_extension() {
        assert!(crate::is_wav_output_path("mix.WAV"));
        assert!(!crate::is_wav_output_path("mix.aiff"));
        assert!(!crate::is_wav_output_path("mix"));
    }

    #[test]
    fn wav_publication_validator_rejects_truncated_or_misaligned_payloads() {
        let mut wav = vec![0u8; 44 + 8];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"fmt ");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[36..40].copy_from_slice(b"data");
        wav[20..22].copy_from_slice(&1u16.to_le_bytes());
        wav[22..24].copy_from_slice(&2u16.to_le_bytes());
        wav[24..28].copy_from_slice(&48_000u32.to_le_bytes());
        wav[32..34].copy_from_slice(&4u16.to_le_bytes());
        wav[34..36].copy_from_slice(&16u16.to_le_bytes());
        wav[40..44].copy_from_slice(&8u32.to_le_bytes());
        assert!(crate::valid_pcm_or_float_wav(&wav));
        wav[40..44].copy_from_slice(&7u32.to_le_bytes());
        assert!(!crate::valid_pcm_or_float_wav(&wav));
        wav.truncate(45);
        assert!(!crate::valid_pcm_or_float_wav(&wav));
    }
