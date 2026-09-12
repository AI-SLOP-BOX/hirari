    #[test]
    fn sandbox_failure_codes_have_stable_retry_policy() {
        assert!(crate::SandboxFailureKind::from_code(12).retryable());
        assert!(crate::SandboxFailureKind::from_code(13).retryable());
        assert!(!crate::SandboxFailureKind::from_code(10).retryable());
        assert!(matches!(
            crate::SandboxFailureKind::from_code(250),
            crate::SandboxFailureKind::Unknown(250)
        ));
    }

    #[test]
    fn sandbox_status_decoder_ignores_partial_records_and_bounds_failure_code() {
        let decoded = crate::decode_sandbox_statuses(&[7, 3, 1, 0, 300, 4]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].track_id, 7);
        assert_eq!(decoded[0].plugin_index, 3);
        assert!(decoded[0].alive);
        assert!(!decoded[0].can_retry);
        assert_eq!(decoded[0].failure, u8::MAX);
        assert_eq!(decoded[0].dropped_output_midi, 4);
        assert_eq!(decoded[0].mailbox_overruns, 0);
        assert_eq!(decoded[0].input_midi_truncations, 0);
    }

    #[test]
    fn sandbox_status_decoder_exposes_mailbox_overruns() {
        let decoded = crate::decode_sandbox_statuses(&[11, 2, 1, 1, 0, 3, 9]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].dropped_output_midi, 3);
        assert_eq!(decoded[0].mailbox_overruns, 9);
    }

    #[test]
    fn sandbox_status_decoder_exposes_midi_truncations() {
        let decoded = crate::decode_sandbox_statuses(&[11, 2, 1, 1, 0, 3, 9, 4]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].mailbox_overruns, 9);
        assert_eq!(decoded[0].input_midi_truncations, 4);
        assert_eq!(decoded[0].recovery_mode, 0);
    }

    #[test]
    fn sandbox_status_decoder_exposes_recovery_mode() {
        let decoded = crate::decode_sandbox_statuses(&[0x4155_5209, 11, 2, 0, 0, 10, 0, 12, 3, 1]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].mailbox_overruns, 12);
        assert_eq!(decoded[0].input_midi_truncations, 3);
        assert_eq!(decoded[0].recovery_mode, 1);
    }

    #[test]
    fn sandbox_status_header_prevents_multi_plugin_width_ambiguity() {
        let mut raw = vec![0x4155_5209];
        for plugin in 0..9u32 {
            raw.extend([7, plugin, 1, 0, 0, 0, 0, 0, 1]);
        }
        let decoded = crate::decode_sandbox_statuses(&raw);
        assert_eq!(decoded.len(), 9);
        assert_eq!(decoded[8].plugin_index, 8);
        assert!(decoded.iter().all(|status| status.is_quarantined()));
    }

    #[test]
    fn sandbox_snapshot_exposes_typed_quarantine_state() {
        let mut snapshot = crate::SandboxSnapshot {
            track_id: 1,
            plugin_index: 2,
            display_name: "fixture".to_owned(),
            alive: false,
            can_retry: false,
            failure: 10,
            dropped_output_midi: 0,
            mailbox_overruns: 8,
            recovery_mode: crate::SandboxSnapshot::QUARANTINED,
        };
        assert!(snapshot.is_quarantined());
        snapshot.recovery_mode = crate::SandboxSnapshot::CLEAR_BLOCK;
        assert!(!snapshot.is_quarantined());
    }

    #[test]
    fn bounce_progress_normalization_separates_indeterminate_from_invalid_state() {
        assert_eq!(crate::normalize_bounce_progress(0.25), (0.25, true));
        assert_eq!(crate::normalize_bounce_progress(-1.0), (0.0, true));
        assert_eq!(crate::normalize_bounce_progress(2.0), (1.0, true));
        let (progress, available) = crate::normalize_bounce_progress(f32::NAN);
        assert_eq!(progress, 0.0);
        assert!(!available);
    }

    #[test]
    fn plugin_display_name_hides_parent_directories_and_extensions() {
        assert_eq!(
            crate::safe_plugin_display_name("/private/user/Secret.vst3"),
            "Secret"
        );
        assert_eq!(
            crate::safe_plugin_display_name("C:\\Users\\Secret\\Echo.clap"),
            "Echo"
        );
        assert_eq!(
            crate::safe_plugin_display_name("builtin://passthrough"),
            "passthrough"
        );
    }

    // The native bridge intentionally owns a process-wide engine singleton.
    // Serialize tests that mutate its transport tempo so Rust's parallel test
    // runner cannot make unrelated assertions observe another test's value.
    fn tempo_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn native_engine_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Unit tests exercise the native graph and FFI contract, not the
        // machine's physical CoreAudio device. Keep them on the same
        // offline-isolation path as integration tests so a device format
        // change cannot make deterministic graph tests flaky.
        std::env::set_var("AURA_NATIVE_TEST_ISOLATION", "1");
        guard
    }

    #[test]
    fn test_brickwall_limiter() {
        let mut limiter = TruePeakLimiterEngine::new(44100.0);
        let mut l = vec![1.5; 512];
        let mut r = vec![1.5; 512];
        limiter.process(&mut l, &mut r, -1.0, -0.1);
        for i in 0..512 {
            assert!(l[i] <= 1.0, "Limiter ceiling violated: {}", l[i]);
            assert!(r[i] <= 1.0, "Limiter ceiling violated: {}", r[i]);
        }
        assert!(limiter.audit_true_peak_limiter());
    }

    #[test]
    fn test_tape_machine() {
        let mut tape = TapeMachineEngine::new(44100.0);
        let mut l = vec![0.5; 512];
        let mut r = vec![0.5; 512];
        tape.set_drive(3.0);
        tape.process(&mut l, &mut r);
        assert!(tape.audit_tape_machine());
    }

    #[test]
    fn test_virtuoso_space_reverb() {
        let mut space = VirtuosoSpaceEngine::new(44100.0);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        l[0] = 1.0;
        r[0] = 1.0; // Impulse
        space.process(&mut l, &mut r);
        assert!(space.audit_virtuoso_space());
    }

    #[test]
    fn test_passive_curing_eq() {
        let mut eq = PassiveCuringEqEngine::new(44100.0);
        let mut l = vec![0.5; 512];
        let mut r = vec![0.5; 512];
        eq.set_parameters(2.0, 1.0, 3.0, 0.5);
        eq.process(&mut l, &mut r);
        assert!(eq.audit_passive_curing_eq());
    }

    #[test]
    fn test_aura_wavetable_synth() {
        let mut synth = AuraWavetableSynthEngine::new(44100.0);
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        synth.note_on(440.0, 0.8);
        synth.process(&mut l, &mut r);
        assert!(synth.audit_aura_wavetable_synth());
    }

    #[test]
    fn test_loudness_analyzer() {
        let mut analyzer = LoudnessAnalyzerEngine::new(44100.0);
        let l = vec![0.707; 1024];
        let r = vec![0.707; 1024];
        let metrics = analyzer.process(&l, &r);
        assert!(metrics.true_peak_db > -100.0);
        assert!(analyzer.audit_loudness_analyzer());
    }

    #[test]
    fn test_virtuoso_vocal_engine() {
        use crate::virtuoso_vocal::VirtuosoVocalEngine;
        let mut engine = VirtuosoVocalEngine::new(44100.0);
        let mut l = vec![0.1; 512];
        let mut r = vec![0.1; 512];
        engine.pitch_shift_semi = 2.0;
        engine.formant_shift = -1.0;
        engine.process(&mut l, &mut r);
        assert!(engine.audit_virtuoso_vocal());
    }

    #[test]
    fn test_tempo_orchestrator() {
        use crate::tempo::{TempoEvent, TempoOrchestrator};
        let mut orch = TempoOrchestrator::new();
        orch.events = vec![
            TempoEvent {
                sample_pos: 0,
                bpm: 120.0,
                ramp: true,
                world_beats: 0.0,
            },
            TempoEvent {
                sample_pos: 44100,
                bpm: 180.0,
                ramp: false,
                world_beats: 0.0,
            },
        ];
        orch.recalculate_integrated_time(44100.0);
        assert!(orch.events[1].world_beats > 0.0);

        let mid_samples = 22050;
        let beats = orch.samples_to_beats(mid_samples, 44100.0);
        let samples = orch.beats_to_samples(beats, 44100.0);
        assert!((samples as i64 - mid_samples as i64).abs() < 5); // Allow small rounding deviation
        assert!(orch.audit_tempo());
    }

    #[test]
    fn test_wav_decoder_integration() {
        let _guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let engine_ref = engine.as_ref().unwrap();

        // 1. Add track
        let tid = engine_ref.add_track(0);

        // 2. Write temp WAV file by bouncing
        let temp_dir = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after UNIX_EPOCH")
            .as_nanos();
        let file_path = temp_dir.join(format!(
            "aura_test_import_{}_{}.wav",
            std::process::id(),
            nonce
        ));
        let path_str = file_path.to_str().unwrap();

        // Make sure it bounces successfully
        engine_ref.bounce_project(path_str, 0);
        assert!(file_path.exists());

        // 3. Import back by adding a region
        engine_ref.add_region(tid, path_str, 0.0);

        // Clean up
        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn bounce_reports_success_only_after_publishing_a_valid_wav() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let path = std::env::temp_dir().join(format!(
            "aura-bounce-validation-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_str = path.to_str().unwrap();

        assert!(core.bounce_project(path_str, 0));
        let bytes = std::fs::read(&path).expect("published WAV");
        assert!(bytes.len() >= 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn openutau_source_render_pair_survives_canonical_project_save() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let root = std::env::temp_dir().join(format!(
            "aura-openutau-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("vocal.ustx");
        let render = root.join("vocal.wav");
        let project = root.join("song.aura");
        std::fs::write(&source, b"project: openutau\n").unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&38u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&88_200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0]);
        std::fs::write(&render, wav).unwrap();

        core.register_openutau_vocal(source.to_str().unwrap(), render.to_str().unwrap())
            .unwrap();
        core.save_project_v2(project.to_str().unwrap(), "OpenUtau Song", 120.0)
            .unwrap();
        let document = crate::ProjectDocument::load(project.to_str().unwrap()).unwrap();
        assert_eq!(document.openutau_vocals.len(), 1);
        assert_eq!(
            document.openutau_vocals[0].source_path,
            source.to_str().unwrap()
        );
        assert_eq!(
            document.openutau_vocals[0].rendered_audio_path,
            render.to_str().unwrap()
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn midi_only_bounce_uses_scheduled_note_end_instead_of_silent_fallback_tail() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let path = std::env::temp_dir().join(format!(
            "aura-midi-only-bounce-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // There are no audio regions here. The scheduled MIDI note itself is
        // the only source of the render extent and ends at eight seconds.
        core.clear_midi_notes();
        core.set_midi_note(0, 69, 100, 0, 44_100 * 8);
        assert!(core.bounce_project(path.to_str().unwrap(), 0));

        let bytes = std::fs::read(&path).expect("MIDI-only render output exists");
        assert!(bytes.len() >= 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as usize;
        let bits = u16::from_le_bytes([bytes[34], bytes[35]]) as usize;
        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
        let bytes_per_frame = channels * (bits / 8);
        assert!(bytes_per_frame > 0);
        let frames = data_size / bytes_per_frame;
        assert!(frames >= 44_100 * 8);
        assert!(
            frames < 44_100 * 10,
            "MIDI-only bounce retained the 30s fallback tail"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn midi_note_edit_history_roundtrips_native_and_project_mirrors() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        core.clear_midi_notes();
        core.set_midi_note(0, 60, 100, 0, 480);
        assert_eq!(core.midi_notes_snapshot().len(), 5);
        assert_eq!(core.scheduled_midi_notes.lock().unwrap().len(), 1);
        core.set_midi_note(0, 60, 80, 0, 960);
        assert_eq!(core.midi_notes_snapshot().len(), 5);
        assert_eq!(core.scheduled_midi_notes.lock().unwrap().len(), 1);
        assert!(core.set_midi_note_lyric(0, 60, 90, 0, 1_440, "la"));
        let notes = core.scheduled_midi_notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].length_samples, 1_440);
        assert_eq!(notes[0].lyric, "la");
        drop(notes);
        let notes_json: serde_json::Value = serde_json::from_str(&core.midi_notes_json())
            .expect("MIDI authoring JSON must be valid");
        assert_eq!(notes_json[0]["pitch"], 60);
        assert_eq!(notes_json[0]["lyric"], "la");
        assert_eq!(notes_json[0]["drum_lane"], "Hi Bongo");

        assert!(core.undo_depth() > 0);
        core.undo();
        core.undo();
        assert_eq!(core.midi_notes_snapshot().len(), 5);
        assert_eq!(core.scheduled_midi_notes.lock().unwrap().len(), 1);
        core.undo();
        assert!(core.midi_notes_snapshot().is_empty());
        assert!(core.scheduled_midi_notes.lock().unwrap().is_empty());

        core.redo();
        core.redo();
        core.redo();
        assert_eq!(core.midi_notes_snapshot().len(), 5);
        let restored = core.scheduled_midi_notes.lock().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].lyric, "la");
    }

    #[test]
    fn midi_vibrato_rate_edit_requires_note_and_is_visible_to_clients() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        core.clear_midi_notes();
        core.set_midi_note(2, 64, 100, 960, 480);
        assert!(!core.set_midi_note_vibrato_rate_without_undo(2, 64, 1_920, 7_500));
        assert!(core.set_midi_note_vibrato_rate_without_undo(2, 64, 960, 7_500));
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json())
            .expect("MIDI authoring JSON must be valid");
        assert_eq!(notes[0]["vibrato_rate_millihz"], 7_500);
    }

    #[test]
    fn measured_hrtf_provider_payload_is_validated_and_installed() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track = core.add_track(0);
        assert_ne!(track, 0);
        let payload = r#"{"left":[1.0,0.25,0.0],"right":[0.5,0.0,-0.1]}"#;
        let result: serde_json::Value = serde_json::from_str(&core.set_hrtf_kernel_json(track, payload))
            .expect("HRTF response must be JSON");
        assert_eq!(result["ok"], true);
        assert_eq!(result["taps"], 3);
        assert!(core.clear_hrtf_kernel(track));
        assert_eq!(core.set_hrtf_kernel_json(track, "{\"left\":[NaN]}")[..].contains("invalid_hrtf"), true);
    }

    #[test]
    fn chord_track_roundtrips_through_json_and_undo_redo() {
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(core
            .add_chord_event(960, 60, vec![0, 4, 7], "C")
            .then_some(())
            .is_some());
        assert!(core.add_chord_event(0, 62, vec![0, 3, 7], "Dm"));
        let before_undo: serde_json::Value =
            serde_json::from_str(&core.chord_track_json()).unwrap();
        assert_eq!(before_undo.as_array().unwrap().len(), 2);
        assert_eq!(before_undo[0]["name"], "Dm");
        core.undo();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&core.chord_track_json())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        core.redo();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&core.chord_track_json())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
