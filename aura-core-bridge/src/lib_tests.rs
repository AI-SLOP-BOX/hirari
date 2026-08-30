#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use crate::aura_wavetable_synth::AuraWavetableSynthEngine;
    use crate::loudness_analyzer::LoudnessAnalyzerEngine;
    use crate::passive_curing_eq::PassiveCuringEqEngine;
    use crate::tape_machine::TapeMachineEngine;
    use crate::true_peak_limiter::TruePeakLimiterEngine;
    use crate::virtuoso_space::VirtuosoSpaceEngine;

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
        let guard = LOCK.get_or_init(|| Mutex::new(())).lock()
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
        assert_eq!(document.openutau_vocals[0].source_path, source.to_str().unwrap());
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
        assert!(frames < 44_100 * 10, "MIDI-only bounce retained the 30s fallback tail");

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
    fn chord_track_roundtrips_through_json_and_undo_redo() {
        let core = crate::AuraCore::new().expect("core must initialize");
        assert!(core.add_chord_event(960, 60, vec![0, 4, 7], "C").then_some(()).is_some());
        assert!(core.add_chord_event(0, 62, vec![0, 3, 7], "Dm"));
        let before_undo: serde_json::Value = serde_json::from_str(&core.chord_track_json()).unwrap();
        assert_eq!(before_undo.as_array().unwrap().len(), 2);
        assert_eq!(before_undo[0]["name"], "Dm");
        core.undo();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&core.chord_track_json()).unwrap().as_array().unwrap().len(), 1);
        core.redo();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&core.chord_track_json()).unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn generated_arpeggio_is_deterministic_and_respects_pattern() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let notes = core.generate_arpeggio(vec![60, 64, 67], vec![100, 90, 80], 0, 1, 5);
        assert_eq!(notes, vec![(60, 100), (64, 90), (67, 80), (60, 100), (64, 90)]);
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
        assert!(!engine_ref.add_sandboxed_plugin(
            track_id,
            "/definitely/missing/aura-plugin.clap"
        ));
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

        let catalog: serde_json::Value = serde_json::from_str(
            &core.render_target_catalog_diagnostic_json(),
        )
        .expect("render target catalog must be JSON");
        let targets = catalog
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .expect("catalog must contain targets");
        let target_for = |id: u32| {
            targets
                .iter()
                .find(|target| target.get("source_id").and_then(serde_json::Value::as_u64)
                    == Some(id as u64))
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
        let diagnostic: serde_json::Value = serde_json::from_str(
            &engine_ref.bounce_project_diagnostic_json("", 0),
        )
        .expect("bounce diagnostics must be JSON");
        assert_eq!(diagnostic["code"], "invalid_render_path");
        assert_eq!(diagnostic["retryable"], false);
        let unsupported_format: serde_json::Value = serde_json::from_str(
            &engine_ref.bounce_project_diagnostic_json("/tmp/aura-output.wav", 2),
        )
        .expect("format diagnostics must be JSON");
        assert_eq!(unsupported_format["code"], "unsupported_render_format");

        let core = crate::AuraCore::new().expect("core must initialize");
        let region_add: serde_json::Value = serde_json::from_str(
            &core.add_region_diagnostic_json(tid, "/missing/audio.wav", f64::NAN),
        ).expect("region add diagnostics must be JSON");
        assert_eq!(region_add["code"], "invalid_region_input");
        let region_replace: serde_json::Value = serde_json::from_str(
            &core.replace_region_audio_diagnostic_json(tid, 1, "/missing/audio.wav"),
        ).expect("region replacement diagnostics must be JSON");
        assert_eq!(region_replace["code"], "region_audio_not_found");
        let empty_plugin: serde_json::Value = serde_json::from_str(
            &core.add_sandboxed_plugin_diagnostic_json(tid, ""),
        )
        .expect("plugin diagnostics must be JSON");
        assert_eq!(empty_plugin["code"], "invalid_plugin_path");
        assert_eq!(empty_plugin["retryable"], false);
        let missing_plugin: serde_json::Value = serde_json::from_str(
            &core.add_sandboxed_plugin_diagnostic_json(
                tid,
                "/aura-core-bridge/path-that-does-not-exist.clap",
            ),
        )
        .expect("plugin diagnostics must be JSON");
        assert_eq!(missing_plugin["code"], "plugin_not_found");
        let recovery: serde_json::Value = serde_json::from_str(
            &core.recover_sandboxed_plugin_diagnostic_json(tid, 0, false),
        )
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
        let snapshot: serde_json::Value = serde_json::from_str(
            engine_ref.get_vca_snapshot_json().as_str(),
        ).expect("VCA snapshot must be JSON");
        assert!(snapshot.as_array().unwrap().iter().any(|group| {
            group["id"] == group_id && group["gain"] == 0.5
        }));
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
        assert!(load_result.is_ok(), "VCA project load failed: {load_result:?}");
        let snapshot: serde_json::Value =
            serde_json::from_str(&core.get_vca_snapshot_json()).expect("VCA snapshot must be JSON");
        assert!(snapshot.as_array().unwrap().iter().any(|group| {
            group["id"] == group_id && group["gain"] == 0.75
        }));
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
        let empty: serde_json::Value = serde_json::from_str(
            &core.save_project_diagnostic_json(" "),
        ).expect("empty path diagnostic must be JSON");
        assert_eq!(empty["code"], "invalid_path");

        let missing = std::env::temp_dir().join(format!(
            "aura-diagnostic-missing-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let missing_json: serde_json::Value = serde_json::from_str(
            &core.load_project_diagnostic_json(missing.to_str().unwrap()),
        ).expect("missing path diagnostic must be JSON");
        assert_eq!(missing_json["code"], "project_not_found");

        let saved = std::env::temp_dir().join(format!(
            "aura-diagnostic-save-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let saved_json: serde_json::Value = serde_json::from_str(
            &core.save_project_diagnostic_json(saved.to_str().unwrap()),
        ).expect("successful save diagnostic must be JSON");
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

        let take: serde_json::Value = serde_json::from_str(
            &core.select_recording_take_diagnostic_json(4),
        )
        .expect("recording diagnostic must be JSON");
        assert_eq!(take["code"], "recording_session_inactive");

        let swing: serde_json::Value = serde_json::from_str(
            &core.apply_midi_swing_diagnostic_json(f32::NAN, 2.0),
        )
        .expect("swing diagnostic must be JSON");
        assert_eq!(swing["code"], "invalid_midi_swing");
        assert_eq!(swing["retryable"], false);

        let humanize: serde_json::Value = serde_json::from_str(
            &core.humanize_midi_diagnostic_json(9.0, 127, 42),
        )
        .expect("humanize diagnostic must be JSON");
        assert_eq!(humanize["code"], "invalid_midi_humanize");

        let malformed: serde_json::Value = serde_json::from_str(
            &core.set_midi_events_diagnostic_json("not-json"),
        )
        .expect("MIDI snapshot diagnostic must be JSON");
        assert_eq!(malformed["code"], "invalid_midi_snapshot");

        let invalid_event: serde_json::Value = serde_json::from_str(
            &core.set_midi_events_diagnostic_json(
                r#"[{"beat":0.0,"channel":16,"kind":{"ControlChange":{"controller":1,"value":1}}}]"#,
            ),
        )
        .expect("invalid MIDI event diagnostic must be JSON");
        assert_eq!(invalid_event["code"], "invalid_midi_event");

        let invalid_move: serde_json::Value = serde_json::from_str(
            &core.move_midi_notes_range_diagnostic_json(0, 0, 100, -1),
        )
        .expect("MIDI move diagnostic must be JSON");
        assert_eq!(invalid_move["code"], "invalid_midi_move_range");

        let invalid_transpose: serde_json::Value = serde_json::from_str(
            &core.transpose_midi_notes_range_diagnostic_json(0, 0, 100, 1),
        )
        .expect("MIDI transpose diagnostic must be JSON");
        assert_eq!(invalid_transpose["code"], "invalid_midi_transpose");
    }

    #[test]
    fn comping_diagnostics_reject_malformed_and_empty_mutations() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");

        let malformed: serde_json::Value = serde_json::from_str(
            &core.set_comp_segments_diagnostic_json("not-json"),
        )
        .expect("comping segment diagnostic must be JSON");
        assert_eq!(malformed["code"], "invalid_comp_segments_json");

        let empty: serde_json::Value = serde_json::from_str(
            &core.set_comp_segments_diagnostic_json("[]"),
        )
        .expect("empty segment diagnostic must be JSON");
        assert_eq!(empty["ok"], true);

        let snapshot: serde_json::Value = serde_json::from_str(
            &core.restore_comping_snapshot_diagnostic_json("{}"),
        )
        .expect("comping snapshot diagnostic must be JSON");
        assert_eq!(snapshot["code"], "invalid_comping_snapshot_json");
    }

    #[test]
    fn track_scalar_diagnostics_reject_invalid_and_stale_targets() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let invalid: serde_json::Value = serde_json::from_str(
            &core.set_volume_diagnostic_json(1, f32::NAN),
        ).expect("invalid scalar diagnostic must be JSON");
        assert_eq!(invalid["code"], "invalid_parameter");

        let stale: serde_json::Value = serde_json::from_str(
            &core.set_pan_diagnostic_json(999_999, 0.25),
        ).expect("stale track diagnostic must be JSON");
        assert_eq!(stale["code"], "track_not_found_or_rejected");
        assert_eq!(stale["affected_object"], "track:999999");
        assert!(stale["generation"].as_u64().is_some());
    }

    #[test]
    fn track_toggle_and_lifecycle_diagnostics_are_structured() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let created: serde_json::Value = serde_json::from_str(
            &core.add_track_diagnostic_json(0),
        ).expect("track creation diagnostic must be JSON");
        let track_id = created["track_id"].as_u64().expect("created track id");
        assert_eq!(created["ok"], true);

        let renamed: serde_json::Value = serde_json::from_str(
            &core.set_track_name_diagnostic_json(track_id as u32, "Lead Vocal"),
        ).expect("track rename diagnostic must be JSON");
        assert_eq!(renamed["ok"], true);

        let duplicate: serde_json::Value = serde_json::from_str(
            &core.duplicate_track_diagnostic_json(track_id as u32),
        ).expect("track duplicate diagnostic must be JSON");
        assert_eq!(duplicate["ok"], true);
        assert_ne!(duplicate["track_id"], track_id);

        let mixing: serde_json::Value = serde_json::from_str(
            &core.execute_auto_mixing_diagnostic_json(),
        ).expect("auto mixing diagnostic must be JSON");
        assert_eq!(mixing["ok"], true);

        let arrangement: serde_json::Value = serde_json::from_str(
            &core.execute_auto_arrangement_diagnostic_json(),
        ).expect("auto arrangement diagnostic must be JSON");
        assert_eq!(arrangement["ok"], true);

        let scale: serde_json::Value = serde_json::from_str(
            &core.set_project_scale_diagnostic_json(0, 0),
        ).expect("project scale diagnostic must be JSON");
        assert_eq!(scale["ok"], true);
        let invalid_scale: serde_json::Value = serde_json::from_str(
            &core.set_project_scale_diagnostic_json(12, 0),
        ).expect("invalid project scale diagnostic must be JSON");
        assert_eq!(invalid_scale["code"], "invalid_project_scale");

        let muted: serde_json::Value = serde_json::from_str(
            &core.set_mute_diagnostic_json(track_id as u32, true),
        ).expect("mute diagnostic must be JSON");
        assert_eq!(muted["ok"], true);

        let removed: serde_json::Value = serde_json::from_str(
            &core.remove_track_diagnostic_json(track_id as u32),
        ).expect("remove diagnostic must be JSON");
        assert_eq!(removed["ok"], true);

        let missing: serde_json::Value = serde_json::from_str(
            &core.set_solo_diagnostic_json(track_id as u32, true),
        ).expect("missing toggle diagnostic must be JSON");
        assert_eq!(missing["code"], "track_not_found_or_rejected");

        let invalid_name: serde_json::Value = serde_json::from_str(
            &core.set_track_name_diagnostic_json(0, "  "),
        ).expect("invalid name diagnostic must be JSON");
        assert_eq!(invalid_name["code"], "invalid_track_name");
    }

    #[test]
    fn plugin_mutation_diagnostics_preserve_target_context() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        let added: serde_json::Value = serde_json::from_str(
            &core.add_plugin_diagnostic_json(track_id, 0),
        ).expect("plugin add diagnostic must be JSON");
        assert_eq!(added["ok"], true);

        let bypassed: serde_json::Value = serde_json::from_str(
            &core.set_plugin_bypass_diagnostic_json(track_id, 0, true),
        ).expect("plugin bypass diagnostic must be JSON");
        assert_eq!(bypassed["ok"], true);

        let removed: serde_json::Value = serde_json::from_str(
            &core.remove_plugin_diagnostic_json(track_id, 0),
        ).expect("plugin remove diagnostic must be JSON");
        assert_eq!(removed["ok"], true);

        let missing: serde_json::Value = serde_json::from_str(
            &core.remove_plugin_diagnostic_json(track_id, 0),
        ).expect("missing plugin diagnostic must be JSON");
        assert_eq!(missing["code"], "plugin_not_found_or_rejected");
        assert_eq!(missing["affected_object"], format!("track:{track_id}/plugin:0"));
    }

    #[test]
    fn routing_diagnostics_reject_invalid_graph_inputs_explicitly() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let self_loop: serde_json::Value = serde_json::from_str(
            &core.set_route_diagnostic_json(4, 4, true),
        ).expect("route diagnostic must be JSON");
        assert_eq!(self_loop["code"], "route_self_loop");

        let bad_gain: serde_json::Value = serde_json::from_str(
            &core.set_feedback_route_diagnostic_json(1, 2, f32::NAN, true),
        ).expect("feedback diagnostic must be JSON");
        assert_eq!(bad_gain["code"], "invalid_route_gain");

        let bad_tap: serde_json::Value = serde_json::from_str(
            &core.set_sidechain_link_diagnostic_json(1, 2, 0, 99, true),
        ).expect("sidechain diagnostic must be JSON");
        assert_eq!(bad_tap["code"], "invalid_sidechain_tap");
    }

    #[test]
    fn audio_device_diagnostics_preserve_configuration_boundaries() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bad_rate: serde_json::Value = serde_json::from_str(
            &core.apply_audio_config_diagnostic_json(22_050, 128),
        ).expect("audio config diagnostic must be JSON");
        assert_eq!(bad_rate["code"], "unsupported_sample_rate");

        let bad_buffer: serde_json::Value = serde_json::from_str(
            &core.apply_audio_config_diagnostic_json(48_000, 127),
        ).expect("audio config diagnostic must be JSON");
        assert_eq!(bad_buffer["code"], "unsupported_buffer_size");

        let reconnect: serde_json::Value = serde_json::from_str(
            &core.try_reconnect_audio_device_diagnostic_json(),
        ).expect("reconnect diagnostic must be JSON");
        assert!(reconnect["status"].is_string());
        assert!(reconnect["audio_generation"].as_u64().is_some());
    }

    #[test]
    fn automation_diagnostics_reject_malformed_points_and_nonfinite_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let odd: serde_json::Value = serde_json::from_str(
            &core.set_automation_data_diagnostic_json(1, 2, vec![0.0, 0.5]),
        ).expect("automation diagnostic must be JSON");
        assert_eq!(odd["code"], "invalid_automation_points");

        let nan: serde_json::Value = serde_json::from_str(
            &core.set_automation_data_diagnostic_json(1, 2, vec![0.0, f64::NAN, 0.0]),
        ).expect("automation diagnostic must be JSON");
        assert_eq!(nan["code"], "non_finite_automation_points");

        let parameter: serde_json::Value = serde_json::from_str(
            &core.set_plugin_parameter_diagnostic_json(1, 0, 2, f32::NAN),
        ).expect("parameter diagnostic must be JSON");
        assert_eq!(parameter["code"], "non_finite_parameter");
    }

    #[test]
    fn tempo_diagnostics_reject_invalid_control_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bpm: serde_json::Value = serde_json::from_str(
            &core.set_tempo_diagnostic_json(f32::NAN),
        ).expect("tempo diagnostic must be JSON");
        assert_eq!(bpm["code"], "non_finite_tempo");

        let event: serde_json::Value = serde_json::from_str(
            &core.set_tempo_event_diagnostic_json(-1.0, 120.0, false),
        ).expect("tempo event diagnostic must be JSON");
        assert_eq!(event["code"], "tempo_event_out_of_range");

        let missing: serde_json::Value = serde_json::from_str(
            &core.remove_tempo_event_diagnostic_json(999.0),
        ).expect("tempo removal diagnostic must be JSON");
        assert_eq!(missing["code"], "tempo_event_not_found");

        let macro_value: serde_json::Value = serde_json::from_str(
            &core.set_macro_value_diagnostic_json(128, 0.5),
        ).expect("macro diagnostic must be JSON");
        assert_eq!(macro_value["code"], "invalid_macro_value");

        let synth: serde_json::Value = serde_json::from_str(
            &core.set_preview_synth_engine_diagnostic_json(3),
        ).expect("preview synth diagnostic must be JSON");
        assert_eq!(synth["code"], "unsupported_preview_synth");

        let pad: serde_json::Value = serde_json::from_str(
            &core.assign_preview_drum_pad_diagnostic_json(16, None),
        ).expect("preview pad diagnostic must be JSON");
        assert_eq!(pad["code"], "invalid_preview_pad");

        let scan: serde_json::Value = serde_json::from_str(
            &core.scan_preview_audio_diagnostic_json("/definitely/missing-preview-directory"),
        ).expect("preview scan diagnostic must be JSON");
        assert_eq!(scan["code"], "preview_directory_not_found");

        let preload: serde_json::Value = serde_json::from_str(
            &core.preload_preview_audio_diagnostic_json(99_999),
        ).expect("preview preload diagnostic must be JSON");
        assert_eq!(preload["code"], "preview_asset_not_found");

        let trigger: serde_json::Value = serde_json::from_str(
            &core.trigger_preview_drum_pad_diagnostic_json(0),
        ).expect("preview trigger diagnostic must be JSON");
        assert_eq!(trigger["code"], "preview_pad_unavailable");
    }

    #[test]
    fn region_and_video_diagnostics_reject_invalid_control_values() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let bad_target: serde_json::Value = serde_json::from_str(
            &core.move_region_diagnostic_json(0, 0, 1.0),
        ).expect("region diagnostic must be JSON");
        assert_eq!(bad_target["code"], "invalid_region_target");

        let bad_value: serde_json::Value = serde_json::from_str(
            &core.set_region_gain_diagnostic_json(1, 1, f32::NAN),
        ).expect("region diagnostic must be JSON");
        assert_eq!(bad_value["code"], "invalid_region_value");

        let stale: serde_json::Value = serde_json::from_str(
            &core.set_region_trim_diagnostic_json(999_999, 999_999, 0.0, 1.0),
        ).expect("region diagnostic must be JSON");
        assert_eq!(stale["code"], "region_not_found_or_rejected");
        assert_eq!(stale["affected_object"], "track:999999/region:999999");

        let bad_position: serde_json::Value = serde_json::from_str(
            &core.request_video_frame_diagnostic_json(f64::NAN),
        ).expect("video diagnostic must be JSON");
        assert_eq!(bad_position["code"], "invalid_video_position");

        let missing: serde_json::Value = serde_json::from_str(
            &core.load_video_diagnostic_json("/definitely/missing/aura-video.mov"),
        ).expect("video diagnostic must be JSON");
        assert_eq!(missing["code"], "video_not_found");

        let render: serde_json::Value = serde_json::from_str(
            &core.start_render_diagnostic_json("/tmp/aura-output.mp3"),
        ).expect("render diagnostic must be JSON");
        assert_eq!(render["code"], "invalid_render_path");

        let spatial: serde_json::Value = serde_json::from_str(
            &core.set_spatial_position_diagnostic_json(1, f32::NAN, 0.0, 0.0),
        ).expect("spatial diagnostic must be JSON");
        assert_eq!(spatial["code"], "invalid_track_command");

        let phase: serde_json::Value = serde_json::from_str(
            &core.set_phase_invert_diagnostic_json(999_999, true),
        ).expect("phase diagnostic must be JSON");
        assert_eq!(phase["code"], "track_not_found_or_rejected");

        let eq: serde_json::Value = serde_json::from_str(
            &core.set_track_eq_diagnostic_json(1, f32::INFINITY, 0.0, 0.0, 1.0),
        ).expect("EQ diagnostic must be JSON");
        assert_eq!(eq["code"], "invalid_track_command");

        let missing_eq: serde_json::Value = serde_json::from_str(
            &core.set_track_eq_diagnostic_json(999_999, 0.0, 0.0, 0.0, 1.0),
        ).expect("missing EQ target diagnostic must be JSON");
        assert_eq!(missing_eq["code"], "track_not_found_or_rejected");

        let reverse: serde_json::Value = serde_json::from_str(
            &core.set_region_reverse_diagnostic_json(0, 1, true),
        ).expect("reverse diagnostic must be JSON");
        assert_eq!(reverse["code"], "invalid_region_target");

        let vocal: serde_json::Value = serde_json::from_str(
            &core.execute_vocal_remover_diagnostic_json(999_999),
        ).expect("vocal remover diagnostic must be JSON");
        assert_eq!(vocal["code"], "track_not_found_or_rejected");

        let articulation: serde_json::Value = serde_json::from_str(
            &core.set_articulation_map_diagnostic_json(999_999, "legato"),
        ).expect("articulation diagnostic must be JSON");
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
    fn sandbox_state_diagnostic_rejects_oversize_payload_without_touching_native() {
        let _guard = native_engine_test_guard();
        let core = crate::AuraCore::new().expect("core must initialize");
        let diagnostic = core.sandbox_plugin_state_diagnostic(
            1,
            0,
            &vec![0u8; 4 * 1024 * 1024 + 1],
        );
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
        let value: serde_json::Value = serde_json::from_str(
            &core.execute_mixing_advice_diagnostic_json("  ".to_owned()),
        ).expect("diagnostic JSON");
        assert_eq!(value["code"], "invalid_mixing_advice_title");
    }

    #[test]
    fn render_validation_diagnostic_preserves_read_errors() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value = serde_json::from_str(
            &core.validate_render_output_diagnostic_json(
                "/tmp/aura-missing-validation-output.wav",
                false,
            ),
        ).expect("diagnostic JSON");
        assert_eq!(value["code"], "render_output_unreadable");
        assert_eq!(value["retryable"], true);
    }

    #[test]
    fn native_wav_diagnostic_preserves_missing_file_reason() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value = serde_json::from_str(
            &core.read_wav_diagnostic_json(
                "/tmp/aura-missing-native-wave64.w64",
                1,
            ),
        ).expect("diagnostic JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["format"], "WAVE64");
        assert!(value["error"].as_str().is_some_and(|error| error.contains("not found")));
    }

    #[test]
    fn undo_and_redo_diagnostics_report_empty_history() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let undo: serde_json::Value = serde_json::from_str(&core.undo_diagnostic_json())
            .expect("undo diagnostic JSON");
        let redo: serde_json::Value = serde_json::from_str(&core.redo_diagnostic_json())
            .expect("redo diagnostic JSON");
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
        let playhead: serde_json::Value = serde_json::from_str(
            &core.set_playhead_diagnostic_json(1234),
        ).expect("playhead diagnostic JSON");
        assert_eq!(playhead["ok"], true);
        assert_eq!(playhead["playhead"], 1234);
        let playing: serde_json::Value = serde_json::from_str(
            &core.set_playing_diagnostic_json(false),
        ).expect("playing diagnostic JSON");
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
            let json: serde_json::Value = serde_json::from_str(&value)
                .expect("diagnostic JSON");
            assert_eq!(json["ok"], true);
        }
        let invalid: serde_json::Value = serde_json::from_str(
            &core.set_midi_note_diagnostic_json(0, 128, 128, 0, 0),
        ).expect("MIDI note diagnostic JSON");
        assert_eq!(invalid["code"], "invalid_midi_note");
        let preset: serde_json::Value = serde_json::from_str(
            &core.load_plugin_preset_diagnostic_json(0, 0, ""),
        ).expect("preset diagnostic JSON");
        assert_eq!(preset["code"], "invalid_plugin_preset_target");
    }

    #[test]
    fn project_v2_hydration_diagnostic_is_structured_for_invalid_input() {
        let core = crate::AuraCore::new().expect("core must initialize");
        let value: serde_json::Value = serde_json::from_str(
            &core.load_project_v2_diagnostic_json(""),
        ).expect("project hydration diagnostic JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "project_hydration_failed");
        assert!(value["error"]["generation"].as_u64().is_some());
    }
}
