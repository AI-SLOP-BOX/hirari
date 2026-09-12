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
