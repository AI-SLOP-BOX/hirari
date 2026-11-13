// Split out of lib_tests_rendering.rs.

#[test]
fn sidecar_failure_rolls_back_native_project_state() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let unique = format!(
        "hirari-sidecar-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{unique}.hirari"));
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
fn ui_midi_sidecar_failure_rolls_back_the_project_save() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let unique = format!(
        "hirari-ui-midi-save-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{unique}.hirari"));
    let project_text = project.to_str().unwrap();
    let ui_sidecar = std::path::PathBuf::from(format!("{project_text}.midi.json"));

    let saved_track = core.add_track(0);
    assert_ne!(saved_track, 0);
    assert!(core.set_track_name(saved_track, "saved-state"));
    assert!(core.save_project(project_text));
    let previous_project = std::fs::read(&project).unwrap();

    let current_track = core.add_track(0);
    assert_ne!(current_track, 0);
    assert!(core.set_track_name(current_track, "current-state"));
    std::fs::create_dir(&ui_sidecar).unwrap();

    assert!(!core.save_project_with_ui_midi_notes(project_text, "[]"));
    assert_eq!(std::fs::read(&project).unwrap(), previous_project);
    let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json()).unwrap();
    assert_eq!(layout.as_array().unwrap().len(), 2);
    assert_eq!(layout[1]["name"], "current-state");

    let _ = std::fs::remove_dir_all(&ui_sidecar);
    assert!(core.save_project_with_ui_midi_notes(project_text, "[]"));
    assert_eq!(std::fs::read_to_string(&ui_sidecar).unwrap(), "[]");
    assert!(core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":2}]"#));
    assert!(core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":3}]"#));
    let backup_one: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{project_text}.midi.json.bak.1")).unwrap())
            .unwrap();
    let backup_two: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{project_text}.midi.json.bak.2")).unwrap())
            .unwrap();
    assert_eq!(backup_one["notes"], serde_json::json!([{"generation":2}]));
    assert!(backup_one["hirari_project_checksum"].as_u64().is_some());
    assert!(backup_one["hirari_project_modified_ns"].as_str().is_some());
    assert_eq!(backup_two["notes"], serde_json::json!([]));
    assert!(backup_two["hirari_project_checksum"].as_u64().is_some());
    assert!(backup_two["hirari_project_modified_ns"].as_str().is_some());

    let current_sidecar = std::fs::read(&ui_sidecar).unwrap();
    let comping_sidecar = std::path::PathBuf::from(format!("{project_text}.comping.json"));
    std::fs::remove_file(&comping_sidecar).unwrap();
    std::fs::create_dir(&comping_sidecar).unwrap();
    assert!(!core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":4}]"#));
    assert_eq!(std::fs::read(&ui_sidecar).unwrap(), current_sidecar);
    assert!(serde_json::from_slice::<Vec<serde_json::Value>>(&current_sidecar).is_ok());
    std::fs::remove_dir_all(&comping_sidecar).unwrap();

    let _ = std::fs::remove_file(&ui_sidecar);
    let _ = std::fs::remove_file(&project);
    for suffix in [
        ".bak.1",
        ".bak.2",
        ".bak.3",
        ".comping.json",
        ".comping.json.bak.1",
        ".comping.json.bak.2",
        ".comping.json.bak.3",
        ".midi-events.json",
        ".midi-events.json.bak.1",
        ".midi-events.json.bak.2",
        ".midi-events.json.bak.3",
        ".midi.json",
        ".midi.json.bak.1",
        ".midi.json.bak.2",
        ".midi.json.bak.3",
    ] {
        let _ = std::fs::remove_file(format!("{project_text}{suffix}"));
    }
}

#[test]
fn test_non_finite_tempo_input_is_ignored() {
    let _guard = tempo_test_guard();
    let _engine_guard = native_engine_test_guard();
    let engine = crate::new_audio_engine();
    assert!(!false);
    let engine_ref = &*engine;
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
    let engine = crate::new_audio_engine();
    assert!(!false);
    let engine_ref = &*engine;

    engine_ref.set_tempo(0.0);
    assert_eq!(engine_ref.get_tempo(), 20.0);
    engine_ref.set_tempo(301.0);
    assert_eq!(engine_ref.get_tempo(), 300.0);
}

#[test]
fn project_diagnostic_api_preserves_structured_error_codes() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let empty: serde_json::Value = serde_json::from_str(&core.save_project_diagnostic_json(" "))
        .expect("empty path diagnostic must be JSON");
    assert_eq!(empty["code"], "invalid_path");

    let missing = std::env::temp_dir().join(format!(
        "hirari-diagnostic-missing-{}-{}.hirari",
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
        "hirari-diagnostic-save-{}-{}.hirari",
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
fn project_load_rollback_failure_has_a_distinct_diagnostic_code() {
    use crate::{project_load_failure_code, project_load_rollback_complete, ProjectLoadFailure};

    assert!(!project_load_rollback_complete(false, true, true));
    assert!(!project_load_rollback_complete(true, false, true));
    assert!(!project_load_rollback_complete(true, true, false));
    assert!(project_load_rollback_complete(true, true, true));
    assert_eq!(
        project_load_failure_code(ProjectLoadFailure::RollbackFailed),
        "project_load_rollback_failed"
    );
}

#[test]
fn recording_and_midi_diagnostics_reject_invalid_commands_structurally() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");

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

    let invalid_event: serde_json::Value =
        serde_json::from_str(&core.set_midi_events_diagnostic_json(
            r#"[{"beat":0.0,"channel":16,"kind":{"ControlChange":{"controller":1,"value":1}}}]"#,
        ))
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
    let core = crate::HirariCore::new().expect("core must initialize");

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
    let core = crate::HirariCore::new().expect("core must initialize");
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

