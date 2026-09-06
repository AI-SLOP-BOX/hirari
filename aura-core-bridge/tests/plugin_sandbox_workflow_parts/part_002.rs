fn minimal_clap_fixture_keeps_multiple_instances_isolated() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let first_track = core.add_track(0);
    let second_track = core.add_track(0);
    assert!(core.add_sandboxed_plugin(first_track, &fixture));
    assert!(core.add_sandboxed_plugin(second_track, &fixture));

    let first_gain = 0.25_f32.to_le_bytes();
    let second_gain = 0.75_f32.to_le_bytes();
    assert!(core.set_sandbox_plugin_state(first_track, 0, &first_gain));
    assert!(core.set_sandbox_plugin_state(second_track, 0, &second_gain));
    assert_eq!(core.sandbox_plugin_state(first_track, 0), first_gain);
    assert_eq!(core.sandbox_plugin_state(second_track, 0), second_gain);
    assert!(core.restart_sandboxed_plugin(first_track, 0));
    assert!(core.restart_sandboxed_plugin(second_track, 0));

    for _block in 0..16u32 {
        let first_input = vec![1.0_f32; 32];
        let second_input = vec![1.0_f32; 32];
        let mut first_left = first_input.clone();
        let mut first_right = first_input.clone();
        let mut second_left = second_input.clone();
        let mut second_right = second_input.clone();
        wait_for_audio_block(
            &core,
            first_track,
            &first_input,
            &first_input,
            &mut first_left,
            &mut first_right,
        );
        wait_for_audio_block(
            &core,
            second_track,
            &second_input,
            &second_input,
            &mut second_left,
            &mut second_right,
        );
        let expected_first = first_input[0] * 0.25;
        let expected_second = second_input[0] * 0.75;
        assert!(
            first_left
                .iter()
                .all(|sample| (*sample - expected_first).abs() < 1.0e-5),
            "first instance leaked state or block data: actual={:?} expected={expected_first}",
            first_left
        );
        assert!(
            second_left
                .iter()
                .all(|sample| (*sample - expected_second).abs() < 1.0e-5),
            "second instance leaked state or block data: actual={:?} expected={expected_second}",
            second_left
        );
    }
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn two_independent_sandbox_hosts_interleave_without_cross_talk() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    assert!(
        std::path::Path::new(&fixture).is_file(),
        "CLAP fixture must remain available before the second-host test: {fixture}"
    );
    if let Ok(worker) = std::env::var("AURA_PLUGIN_HOST_BIN") {
        assert!(
            std::path::Path::new(&worker).is_file(),
            "sandbox worker must remain available before the second-host test: {worker}"
        );
    }
    let first = AuraCore::new().expect("first core must initialize");
    let second = AuraCore::new().expect("second core must initialize");
    assert!(first.apply_audio_config(48_000, 128));
    assert!(second.apply_audio_config(48_000, 128));
    let first_track = first.add_track(0);
    let second_track = second.add_track(0);
    assert_ne!(first_track, 0, "first track allocation failed");
    assert_ne!(second_track, 0, "second track allocation failed");
    assert!(
        first.add_sandboxed_plugin(first_track, &fixture),
        "first sandbox admission failed: diagnostic={} failure={}",
        first.add_sandboxed_plugin_diagnostic_json(first_track, &fixture),
        first.last_sandbox_failure_text(first_track)
    );
    assert!(
        second.add_sandboxed_plugin(second_track, &fixture),
        "second sandbox admission failed: {}",
        second.add_sandboxed_plugin_diagnostic_json(second_track, &fixture)
    );

    let first_gain = 0.25_f32.to_le_bytes();
    let second_gain = 0.75_f32.to_le_bytes();
    assert!(first.set_sandbox_plugin_state(first_track, 0, &first_gain));
    assert!(second.set_sandbox_plugin_state(second_track, 0, &second_gain));

    for _ in 0..16 {
        let first_input = vec![1.0_f32; 32];
        let second_input = vec![1.0_f32; 32];
        let mut first_left = first_input.clone();
        let mut first_right = first_input.clone();
        let mut second_left = second_input.clone();
        let mut second_right = second_input.clone();
        wait_for_audio_block(
            &first,
            first_track,
            &first_input,
            &first_input,
            &mut first_left,
            &mut first_right,
        );
        wait_for_audio_block(
            &second,
            second_track,
            &second_input,
            &second_input,
            &mut second_left,
            &mut second_right,
        );
        assert!(first_left
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1.0e-5));
        assert!(second_left
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 1.0e-5));
    }

    assert!(first
        .sandbox_snapshots()
        .into_iter()
        .all(|snapshot| snapshot.alive));
    assert!(second
        .sandbox_snapshots()
        .into_iter()
        .all(|snapshot| snapshot.alive));
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn minimal_clap_fixture_state_survives_project_v2_reload() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let project_path = std::env::temp_dir().join(format!(
        "aura-v2-sandbox-state-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));

    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );
    let restored_gain = 0.25_f32.to_le_bytes();
    assert!(core.set_sandbox_plugin_state(track_id, 0, &restored_gain));
    assert_eq!(core.sandbox_plugin_state(track_id, 0), restored_gain);
    core.save_project_v2(
        project_path.to_str().expect("project path must be UTF-8"),
        "Sandbox State",
        120.0,
    )
    .expect("sandbox project save must succeed");
    // The compatibility native engine is process-owned. Release the first
    // session before opening the persisted project so this test proves a
    // save/close/reopen lifecycle rather than accidentally requiring two
    // native singleton owners at once.
    drop(core);

    let restored = AuraCore::new().expect("restored core must initialize");
    restored
        .load_project_v2(project_path.to_str().expect("project path must be UTF-8"))
        .expect("sandbox project load must succeed");

    let layout: serde_json::Value = serde_json::from_str(&restored.get_project_layout_json())
        .expect("restored layout must be JSON");
    let restored_track = layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .expect("restored track must exist");
    assert_eq!(restored_track["sandbox_plugin_paths"][0], fixture.as_str());
    assert_eq!(restored_track["sandbox_plugin_state_hex"][0], "0000803e");
    let restored_track_id = restored_track["id"]
        .as_u64()
        .expect("restored track id must be numeric") as u32;
    assert_eq!(
        restored.sandbox_plugin_state(restored_track_id, 0),
        restored_gain
    );
    assert!(restored
        .sandbox_snapshots()
        .iter()
        .any(|snapshot| snapshot.alive && snapshot.failure == 0));

    let _ = std::fs::remove_file(project_path);
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn minimal_clap_fixture_project_reload_preserves_audio_output() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let project_path = std::env::temp_dir().join(format!(
        "aura-v2-sandbox-audio-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));

    let input_left = vec![1.0_f32; 4];
    let input_right = vec![-1.0_f32; 4];
    let restored_gain = 0.25_f32.to_le_bytes();

    let before_reload = {
        let core = AuraCore::new().expect("core must initialize");
        assert!(core.apply_audio_config(48_000, 128));
        let track_id = core.add_track(0);
        assert!(
            core.add_sandboxed_plugin(track_id, &fixture),
            "sandbox admission failed: {}",
            core.last_sandbox_failure_text(track_id)
        );
        assert!(core.set_sandbox_plugin_state(track_id, 0, &restored_gain));
        assert!(core.restart_sandboxed_plugin(track_id, 0));

        let mut submitted_left = input_left.clone();
        let mut submitted_right = input_right.clone();
        assert!(!core.process_sandboxed_plugin_block(
            track_id,
            0,
            &mut submitted_left,
            &mut submitted_right
        ));
        let mut output_left = vec![0.0_f32; input_left.len()];
        let mut output_right = vec![0.0_f32; input_right.len()];
        wait_for_audio_block(
            &core,
            track_id,
            &input_left,
            &input_right,
            &mut output_left,
            &mut output_right,
        );
        assert!(output_left
            .iter()
            .chain(output_right.iter())
            .all(|sample| sample.is_finite()));
        assert!(output_left
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1.0e-5));
        assert!(output_right
            .iter()
            .all(|sample| (*sample + 0.25).abs() < 1.0e-5));

        core.save_project_v2(
            project_path.to_str().expect("project path must be UTF-8"),
            "Sandbox Audio State",
            120.0,
        )
        .expect("sandbox project save must succeed");
        (output_left, output_right)
    };

    let after_reload = {
        let restored = AuraCore::new().expect("restored core must initialize");
        restored
            .load_project_v2(project_path.to_str().expect("project path must be UTF-8"))
            .expect("sandbox project load must succeed");
        let layout: serde_json::Value = serde_json::from_str(&restored.get_project_layout_json())
            .expect("restored layout must be JSON");
        let restored_track_id = layout
            .as_array()
            .and_then(|tracks| tracks.first())
            .and_then(|track| track["id"].as_u64())
            .expect("restored track id must be numeric") as u32;
        assert_eq!(
            restored.sandbox_plugin_state(restored_track_id, 0),
            restored_gain
        );

        let mut submitted_left = input_left.clone();
        let mut submitted_right = input_right.clone();
        assert!(!restored.process_sandboxed_plugin_block(
            restored_track_id,
            0,
            &mut submitted_left,
            &mut submitted_right
        ));
        let mut output_left = vec![0.0_f32; input_left.len()];
        let mut output_right = vec![0.0_f32; input_right.len()];
        wait_for_audio_block(
            &restored,
            restored_track_id,
            &input_left,
            &input_right,
            &mut output_left,
            &mut output_right,
        );
        (output_left, output_right)
    };

    assert_eq!(before_reload.0, after_reload.0);
    assert_eq!(before_reload.1, after_reload.1);
    let _ = std::fs::remove_file(project_path);
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn failed_sandbox_project_hydration_rolls_back_the_previous_native_graph() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let project_path = std::env::temp_dir().join(format!(
        "aura-v2-sandbox-rollback-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));

    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.set_track_name(track_id, "Before Failed Hydration"));
    assert!(core.add_sandboxed_plugin(track_id, &fixture));
    let state = 0.5_f32.to_le_bytes();
    assert!(core.set_sandbox_plugin_state(track_id, 0, &state));
    let before = core.get_project_layout_json();
    core.save_project_v2(
        project_path.to_str().expect("project path must be UTF-8"),
        "Rollback",
        120.0,
    )
    .expect("baseline project save must succeed");

    let mut invalid: serde_json::Value =
        serde_json::from_str(&before).expect("baseline layout must be JSON");
    invalid[0]["sandbox_plugin_paths"][0] = serde_json::json!(std::env::temp_dir()
        .join(format!("aura-missing-plugin-{}.clap", std::process::id()))
        .to_string_lossy()
        .to_string());
    let document = serde_json::json!({
        "schema_version": 1,
        "metadata": {"name": "Rollback", "version": 1, "bpm": 120.0, "tracks_count": 1},
        "sample_rate": 48000.0,
        "tracks": [{
            "id": invalid[0]["id"],
            "name": invalid[0]["name"],
            "track_type": "Audio",
            "volume": 1.0,
            "pan": 0.0,
            "muted": false,
            "solo": false,
            "plugin_types": [u32::MAX],
            "plugin_states": [[]],
            "sandbox_plugin_paths": [invalid[0]["sandbox_plugin_paths"][0]],
            "sandbox_plugin_states": [[0, 0, 0, 63]]
        }],
        "regions": []
    });
    std::fs::write(&project_path, serde_json::to_vec(&document).unwrap())
        .expect("invalid project fixture must be writable");

    assert!(core
        .load_project_v2(project_path.to_str().expect("project path must be UTF-8"))
        .is_err());
    assert_eq!(core.get_project_layout_json(), before);
    assert_eq!(core.sandbox_plugin_state(track_id, 0), state);
    assert!(core
        .sandbox_snapshots()
        .iter()
        .any(|snapshot| snapshot.alive && snapshot.failure == 0));

    let _ = std::fs::remove_file(project_path);
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn crashing_clap_worker_is_restarted_then_quarantined() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    std::env::set_var("AURA_CLAP_FIXTURE_CRASH", "1");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_sandboxed_plugin(track_id, &fixture));

    let mut left = vec![1.0_f32; 4];
    let mut right = vec![1.0_f32; 4];
    let mut quarantined = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let _ = core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right);
        assert!(
            left.iter()
                .chain(right.iter())
                .all(|sample| sample.is_finite()),
            "sandbox failure must never publish NaN or infinity"
        );
        // Poll the lifecycle state instead of relying on a fixed sleep.  The
        // worker may exit immediately or only after the OS schedules it.
        for _ in 0..200 {
            let _ = core.maintain_sandboxes(true);
            assert!(
                left.iter()
                    .chain(right.iter())
                    .all(|sample| sample.is_finite()),
                "restart polling must preserve finite fallback audio"
            );
            if let Some(snapshot) = core.sandbox_snapshots().first() {
                if !snapshot.alive && snapshot.failure != 0 && snapshot.can_retry {
                    quarantined = true;
                    break;
                }
            }
            std::thread::yield_now();
        }
        if quarantined {
            break;
        }
    }
    std::env::remove_var("AURA_CLAP_FIXTURE_CRASH");
    assert!(
        quarantined,
        "repeated worker crashes must end in quarantine"
    );
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn generic_worker_fault_injection_quarantines_clap() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    std::env::set_var("AURA_PLUGIN_TEST_FAULTS", "1");
    std::env::set_var("AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS", "2");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_sandboxed_plugin(track_id, &fixture));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut quarantined = false;
    while std::time::Instant::now() < deadline {
        let mut left = vec![0.25_f32; 32];
        let mut right = vec![0.25_f32; 32];
        let _ = core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        for _ in 0..200 {
            let _ = core.maintain_sandboxes(true);
            if core
                .sandbox_statuses()
                .iter()
                .any(|status| !status.alive && status.failure != 0 && status.can_retry)
            {
                quarantined = true;
                break;
            }
            std::thread::yield_now();
        }
        if quarantined {
            break;
        }
    }
    std::env::remove_var("AURA_PLUGIN_TEST_FAULTS");
    std::env::remove_var("AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS");
    assert!(quarantined, "generic worker fault must end in quarantine");
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
