use aura_core_bridge::AuraCore;
mod support {
    include!("recording_support.rs");
}
use support::native_engine_test_guard;

#[test]
fn project_layout_round_trips_through_public_core_api() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.set_track_name(track_id, "Roundtrip Track"));
    core.set_track_fader(track_id, 0.72);
    core.set_track_pan(track_id, -0.18);

    let path = std::env::temp_dir().join(format!(
        "aura-project-roundtrip-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    let path_text = path
        .to_str()
        .expect("temporary path must be UTF-8")
        .to_owned();

    let before = core.get_project_layout_json();
    assert!(before.contains("Roundtrip Track"));
    assert!(core.save_project(&path_text));
    assert!(core.load_project(&path_text));
    let after = core.get_project_layout_json();

    assert!(after.contains("Roundtrip Track"));
    let before_track = serde_json::from_str::<serde_json::Value>(&before)
        .expect("before layout must be JSON")
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .cloned()
        .expect("created track must be present before reload");
    let after_track = serde_json::from_str::<serde_json::Value>(&after)
        .expect("after layout must be JSON")
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .cloned()
        .expect("created track must be present after reload");
    assert_eq!(before_track, after_track);
    assert!(core.remove_track(track_id));
    let _ = std::fs::remove_file(path);
}

#[test]
fn control_room_state_round_trips_through_project_document_api() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.add_control_room_output("Nearfield"));
    assert!(core.select_control_room_output(1));
    assert!(core.set_control_room_speaker_gain(1, 0.8));
    assert!(core.set_control_room_speaker_enabled(1, false));
    core.set_control_room_dim(true);
    assert!(core.upsert_control_room_cue(7, 0.75, true));
    assert!(core.set_control_room_reference_track(Some("reference.wav".into())));
    let path = std::env::temp_dir().join(format!(
        "aura-control-room-roundtrip-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_text = path.to_str().unwrap().to_owned();
    core.save_project_v2(&path_text, "Control Room", 120.0)
        .unwrap();
    let restored = AuraCore::new().expect("restored core must initialize");
    restored.load_project_v2(&path_text).unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_str(&restored.control_room_monitor_snapshot_json()).unwrap();
    assert_eq!(snapshot["active_output"], "Nearfield");
    assert!((snapshot["active_output_gain"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(snapshot["active_output_enabled"], false);
    assert_eq!(snapshot["dim"], true);
    assert_eq!(snapshot["reference_track"], "reference.wav");
    let state: serde_json::Value = serde_json::from_str(&restored.control_room_json()).unwrap();
    assert_eq!(state["cues"][0]["id"], 7);
    assert_eq!(state["cues"][0]["gain"], 0.75);
    let _ = std::fs::remove_file(path);
}

#[test]
fn reload_reseeds_track_allocator_without_reusing_persisted_ids() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let first = core.add_track(0);
    let second = core.add_track(0);
    let path = std::env::temp_dir().join(format!(
        "aura-id-reseed-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    let path_text = path.to_str().expect("temporary path must be UTF-8");
    assert!(core.save_project(path_text));
    assert!(core.load_project(path_text));

    let restored = core.add_track(0);
    assert!(restored > first && restored > second);
    assert_ne!(restored, first);
    assert_ne!(restored, second);
    let _ = std::fs::remove_file(path);
}

#[test]
fn template_first_track_id_zero_survives_save_and_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    core.new_project();
    assert!(core.set_track_name(0, "Drums"));
    for name in ["Bass", "Synth Lead", "Pads", "Main Out"] {
        let id = core.add_track(0);
        assert_ne!(id, 0);
        assert!(core.set_track_name(id, name));
    }

    let path = std::env::temp_dir().join(format!(
        "aura-template-zero-id-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    let path_text = path.to_str().expect("temporary path must be UTF-8");
    assert!(core.save_project(path_text));
    assert!(core.load_project(path_text));
    let layout = core.get_project_layout_json();
    assert!(layout.contains("Drums"));
    assert!(layout.contains("Main Out"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn selected_recovery_generation_restores_the_requested_project_state() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let path = std::env::temp_dir().join(format!(
        "aura-project-generation-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    let path_text = path.to_str().expect("temporary path must be UTF-8");

    let first_track = core.add_track(0);
    assert!(core.set_track_name(first_track, "Generation One"));
    assert!(core.save_project(path_text));

    let second_track = core.add_track(0);
    assert!(core.set_track_name(second_track, "Generation Two"));
    assert!(core.save_project(path_text));

    let candidates: serde_json::Value =
        serde_json::from_str(&core.recovery_candidates_json(path_text))
            .expect("recovery candidates must be JSON");
    let first_candidate = candidates
        .as_array()
        .and_then(|items| items.first())
        .expect("a recovery candidate must be present");
    assert!(first_candidate
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|bytes| bytes > 0));
    assert!(first_candidate
        .get("modified_unix_seconds")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|timestamp| timestamp > 0));
    assert!(first_candidate
        .get("checksum")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|checksum| checksum > 0));
    let generation = candidates
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("generation"))
        .and_then(serde_json::Value::as_u64)
        .expect("a first recovery generation must exist") as u32;
    assert!(core.restore_project_backup(path_text, generation));

    let restored = core.get_project_layout_json();
    assert!(restored.contains("Generation One"));
    assert!(!restored.contains("Generation Two"));

    // Restoring generation one legitimately removes the second track, so cleanup
    // must tolerate an already-absent entity in the process-wide test engine.
    let _ = core.remove_track(second_track);
    let _ = core.remove_track(first_track);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}.bak.1", path.display()));
    let _ = std::fs::remove_file(format!("{}.bak.2", path.display()));
}

#[test]
fn corrupted_recovery_backup_is_rejected_without_loading_partial_state() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let path = std::env::temp_dir().join(format!(
        "aura-project-corrupt-backup-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    let path_text = path.to_str().expect("temporary path must be UTF-8");
    let track_id = core.add_track(0);
    assert!(core.set_track_name(track_id, "Healthy Project"));
    assert!(core.save_project(path_text));
    assert!(core.set_track_name(track_id, "Newer Project"));
    assert!(core.save_project(path_text));

    let backup = format!("{}.bak.1", path.display());
    std::fs::write(&backup, b"corrupted project payload").expect("backup must be writable");
    assert!(!core.restore_project_backup(path_text, 1));
    assert!(core.get_project_layout_json().contains("Newer Project"));
    assert!(std::path::Path::new(&format!("{path_text}.before-restore")).is_file());

    let _ = core.remove_track(track_id);
    let rollback = format!("{path_text}.before-restore");
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(backup);
    let _ = std::fs::remove_file(rollback);
}
