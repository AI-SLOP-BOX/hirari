    #[test]
    fn read_only_commands_cannot_mutate_project_state() {
        let document = CommandDocument {
            transaction: "read-only-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::SetVolume {
                track_id: 1,
                value: 0.5,
            }],
        };
        assert_eq!(
            validate(document).unwrap_err(),
            "read_only permission allows project inspection only"
        );
    }

    #[test]
    fn read_only_project_inspection_remains_allowed() {
        let document = CommandDocument {
            transaction: "inspect".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        };
        assert!(validate(document).is_ok());
    }

    #[test]
    fn rejects_commands_from_stale_project_or_audio_generations() {
        let command = CommandDocument {
            transaction: "stale".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(4),
            expected_audio_generation: Some(9),
            actions: vec![CommandAction::Undo],
        };
        assert!(validate_for_generations(command.clone(), 3, 9)
            .unwrap_err()
            .starts_with("stale_project_generation:"));
        assert!(validate_for_generations(command, 4, 8)
            .unwrap_err()
            .starts_with("stale_audio_generation:"));
    }

    #[test]
    fn accepts_commands_when_both_generations_match() {
        let command = CommandDocument {
            transaction: "current".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(4),
            expected_audio_generation: Some(9),
            actions: vec![CommandAction::Undo],
        };
        assert!(validate_for_generations(command, 4, 9).is_ok());
    }

    #[test]
    fn apply_rejects_mutation_without_both_snapshot_generations() {
        let base = CommandDocument {
            transaction: "unsafe-apply".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetVolume {
                track_id: 1,
                value: 1.0,
            }],
        };
        assert!(validate_for_apply(base.clone(), 1, 1)
            .unwrap_err()
            .starts_with("missing_project_generation:"));

        let mut audio_missing = base;
        audio_missing.expected_generation = Some(1);
        assert!(validate_for_apply(audio_missing, 1, 1)
            .unwrap_err()
            .starts_with("missing_audio_generation:"));
    }

    #[test]
    fn structured_apply_validation_preserves_generation_context() {
        let error = validate_for_apply_diagnostic(
            CommandDocument {
                transaction: "edit".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ProjectWrite,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::Undo],
            },
            42,
            7,
        )
        .unwrap_err();
        assert_eq!(error.code, "missing_project_generation");
        assert_eq!(error.generation, Some(42));
        assert!(error.retryable);
    }

    #[test]
    fn apply_allows_read_only_inspection_without_generations() {
        let command = CommandDocument {
            transaction: "inspect".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        };
        assert!(validate_for_apply(command, 1, 1).is_ok());
    }

    #[test]
    fn apply_allows_plugin_catalog_without_generations() {
        let command = CommandDocument {
            transaction: "catalog".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::PluginCatalog],
        };
        assert!(validate_for_apply(command, 1, 1).is_ok());
    }

    #[test]
    fn snapshot_generation_is_stable_and_content_scoped() {
        assert_eq!(
            snapshot_generation(b"layout"),
            snapshot_generation(b"layout")
        );
        assert_ne!(
            snapshot_generation(b"layout"),
            snapshot_generation(b"layout-v2")
        );
    }

    #[test]
    fn mutation_classes_distinguish_reversible_and_external_effects() {
        assert_eq!(
            mutation_class(&CommandAction::ProjectInspect),
            MutationClass::ReadOnly
        );
        assert_eq!(
            mutation_class(&CommandAction::SetPan {
                track_id: 1,
                value: 0.0
            }),
            MutationClass::Reversible
        );
        assert_eq!(
            mutation_class(&CommandAction::BounceProject {
                path: "mix.wav".into(),
                format: 0
            }),
            MutationClass::ExternalSideEffect
        );
        assert_eq!(
            mutation_class(&CommandAction::ProjectLoad {
                path: "project.aura".into(),
            }),
            MutationClass::Irreversible
        );
        assert_eq!(
            mutation_class(&CommandAction::RemoveTrack { track_id: 1 }),
            MutationClass::Irreversible
        );
        assert_eq!(
            mutation_class(&CommandAction::SetMute {
                track_id: 1,
                muted: true,
            }),
            MutationClass::Reversible
        );
        assert_eq!(
            mutation_class(&CommandAction::ExtensionSetEnabled {
                root: "project".into(),
                extension_id: "example".into(),
                enabled: false,
            }),
            MutationClass::Reversible
        );
    }

    #[test]
    fn daily_mix_actions_round_trip_and_have_explicit_diffs() {
        let document = CommandDocument {
            transaction: "mix-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![
                CommandAction::SetMute {
                    track_id: 7,
                    muted: true,
                },
                CommandAction::RemovePlugin {
                    track_id: 7,
                    plugin_index: 2,
                },
            ],
        };
        let encoded = serde_json::to_value(&document).unwrap();
        let decoded: CommandDocument = serde_json::from_value(encoded).unwrap();
        let validated = validate(decoded).unwrap();
        let summaries = diff(&validated)
            .into_iter()
            .map(|item| item.summary)
            .collect::<Vec<_>>();
        assert_eq!(summaries[0], "set track 7 mute: true");
        assert_eq!(summaries[1], "remove plugin 2 from track 7");
        assert_eq!(validated.mutation_class, MutationClass::Irreversible);
    }

    #[test]
    fn midi_inspection_is_read_only_and_round_trips() {
        let document = CommandDocument {
            permission: Permission::ReadOnly,
            actions: vec![CommandAction::InspectMidiNotes],
            ..validated_command_document("inspect-midi")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::ReadOnly);
        assert_eq!(
            diff(&validated)[0].summary,
            "inspect canonical MIDI notes and lyrics"
        );
    }

    #[test]
    fn transport_pause_round_trips_and_is_reversible() {
        let document = CommandDocument {
            actions: vec![CommandAction::TransportPause],
            ..validated_command_document("pause")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.actions, vec![CommandAction::TransportPause]);
        assert_eq!(diff(&validated)[0].summary, "pause transport");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
    }

    #[test]
    fn vca_actions_round_trip_validate_and_describe() {
        let document = CommandDocument {
            actions: vec![
                CommandAction::AddVcaGroup {
                    group_id: 9,
                    gain: 0.75,
                },
                CommandAction::AssignTrackToVca {
                    track_id: 12,
                    group_id: 9,
                },
                CommandAction::SetVcaGroupGain {
                    group_id: 9,
                    gain: 0.5,
                },
            ],
            ..validated_command_document("vca-edit")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        let summaries = diff(&validated)
            .into_iter()
            .map(|item| item.summary)
            .collect::<Vec<_>>();
        assert_eq!(summaries[0], "add VCA group 9 at gain 0.75");
        assert_eq!(summaries[1], "assign track 12 to VCA group 9");
        assert_eq!(summaries[2], "set VCA group 9 gain to 0.5");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);

        for action in [
            CommandAction::AddVcaGroup {
                group_id: 0,
                gain: 1.0,
            },
            CommandAction::SetVcaGroupGain {
                group_id: 9,
                gain: f32::NAN,
            },
            CommandAction::AssignTrackToVca {
                track_id: 0,
                group_id: 9,
            },
        ] {
            let invalid = CommandDocument {
                actions: vec![action],
                ..validated_command_document("bad-vca")
            };
            assert!(validate(invalid).is_err());
        }
    }

    #[test]
    fn low_latency_action_round_trips_as_reversible() {
        let document = CommandDocument {
            actions: vec![CommandAction::SetLowLatencyMode { enabled: true }],
            ..validated_command_document("latency-edit")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        assert_eq!(diff(&validated)[0].summary, "enable low-latency monitoring");
    }

    #[test]
    fn tonal_scale_action_validates_root_and_mode() {
        let document = CommandDocument {
            actions: vec![CommandAction::SetTonalScale {
                root: 7,
                scale_type: 0,
            }],
            ..validated_command_document("tonal-edit")
        };
        let validated = validate(document).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        assert_eq!(
            diff(&validated)[0].summary,
            "set tonal scale root 7, type 0"
        );

        for action in [
            CommandAction::SetTonalScale {
                root: 12,
                scale_type: 11,
            },
            CommandAction::SetTonalScale {
                root: -129,
                scale_type: 0,
            },
        ] {
            assert!(validate(CommandDocument {
                actions: vec![action],
                ..validated_command_document("invalid-tonal-edit")
            })
            .is_err());
        }
    }

    #[test]
    fn project_load_requires_generations_and_project_write() {
        let command = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "load".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(7),
            expected_audio_generation: Some(3),
            actions: vec![CommandAction::ProjectLoad {
                path: "other.aura".into(),
            }],
        };
        assert!(validate_for_apply(command, 7, 3).is_ok());

        let missing_generations = CommandDocument {
            expected_generation: None,
            expected_audio_generation: None,
            ..CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: "load-missing-generation".into(),
                permission: Permission::ProjectWrite,
                expected_generation: Some(7),
                expected_audio_generation: Some(3),
                actions: vec![CommandAction::ProjectLoad {
                    path: "other.aura".into(),
                }],
            }
        };
        let error = validate_for_apply_diagnostic(missing_generations, 7, 3).unwrap_err();
        assert_eq!(error.code, "missing_project_generation");
    }

    #[test]
    fn project_load_paths_use_the_same_root_policy_as_render_paths() {
        let root = std::env::temp_dir().join(format!("aura-command-load-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let actions = vec![CommandAction::ProjectLoad {
            path: "../outside.aura".into(),
        }];
        let error =
            validate_command_action_paths(&actions, Permission::ProjectWrite, &root).unwrap_err();
        assert_eq!(error.code, "path_traversal");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn external_file_operations_require_system_write_permission() {
        let base = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "save".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::SaveProject {
                path: "project.aura".into(),
            }],
        };
        assert!(validate(base.clone()).is_err());
        let allowed = CommandDocument {
            permission: Permission::SystemWrite,
            ..base
        };
        assert!(validate(allowed).is_ok());
    }
