#[test]
fn async_render_failure_returns_to_failed_and_can_be_retried() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-failed-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let missing_parent = std::env::temp_dir()
        .join(format!("{token}-missing"))
        .join("out.wav");
    let valid_output = std::env::temp_dir().join(format!("{token}.wav"));
    remove_test_file(&valid_output);

    assert!(core.start_render_async_to(missing_parent.to_str().unwrap()));
    let mut state = 0;
    let failed_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < failed_deadline {
        state = core
            .get_bounce_status()
            .map(|(value, _)| value)
            .unwrap_or(0);
        if state == 4 {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(state, 4, "failed render must publish Failed state");
    assert!(!missing_parent.exists());

    assert!(core.start_render_async_to(valid_output.to_str().unwrap()));
    let valid_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < valid_deadline {
        if core
            .get_bounce_status()
            .is_some_and(|(value, _)| value == 3 || value == 4)
        {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(core.get_bounce_status().map(|(value, _)| value), Some(3));
    assert!(valid_output.is_file());
    remove_test_file(&valid_output);
}

#[test]
fn malformed_project_sidecar_is_rejected_before_native_graph_load() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-malformed-sidecar-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{token}.aura"));
    let sidecar = std::path::PathBuf::from(format!("{}.midi-events.json", project.display()));
    let track = core.add_track(0);
    core.save_project_v2(project.to_str().unwrap(), "Relative Assets", 120.0)
        .expect("v2 project sidecar must save");
    core.set_volume(track, 0.25);
    let before_failed_load = core.get_project_layout_json();
    std::fs::write(&sidecar, "{broken").expect("test sidecar must be writable");
    assert!(!core.load_project(project.to_str().unwrap()));
    assert_eq!(core.get_project_layout_json(), before_failed_load);
    let _ = std::fs::remove_file(project);
    let _ = std::fs::remove_file(sidecar);
}

#[test]
fn native_save_exposes_and_restores_a_recovery_generation() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-recovery-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));

    let first_track = core.add_track(0);
    assert!(core.set_track_name(first_track, "Recovery Source"));
    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));

    let second_track = core.add_track(0);
    assert!(core.set_track_name(second_track, "Recovery Candidate"));
    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));

    let candidates: serde_json::Value = serde_json::from_str(
        &core.recovery_candidates_json(project_path.to_str().expect("project path must be UTF-8")),
    )
    .expect("recovery candidates must be JSON");
    assert!(candidates.as_array().is_some_and(|items| !items.is_empty()));
    assert!(core.restore_project_backup(
        project_path.to_str().expect("project path must be UTF-8"),
        1
    ));
    let restored = core.get_project_layout_json();
    assert!(restored.contains("Recovery Source"));
    assert!(!restored.contains("Recovery Candidate"));

    let _ = std::fs::remove_file(&project_path);
    for generation in 1..=10 {
        let backup =
            std::path::PathBuf::from(format!("{}.bak.{}", project_path.display(), generation));
        let _ = std::fs::remove_file(backup);
    }
}

#[test]
fn mixer_fader_mute_and_phase_reach_rendered_audio() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1_024));
    core.start_recording_capture(48_000.0, 2, 4_096, 0)
        .expect("capture must start");
    let input = (0..4_096usize)
        .flat_map(|frame| {
            let sample = if frame % 32 < 16 { 0.35 } else { -0.15 };
            [sample, sample]
        })
        .collect::<Vec<_>>();
    core.append_recording_preview(&input)
        .expect("capture must accept audio");
    let track_id = core.add_track(0);
    core.commit_recording_capture_to_track(track_id, None)
        .expect("capture must publish");

    let token = format!(
        "aura-mixer-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let baseline_path = std::env::temp_dir().join(format!("{token}-baseline.wav"));
    let fader_path = std::env::temp_dir().join(format!("{token}-fader.wav"));
    let muted_path = std::env::temp_dir().join(format!("{token}-muted.wav"));
    let phase_path = std::env::temp_dir().join(format!("{token}-phase.wav"));

    assert!(core.bounce_project(baseline_path.to_str().unwrap(), 0));
    let baseline = std::fs::read(&baseline_path).expect("baseline render must exist");
    let baseline_rms = pcm16_rms(&baseline);
    assert!(
        baseline[44..].iter().any(|sample| *sample != 0),
        "baseline must contain audio bytes"
    );
    assert!(baseline_rms > 0.0, "baseline must contain audible samples");

    // Mixer callbacks enqueue realtime-safe commands. Give the audio command
    // queue one callback interval before asking the offline renderer to read it.
    assert!(
        core.set_volume(track_id, 0.25),
        "volume command must be accepted for track {track_id}"
    );
    assert_eq!(core.track_volume(track_id), Some(0.25));
    wait_for_track_layout(&core, track_id, |track| {
        track
            .get("volume")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|value| (value - 0.25).abs() < 0.0001)
    });
    let mixer_state: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
        .expect("mixer state must be valid JSON");
    let rendered_volume = mixer_state
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("volume"))
        .and_then(serde_json::Value::as_f64)
        .expect("mixer volume must be serialized");
    assert!((rendered_volume - 0.25).abs() < 0.0001);
    assert!(core.bounce_project(fader_path.to_str().unwrap(), 0));
    let fader = std::fs::read(&fader_path).expect("fader render must exist");
    let fader_rms = pcm16_rms(&fader);
    assert!(fader_rms > 0.0);
    assert_ne!(baseline[44..], fader[44..], "fader must alter rendered PCM");

    core.set_mute(track_id, true);
    wait_for_track_layout(&core, track_id, |track| {
        track.get("mute").and_then(serde_json::Value::as_bool) == Some(true)
    });
    let muted_state: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
        .expect("muted state must be valid JSON");
    let muted = muted_state
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("mute"))
        .and_then(serde_json::Value::as_bool)
        .expect("mute state must be serialized");
    assert!(muted);
    assert!(core.bounce_project(muted_path.to_str().unwrap(), 0));
    let muted_render = std::fs::read(&muted_path).expect("muted render must exist");
    assert!(muted_render.len() > 44);

    core.set_mute(track_id, false);
    core.set_volume(track_id, 1.0);
    assert!(core.set_phase_invert(track_id, true));
    wait_for_track_layout(&core, track_id, |track| {
        track
            .get("phase_invert")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    });
    assert!(core.bounce_project(phase_path.to_str().unwrap(), 0));
    let phase = std::fs::read(&phase_path).expect("phase render must exist");
    let baseline_samples = pcm16_samples(&baseline);
    let phase_samples = pcm16_samples(&phase);
    let compared = baseline_samples
        .iter()
        .zip(phase_samples.iter())
        .filter(|(before, after)| **before != 0 && **after != 0)
        .take(512)
        .collect::<Vec<_>>();
    assert!(!compared.is_empty(), "phase render must contain samples");
    let inverted = compared
        .iter()
        .filter(|(before, after)| ((**before as i32) * (**after as i32)) < 0);
    assert!(
        inverted.count() * 100 / compared.len() > 90,
        "phase inversion must invert the rendered waveform"
    );

    remove_test_file(baseline_path);
    remove_test_file(fader_path);
    remove_test_file(muted_path);
    remove_test_file(phase_path);
}

#[test]
fn project_relative_audio_assets_survive_move_and_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(44_100, 256));
    core.start_recording_capture(44_100.0, 2, 2_048, 0)
        .expect("capture must start");
    let audio = (0..2_048usize)
        .flat_map(|frame| {
            let sample = if frame % 32 < 16 { 0.22 } else { -0.12 };
            [sample, sample]
        })
        .collect::<Vec<_>>();
    core.append_recording_preview(&audio)
        .expect("capture must accept audio");
    let track_id = core.add_track(0);
    core.commit_recording_capture_to_track(track_id, None)
        .expect("capture must publish");

    let token = format!(
        "aura-relative-region-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{token}.aura"));
    let moved_project = std::env::temp_dir().join(format!("{token}-moved"));
    std::fs::create_dir_all(&moved_project).expect("moved project directory must exist");
    let output = moved_project.join("relative.wav");
    core.save_project_v2(project.to_str().unwrap(), "Relative Assets", 120.0)
        .expect("v2 project sidecar must save");

    let mut document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&project).expect("project sidecar must be readable"),
    )
    .expect("project sidecar must be JSON");
    let original_path = document["regions"][0]["path"]
        .as_str()
        .expect("recorded region path must be serialized")
        .to_owned();
    let assets = moved_project.join("assets");
    std::fs::create_dir_all(&assets).expect("asset directory must exist");
    std::fs::copy(&original_path, assets.join("take.wav")).expect("asset must be copied");
    document["regions"][0]["path"] = serde_json::Value::String("assets/take.wav".into());
    let moved_sidecar = moved_project.join("moved.aura");
    std::fs::write(
        &moved_sidecar,
        serde_json::to_vec_pretty(&document).expect("moved project must serialize"),
    )
    .expect("moved project must be writable");

    assert!(core.load_project_v2(moved_sidecar.to_str().unwrap()).is_ok());
    assert!(core.bounce_project(output.to_str().unwrap(), 0));
    assert!(core.validate_render_output(output.to_str().unwrap(), true));

    remove_test_file(project);
    remove_test_file(moved_sidecar);
    remove_test_file(output);
    remove_test_file(assets.join("take.wav"));
    let _ = std::fs::remove_dir(&assets);
    let _ = std::fs::remove_dir(&moved_project);
}
