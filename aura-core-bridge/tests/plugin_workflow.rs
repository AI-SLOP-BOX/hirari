use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn built_in_limiter_processes_audio_inside_the_track_render_path() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-plugin-dsp-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let without_plugin_path = std::env::temp_dir().join(format!("{token}-dry.wav"));
    let with_plugin_path = std::env::temp_dir().join(format!("{token}-wet.wav"));
    let threshold_path = std::env::temp_dir().join(format!("{token}-threshold.wav"));
    let reloaded_path = std::env::temp_dir().join(format!("{token}-reloaded.wav"));
    let preset_path = std::env::temp_dir().join(format!("{token}.aupreset"));

    assert!(core.apply_audio_config(48_000, 1024));
    core.start_recording_capture(48_000.0, 2, 4096, 0)
        .expect("recording capture must start");
    let hot_signal: Vec<f32> = (0..4096u32)
        .flat_map(|frame| {
            let sample = if frame % 16 < 8 { 0.8 } else { -0.8 };
            [sample, sample]
        })
        .collect();
    core.append_recording_preview(&hot_signal)
        .expect("recorded audio must be accepted");
    let track_id = core.add_track(0);
    assert!(
        core.commit_recording_capture_to_track(track_id, None)
            .expect("recording take must publish")
            > 0
    );

    assert!(core.bounce_project(
        without_plugin_path
            .to_str()
            .expect("dry render path must be UTF-8"),
        0
    ));
    let without_plugin = std::fs::read(&without_plugin_path).expect("dry render must exist");
    assert!(core.add_plugin(track_id, 0));
    let track_latency = core.get_track_latency_ms(track_id);
    let engine_latency = core.get_latency_ms();
    assert!(track_latency.is_finite() && track_latency >= 0.0);
    assert!(engine_latency.is_finite() && engine_latency >= 0.0);
    assert!(!core.get_plugin_bypass(track_id, 0));
    assert!(core.set_plugin_bypass(track_id, 0, true));
    assert!(core.get_plugin_bypass(track_id, 0));
    assert!(core.set_plugin_bypass(track_id, 0, false));
    assert!(!core.get_plugin_bypass(track_id, 0));
    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.5));
    assert!(core.bounce_project(
        with_plugin_path
            .to_str()
            .expect("wet render path must be UTF-8"),
        0
    ));
    let with_plugin = std::fs::read(&with_plugin_path).expect("wet render must exist");
    assert_ne!(
        without_plugin[44..],
        with_plugin[44..],
        "built-in limiter must alter rendered PCM"
    );
    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.1));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.1).abs() < 0.001);
    assert!(core.save_plugin_preset(track_id, 0, preset_path.to_str().unwrap()));
    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.8));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.8).abs() < 0.001);
    assert!(core.load_plugin_preset(track_id, 0, preset_path.to_str().unwrap()));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.1).abs() < 0.001);
    let mut corrupt_preset = std::fs::read(&preset_path).expect("preset must exist");
    let last = corrupt_preset
        .last_mut()
        .expect("preset must contain state");
    *last ^= 0x01;
    std::fs::write(&preset_path, corrupt_preset).expect("corrupt preset must be writable");
    assert!(!core.load_plugin_preset(track_id, 0, preset_path.to_str().unwrap()));
    assert!(core.bounce_project(
        threshold_path
            .to_str()
            .expect("threshold render path must be UTF-8"),
        0
    ));
    let threshold = std::fs::read(&threshold_path).expect("threshold render must exist");
    let wet_threshold_rms = pcm16_rms(&with_plugin);
    let threshold_rms = pcm16_rms(&threshold);
    assert!(wet_threshold_rms.is_finite() && threshold_rms.is_finite());
    assert!(
        threshold_rms > 0.0,
        "parameterized limiter render must contain audio"
    );
    let plugin_snapshot = |layout: &str| {
        let layout: serde_json::Value = serde_json::from_str(layout).expect("layout must be JSON");
        let track = layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .expect("saved track must exist");
        (
            track
                .get("plugin_types")
                .cloned()
                .expect("plugin types must exist"),
            track
                .get("plugin_state_sizes")
                .cloned()
                .expect("plugin state sizes must exist"),
            track
                .get("plugin_state_hashes")
                .cloned()
                .expect("plugin state hashes must exist"),
        )
    };
    let before_snapshot = plugin_snapshot(&core.get_project_layout_json());
    assert_eq!(before_snapshot.0, serde_json::json!([0]));
    assert_eq!(before_snapshot.1, serde_json::json!([4]));
    assert!(core.set_plugin_bypass(track_id, 0, true));
    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(core.load_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(
        core.get_plugin_bypass(track_id, 0),
        "plugin bypass must survive project reload"
    );
    let after_snapshot = plugin_snapshot(&core.get_project_layout_json());
    assert_eq!(
        after_snapshot, before_snapshot,
        "plugin state must survive save/load"
    );
    assert!(core.set_plugin_bypass(track_id, 0, false));
    assert!(core.bounce_project(
        reloaded_path.to_str().expect("reloaded path must be UTF-8"),
        0
    ));
    let reloaded = std::fs::read(&reloaded_path).expect("reloaded render must exist");
    let threshold_rms = pcm16_rms(&threshold);
    let reloaded_rms = pcm16_rms(&reloaded);
    assert!(
        (threshold_rms - reloaded_rms).abs() <= threshold_rms.max(1.0) * 0.02,
        "built-in plugin state must survive save/load (threshold={threshold_rms}, reloaded={reloaded_rms})"
    );

    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(without_plugin_path);
    let _ = std::fs::remove_file(with_plugin_path);
    let _ = std::fs::remove_file(threshold_path);
    let _ = std::fs::remove_file(reloaded_path);
    let _ = std::fs::remove_file(preset_path);
}

#[test]
fn rust_project_v2_round_trips_built_in_plugin_state() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_plugin(track_id, 0));
    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.37));
    let before: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be JSON");
    let state_hex_before = before[0]["plugin_state_hex"][0]
        .as_str()
        .expect("plugin state hex must be present")
        .to_owned();
    assert!(!state_hex_before.is_empty());

    let path = std::env::temp_dir().join(format!(
        "aura-v2-plugin-state-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    core.save_project_v2(
        path.to_str().expect("path must be UTF-8"),
        "Plugin State",
        120.0,
    )
    .expect("v2 save must succeed");

    let restored = AuraCore::new().expect("restored core must initialize");
    restored
        .load_project_v2(path.to_str().expect("path must be UTF-8"))
        .expect("v2 load must succeed");
    let after: serde_json::Value =
        serde_json::from_str(&restored.get_project_layout_json()).expect("layout must be JSON");
    assert_eq!(after[0]["plugin_types"], serde_json::json!([0]));
    assert_eq!(after[0]["plugin_state_hex"][0], state_hex_before);
    let _ = std::fs::remove_file(path);
}

#[test]
fn serial_plugin_chain_state_and_order_survive_project_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-plugin-chain-state-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{token}.aura"));
    let track = core.add_track(0);
    assert!(core.add_plugin(track, 0));
    assert!(core.add_plugin(track, 0));
    assert!(core.set_plugin_parameter(track, 0, 0, 0.23));
    assert!(core.set_plugin_parameter(track, 1, 0, 0.77));
    assert!(core.set_plugin_bypass(track, 1, true));
    let before_latency = core.get_track_latency_ms(track);
    assert!(core.save_project(project.to_str().unwrap()));

    assert!(core.remove_plugin(track, 1));
    assert!(core.set_plugin_parameter(track, 0, 0, 0.01));
    assert!(core.load_project(project.to_str().unwrap()));

    let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
        .expect("layout must remain valid JSON");
    let saved_track = layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|item| {
                item.get("id").and_then(serde_json::Value::as_u64) == Some(track as u64)
            })
        })
        .expect("track must survive reload");
    assert_eq!(
        saved_track.get("plugin_types"),
        Some(&serde_json::json!([0, 0]))
    );
    assert_eq!(
        saved_track.get("plugin_state_sizes"),
        Some(&serde_json::json!([4, 4]))
    );
    assert!((core.get_plugin_parameter(track, 0, 0) - 0.23).abs() < 0.001);
    assert!((core.get_plugin_parameter(track, 1, 0) - 0.77).abs() < 0.001);
    assert!(core.get_plugin_bypass(track, 1));
    assert!((core.get_track_latency_ms(track) - before_latency).abs() < 0.001);

    let _ = core.remove_track(track);
    let _ = std::fs::remove_file(project);
}
