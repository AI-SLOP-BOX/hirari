    use super::execute;
    use crate::command_api::{validate, CommandAction, CommandDocument, Permission};
    use crate::AuraCore;

    #[test]
    fn project_inspect_exposes_midi_and_chord_state() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "inspect-project".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        })
        .expect("project inspection must validate");
        let report = execute(&core, &command).expect("project inspection must execute");
        let result = &report.results[0];
        assert!(result
            .get("midi_notes")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("chord_track")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("vca_groups")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("track_stacks")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("macro_mappings")
            .is_some_and(|value| value.is_array()));
    }

    #[test]
    fn set_midi_note_applies_vocal_articulation_metadata() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "vocal-note".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::SetMidiNote {
                track_id: 1,
                pitch: 60,
                velocity: 100,
                start_sample: 0,
                length_samples: 48_000,
                lyric: "la".into(),
                phoneme: "a".into(),
                pitch_curve_cents: vec![0, 20, -10],
                vibrato_depth_cents: 32,
                portamento_samples: 960,
            }],
        })
        .expect("vocal note command must validate");
        execute(&core, &command).expect("vocal note command must execute");
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["phoneme"], "a");
        assert_eq!(
            notes[0]["pitch_curve_cents"],
            serde_json::json!([0, 20, -10])
        );
        assert_eq!(notes[0]["vibrato_depth_cents"], 32);
        assert_eq!(notes[0]["portamento_samples"], 960);
        core.undo();
        let after_undo: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_ne!(after_undo[0]["phoneme"], "a");
        core.redo();
        let after_redo: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(after_redo[0]["phoneme"], "a");
        assert_eq!(after_redo[0]["vibrato_depth_cents"], 32);
    }

    #[test]
    fn logical_editor_rule_executes_through_command_and_undo() {
        let core = AuraCore::new().expect("core must initialize");
        let add = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "logical-editor-seed".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::SetMidiNote {
                track_id: 1,
                pitch: 60,
                velocity: 40,
                start_sample: 0,
                length_samples: 480,
                lyric: String::new(),
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
            }],
        })
        .unwrap();
        execute(&core, &add).unwrap();
        let rule = crate::midi_logical_editor::MidiLogicalRule {
            predicate: crate::midi_logical_editor::MidiNotePredicate {
                track_id: Some(1),
                ..Default::default()
            },
            transforms: vec![
                crate::midi_logical_editor::MidiNoteTransform::Transpose { semitones: 12 },
                crate::midi_logical_editor::MidiNoteTransform::SetVelocity { velocity: 100 },
            ],
        };
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "logical-editor".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::ApplyMidiLogicalRule { rule }],
        })
        .unwrap();
        let report = execute(&core, &command).unwrap();
        assert_eq!(report.results[0]["changed"], 1);
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["pitch"], 72);
        assert_eq!(notes[0]["velocity"], 100);
        core.undo();
        let restored: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(restored[0]["pitch"], 60);
        assert_eq!(restored[0]["velocity"], 40);
    }

    #[test]
    fn mix_snapshot_commands_capture_and_diff_states() {
        let core = AuraCore::new().expect("core must initialize");
        let capture = |name: &str, states| {
            validate(CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: format!("snapshot-{name}"),
                permission: Permission::ProjectWrite,
                expected_generation: Some(core.project_generation()),
                expected_audio_generation: Some(core.audio_config_generation()),
                actions: vec![CommandAction::TakeMixSnapshot {
                    name: name.into(),
                    states,
                }],
            })
            .unwrap()
        };
        execute(
            &core,
            &capture("A", std::collections::HashMap::from([(1, 0.5)])),
        )
        .unwrap();
        execute(
            &core,
            &capture("B", std::collections::HashMap::from([(1, 0.8), (2, 0.2)])),
        )
        .unwrap();
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "snapshot-diff".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::DiffMixSnapshots {
                first: 0,
                second: 1,
            }],
        })
        .unwrap();
        let report = execute(&core, &command).unwrap();
        assert_eq!(report.results[0]["diff"].as_array().unwrap().len(), 2);
        let recall = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "snapshot-recall".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecallMixSnapshot { index: 1 }],
        })
        .unwrap();
        let recalled = execute(&core, &recall).unwrap();
        assert!((recalled.results[0]["states"]["2"].as_f64().unwrap() - 0.2).abs() < 1.0e-5);
    }

    #[test]
    fn vocal_pitch_preview_is_read_only_and_bounded() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "vocal-preview".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::PreviewVocalPitchCorrection {
                samples: vec![0.1, -0.1, 0.1, -0.1],
                sample_rate: 48_000.0,
                speed: 0.75,
                timing_ratio: 2.0,
            }],
        })
        .expect("vocal preview must validate");
        let before = core.project_generation();
        let report = execute(&core, &command).expect("vocal preview must execute");
        assert_eq!(core.project_generation(), before);
        assert_eq!(
            report.results[0]["operation"],
            "preview_vocal_pitch_correction"
        );
        assert_eq!(report.results[0]["timing_ratio"], 2.0);
        assert_eq!(report.results[0]["samples"].as_array().unwrap().len(), 8);
    }
