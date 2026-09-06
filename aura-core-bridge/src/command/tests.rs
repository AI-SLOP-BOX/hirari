#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamics_analysis_is_read_only_and_bounded() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "dynamics-analysis".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeDynamics {
                samples: vec![0.0, 0.25, -0.5, 0.1],
                track_id: Some(7),
            }],
        };
        let command = validate_for_apply(document, 1, 1).expect("analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        let core = crate::AuraCore::new().expect("core must initialize");
        let report =
            crate::command_executor::execute(&core, &command).expect("analysis should execute");
        assert_eq!(report.results[0]["operation"], "analyze_dynamics");
        assert_eq!(report.results[0]["track_id"], 7);
        assert_eq!(report.results[0]["gain_staging_target_db"], -6.0);
        assert!(report.results[0]["recommended_gain_db"].is_number());
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-dynamics".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeDynamics {
                samples: vec![f32::NAN],
                track_id: None
            }],
        })
        .is_err());
    }

    #[test]
    fn silence_analysis_is_read_only_and_returns_bounded_ranges() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "silence-analysis".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeSilence {
                samples: vec![0.0, 0.0, 0.8, 0.0],
                threshold: 0.01,
                min_length: 2,
            }],
        };
        let command =
            validate_for_apply(document, 1, 1).expect("silence analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-silence".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeSilence {
                samples: vec![0.0],
                threshold: 2.0,
                min_length: 1,
            }],
        })
        .is_err());
    }

    #[test]
    fn mix_analysis_is_read_only_and_rejects_non_finite_channels() {
        let command = validate_for_apply(
            CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: "mix-analysis".into(),
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::AnalyzeMix {
                    left: vec![0.5; 8],
                    right: vec![-0.5; 8],
                    reference_left: Vec::new(),
                    reference_right: Vec::new(),
                    ab_left: Vec::new(),
                    ab_right: Vec::new(),
                }],
            },
            1,
            1,
        )
        .expect("mix analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-mix".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeMix {
                left: vec![f32::NAN],
                right: vec![0.0],
                reference_left: Vec::new(),
                reference_right: Vec::new(),
                ab_left: Vec::new(),
                ab_right: Vec::new()
            }],
        })
        .is_err());
    }

    #[test]
    fn dynamics_suggestion_apply_is_reversible_and_requires_generations() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "dynamics-apply".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(7),
            expected_audio_generation: Some(3),
            actions: vec![CommandAction::ApplyDynamicsSuggestion {
                track_id: 1,
                plugin_index: 0,
                samples: vec![0.0, 0.25, -0.5, 0.1],
            }],
        };
        let command = validate_for_apply(document, 7, 3).expect("apply should be admitted");
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate_for_apply(
            CommandDocument {
                expected_generation: None,
                expected_audio_generation: Some(3),
                ..CommandDocument {
                    schema_version: 1,
                    command_version: 1,
                    transaction: "dynamics-apply".into(),
                    permission: Permission::ProjectWrite,
                    expected_generation: None,
                    expected_audio_generation: Some(3),
                    actions: vec![CommandAction::ApplyDynamicsSuggestion {
                        track_id: 1,
                        plugin_index: 0,
                        samples: vec![0.0],
                    }],
                }
            },
            7,
            3
        )
        .is_err());
    }

    fn route_gain_document(gain: f32, enabled: bool) -> CommandDocument {
        CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "route-gain-test".into(),
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetRouteGain {
                source_id: 1,
                dest_id: 2,
                gain,
                enabled,
            }],
        }
    }

    #[test]
    fn route_gain_command_accepts_bounded_values() {
        assert!(validate(route_gain_document(0.0, false)).is_ok());
        assert!(validate(route_gain_document(1.0, true)).is_ok());
        assert!(validate(route_gain_document(2.0, true)).is_ok());
    }

    #[test]
    fn route_gain_command_rejects_invalid_values() {
        assert!(validate(route_gain_document(-0.01, false)).is_err());
        assert!(validate(route_gain_document(2.01, true)).is_err());
        assert!(validate(route_gain_document(0.0, true)).is_err());
        let mut self_route = route_gain_document(1.0, true);
        self_route.actions = vec![CommandAction::SetRouteGain {
            source_id: 7,
            dest_id: 7,
            gain: 1.0,
            enabled: true,
        }];
        assert!(validate(self_route).is_err());
    }

    #[test]
    fn rejects_invalid_protocol_envelopes() {
        let request = ProtocolRequest {
            protocol: "aura.command.v0".into(),
            request_id: "1".into(),
            client: "codex".into(),
            command: CommandDocument {
                transaction: "inspect".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::ProjectInspect],
            },
        };
        assert_eq!(
            validate_request_envelope(&request).unwrap_err().code,
            "unsupported_protocol"
        );
    }

    #[test]
    fn rejects_untraceable_request_metadata() {
        let request = ProtocolRequest {
            protocol: PROTOCOL_VERSION.into(),
            request_id: "".into(),
            client: "".into(),
            command: CommandDocument {
                transaction: "inspect".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::ProjectInspect],
            },
        };
        assert_eq!(
            validate_request_envelope(&request).unwrap_err().code,
            "invalid_request_id"
        );
    }

    #[test]
    fn validates_declarative_batch() {
        let command = validate(CommandDocument {
            transaction: "vocal-polish".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddTrack {
                name: "Vocal FX".into(),
                track_type: 0,
            }],
        })
        .unwrap();
        assert_eq!(command.transaction, "vocal-polish");
    }

    #[test]
    fn validates_recording_lifecycle_limits() {
        let command = validate(CommandDocument {
            transaction: "record-vocal".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecordStart {
                sample_rate: 48_000.0,
                channels: 2,
                max_frames: 48_000 * 60 * 5,
                start_sample: 0,
                count_in_frames: 0,
            }],
        })
        .unwrap();
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate(CommandDocument {
            transaction: "invalid-record".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecordStart {
                sample_rate: 48_000.0,
                channels: 2,
                max_frames: 16_777_217,
                start_sample: 0,
                count_in_frames: 0,
            }],
        })
        .is_err());
    }

    #[test]
    fn validates_macro_mapping_wire_contract() {
        let command = validate(CommandDocument {
            transaction: "macro-bind".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddMacroMapping {
                mapping_id: "cutoff".into(),
                macro_index: 3,
                target_instance_id: "track:1:slot:0".into(),
                target_parameter_id: "12".into(),
                min: 0.1,
                max: 0.9,
                curve: 0.2,
                invert: false,
            }],
        })
        .unwrap();
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate(CommandDocument {
            transaction: "bad-macro-bind".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddMacroMapping {
                mapping_id: "cutoff".into(),
                macro_index: 3,
                target_instance_id: "../../escape".into(),
                target_parameter_id: "12".into(),
                min: 0.1,
                max: 0.9,
                curve: 0.2,
                invert: false,
            }],
        })
        .is_err());
    }

    #[test]
    fn openutau_operation_accepts_canonical_and_legacy_wire_names() {
        let base = serde_json::json!({
            "track_id": 1,
            "source_path": "voice.ustx",
            "rendered_audio_path": "voice.wav"
        });
        let canonical = serde_json::from_value::<CommandAction>({
            let mut value = base.clone();
            value["op"] = serde_json::json!("open_utau_import");
            value
        })
        .unwrap();
        let legacy = serde_json::from_value::<CommandAction>({
            let mut value = base;
            value["op"] = serde_json::json!("openutau_import");
            value
        })
        .unwrap();
        assert_eq!(canonical, legacy);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_values() {
        for value in [f32::NAN, f32::INFINITY, 3.0] {
            assert!(validate(CommandDocument {
                transaction: "bad".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ProjectWrite,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::SetVolume { track_id: 1, value }],
            })
            .is_err());
        }
    }

    #[test]
    fn eq_command_is_reversible_and_rejects_non_finite_bands() {
        let valid = CommandDocument {
            transaction: "eq-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetEq {
                track_id: 1,
                low_band: 0.2,
                low_cut: 0.1,
                high_band: 0.7,
                high_cut: 0.8,
            }],
        };
        let validated = validate(valid).expect("EQ command should validate");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        let invalid = CommandDocument {
            transaction: "bad-eq".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetEq {
                track_id: 1,
                low_band: f32::NAN,
                low_cut: 0.1,
                high_band: 0.7,
                high_cut: 0.8,
            }],
        };
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn validates_sample_automation_and_rejects_unsorted_points() {
        let valid = CommandDocument {
            transaction: "automation".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetAutomation {
                track_id: 1,
                parameter_id: 7,
                points: vec![0.0, 0.2, 0.0, 22050.0, 0.8, 0.2, 44100.0, 0.4, 0.0],
            }],
        };
        assert!(validate(valid).is_ok());

        let mut unsorted = CommandDocument {
            transaction: "automation-unsorted".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetAutomation {
                track_id: 1,
                parameter_id: 7,
                points: vec![22050.0, 0.2, 0.0, 0.0, 0.8, 0.0],
            }],
        };
        assert!(validate(unsorted.clone()).is_err());
        unsorted.actions = vec![CommandAction::SetAutomation {
            track_id: 1,
            parameter_id: 7,
            points: vec![0.0, 0.2, 0.0, 22050.0, 1.1, 0.0],
        }];
        assert!(validate(unsorted).is_err());
    }

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

    #[test]
    fn selected_stem_targets_are_validated_and_described() {
        let command = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "selected-stems".into(),
            permission: Permission::SystemWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::BounceStems {
                output_dir: "stems".into(),
                format: 0,
                track_ids: vec![2, 5],
                tail_seconds: 2.0,
                pre_fader: false,
                include_inserts: true,
            }],
        };
        let validated = validate(command).unwrap();
        assert!(diff(&validated)[0].summary.contains("2 selected stems"));

        let duplicate = CommandDocument {
            actions: vec![CommandAction::BounceStems {
                output_dir: "stems".into(),
                format: 0,
                track_ids: vec![2, 2],
                tail_seconds: 2.0,
                pre_fader: false,
                include_inserts: true,
            }],
            ..validated_command_document("duplicate-stems")
        };
        assert!(validate(duplicate).is_err());
    }

    fn validated_command_document(transaction: &str) -> CommandDocument {
        CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: transaction.into(),
            permission: Permission::SystemWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: Vec::new(),
        }
    }

    #[test]
    fn project_paths_cannot_escape_root() {
        let root = std::env::temp_dir().join(format!("aura-command-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(validate_project_path(&root, "mix.wav").is_ok());
        assert_eq!(
            validate_project_path(&root, "../outside.wav")
                .unwrap_err()
                .code,
            "path_traversal"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unrestricted_policy_allows_explicit_external_paths() {
        let root =
            std::env::temp_dir().join(format!("aura-unrestricted-root-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("aura-unrestricted-output-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let requested = outside.join("render.wav");
        let resolved =
            validate_command_path(&root, requested.to_str().unwrap(), PathPolicy::Unrestricted)
                .unwrap();
        assert_eq!(resolved, outside.canonicalize().unwrap().join("render.wav"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn output_paths_require_explicit_overwrite() {
        let root = std::env::temp_dir().join(format!("aura-output-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mix.wav"), b"existing").unwrap();
        assert_eq!(
            validate_project_output_path(&root, "mix.wav", false)
                .unwrap_err()
                .code,
            "overwrite_confirmation_required"
        );
        assert!(validate_project_output_path(&root, "mix.wav", true).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn command_paths_reject_symlink_components() {
        let root = std::env::temp_dir().join(format!("aura-symlink-root-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("aura-symlink-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        assert_eq!(
            validate_project_path(&root, "linked/output.wav")
                .unwrap_err()
                .code,
            "symlink_path"
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn request_ledger_replays_and_rejects_duplicate_requests() {
        let root = std::env::temp_dir().join(format!("aura-ledger-{}", std::process::id()));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        ledger
            .record("req-1", serde_json::json!({"ok": true}), &path)
            .unwrap();
        assert_eq!(
            ledger.replay("req-1"),
            Some(serde_json::json!({"ok": true}))
        );
        assert!(ledger
            .record("req-1", serde_json::json!({"ok": false}), &path)
            .is_err());
        assert!(ledger
            .record_once(
                "req-2",
                Some("tx-1"),
                serde_json::json!({"ok": true}),
                &path
            )
            .is_ok());
        assert!(ledger
            .record_once(
                "req-3",
                Some("tx-1"),
                serde_json::json!({"ok": false}),
                &path
            )
            .is_err());
        let lock_path = path.with_extension("lock");
        std::fs::write(&lock_path, b"other process").unwrap();
        assert_eq!(
            ledger
                .record("req-4", serde_json::json!({}), &path)
                .unwrap_err()
                .code,
            "ledger_busy"
        );
        let _ = std::fs::remove_file(lock_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_persists_in_flight_before_completion() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-in-flight-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .begin("req-flight", Some("tx-flight"), &path)
                .unwrap(),
            LedgerBegin::Started
        );

        let reopened = RequestLedger::open(&path).unwrap();
        assert_eq!(reopened.replay("req-flight"), None);
        let mut retry = reopened;
        assert_eq!(
            retry
                .begin("req-flight", Some("tx-flight"), &path)
                .unwrap_err()
                .code,
            "ledger_in_flight"
        );

        ledger
            .complete("req-flight", serde_json::json!({"ok": true}), &path)
            .unwrap();
        let completed = RequestLedger::open(&path).unwrap();
        assert_eq!(
            completed.replay("req-flight"),
            Some(serde_json::json!({"ok": true}))
        );
        assert_eq!(
            completed.replay("transaction:tx-flight"),
            Some(serde_json::json!({"ok": true}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_replay_is_a_successful_idempotent_outcome() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-replay-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .record_or_replay(
                    "req-replay",
                    Some("tx-replay"),
                    serde_json::json!({"created": "track-1"}),
                    &path,
                )
                .unwrap(),
            LedgerOutcome::Applied(serde_json::json!({"created": "track-1"}))
        );
        assert_eq!(
            ledger
                .record_or_replay(
                    "req-replay",
                    Some("tx-replay"),
                    serde_json::json!({"created": "track-2"}),
                    &path,
                )
                .unwrap(),
            LedgerOutcome::Replayed(serde_json::json!({"created": "track-1"}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audit_log_is_deterministic_and_marks_replayable_results() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-audit-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        ledger
            .record("req-b", serde_json::json!({"ok": true}), &path)
            .unwrap();
        ledger
            .record("req-a", serde_json::json!({"ok": false}), &path)
            .unwrap();
        let log = ledger.audit_log();
        assert_eq!(
            log.iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["req-a", "req-b"]
        );
        assert!(log.iter().all(|entry| entry.replayable));
        assert_eq!(log[0].result, Some(serde_json::json!({"ok": false})));
        let json = audit_log_json(&path).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["request_id"], "req-a");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_publishes_applying_state_before_side_effect() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-applying-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .begin("req-applying", Some("tx-applying"), &path)
                .unwrap(),
            LedgerBegin::Started
        );
        ledger.mark_applying("req-applying", &path).unwrap();
        let mut reopened = RequestLedger::open(&path).unwrap();
        let entry = reopened.entries.get("req-applying").unwrap();
        assert!(matches!(entry.state, LedgerState::Applying));
        assert!(reopened
            .begin("req-applying", Some("tx-applying"), &path)
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn transaction_lock_creates_parent_for_a_new_project() {
        let root = std::env::temp_dir().join(format!(
            "aura-transaction-lock-parent-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let ledger_path = root.join(".aura").join("request-ledger.json");
        let lock = CommandTransactionLock::acquire(&ledger_path).unwrap();
        assert!(root.join(".aura").is_dir());
        assert!(ledger_path.with_extension("transaction.lock").is_file());
        drop(lock);
        assert!(!ledger_path.with_extension("transaction.lock").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_migrates_result_only_format() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-legacy-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, br#"{"legacy-request":{"ok":true}}"#).unwrap();
        let ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger.replay("legacy-request"),
            Some(serde_json::json!({"ok": true}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ledger_lock_drop_does_not_remove_a_replaced_owner() {
        let root = std::env::temp_dir().join(format!("aura-ledger-owner-{}", std::process::id()));
        let path = root.join("requests.json");
        std::fs::create_dir_all(&root).unwrap();
        let lock_path = path.with_extension("lock");
        let lock = LedgerLock::acquire(&path).unwrap();
        std::fs::write(&lock_path, "pid=999 nonce=replaced-owner\n").unwrap();
        drop(lock);
        assert!(lock_path.exists());
        let _ = std::fs::remove_file(lock_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capabilities_advertise_all_history_cli_operations() {
        let advertised = capabilities();
        assert_eq!(
            advertised["midi_read_operations"],
            serde_json::json!(["inspect_midi_notes"])
        );
        let operations = advertised
            .get("operations")
            .and_then(serde_json::Value::as_array)
            .expect("capabilities must expose operations");
        for operation in [
            "project.load",
            "analyze_mix",
            "analyze_silence",
            "add_aux_track",
            "split_region_with_crossfade",
            "select_comp_take",
            "suggest_next_chords",
            "generate_arpeggio",
            "place_arpeggio",
            "history.status",
            "history.log",
            "history.diff",
            "history.commit",
            "history.branch",
            "history.checkout",
            "history.tag",
            "history.revert",
            "history.cherry_pick",
        ] {
            assert!(
                operations
                    .iter()
                    .any(|value| value.as_str() == Some(operation)),
                "missing advertised operation {operation}"
            );
        }
    }

    #[test]
    fn capabilities_do_not_claim_unverified_industry_integrations() {
        let advertised = capabilities();
        let integrations = advertised
            .get("integration_capabilities")
            .expect("integration capability matrix must be public");
        assert_eq!(integrations["ara2"]["verified"], false);
        assert_eq!(integrations["ara2"]["plugin_protocol_bridge"], true);
        assert_eq!(
            integrations["hardware_controllers"]["device_driver_integration"],
            false
        );
        assert_eq!(integrations["immersive_audio"]["dolby_renderer"], false);
        assert_eq!(integrations["immersive_audio"]["metadata_export"], false);
    }

    #[test]
    fn capabilities_advertise_distinct_pause_semantics() {
        let value = capabilities();
        assert_eq!(value["transport_capabilities"]["pause"], true);
        assert_eq!(
            value["transport_capabilities"]["pause_preserves_playhead"],
            true
        );
    }

    #[test]
    fn capabilities_expose_computer_use_power_profile_without_bypassing_audit() {
        let advertised = capabilities();
        let computer_use = &advertised["clients"]["computer_use"];
        assert_eq!(computer_use["supported"], true);
        assert_eq!(computer_use["power_profile"], "unrestricted_explicit");
        assert!(computer_use["requires"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "audit_log"));
        assert_eq!(advertised["safety"]["unknown_operations_rejected"], true);
    }

    #[test]
    fn capabilities_match_external_codec_support() {
        let advertised = capabilities();
        let render = advertised
            .get("render_capabilities")
            .expect("render capabilities must be present");
        let supported = render
            .get("supported_codecs")
            .and_then(serde_json::Value::as_array)
            .expect("supported codecs must be an array");
        let unsupported = render
            .get("unsupported_codecs")
            .and_then(serde_json::Value::as_array)
            .expect("unsupported codecs must be an array");
        for codec in ["aiff_pcm16", "flac", "mp3"] {
            assert!(supported.iter().any(|value| value.as_str() == Some(codec)));
            assert!(!unsupported
                .iter()
                .any(|value| value.as_str() == Some(codec)));
        }
        assert_eq!(
            render
                .get("external_codec_provider")
                .and_then(serde_json::Value::as_str),
            Some("ffmpeg")
        );
        let imports = render
            .get("supported_import_formats")
            .and_then(serde_json::Value::as_array)
            .expect("supported import formats must be an array");
        for format in ["wav", "mp3", "flac", "aiff", "m4a", "ogg", "aac"] {
            assert!(imports.iter().any(|value| value.as_str() == Some(format)));
        }
        let operations = advertised
            .get("operations")
            .and_then(serde_json::Value::as_array)
            .expect("operations must be an array");
        for operation in [
            "plugin_search",
            "set_plugin_favorite",
            "add_vca_group",
            "assign_track_to_vca",
            "set_vca_group_gain",
        ] {
            assert!(operations
                .iter()
                .any(|value| value.as_str() == Some(operation)));
        }
    }
}

