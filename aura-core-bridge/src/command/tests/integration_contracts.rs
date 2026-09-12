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
        ] {
            assert!(
                operations
                    .iter()
                    .any(|value| value.as_str() == Some(operation)),
                "missing advertised operation {operation}"
            );
        }
        let history_operations = advertised
            .get("history_operations")
            .and_then(serde_json::Value::as_array)
            .expect("history operations must be exposed separately");
        for operation in [
            "history.status", "history.log", "history.diff", "history.commit",
            "history.branch", "history.checkout", "history.tag", "history.revert",
            "history.cherry_pick",
        ] {
            assert!(history_operations
                .iter()
                .any(|value| value.as_str() == Some(operation)));
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
