use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

fn wait_for_track_layout<F: Fn(&serde_json::Value) -> bool>(
    core: &AuraCore,
    track_id: u32,
    check: F,
) {
    let timeout_secs = std::env::var("AURA_RENDER_TEST_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut last_layout = String::new();
    while std::time::Instant::now() < deadline {
        let layout_json = core.get_project_layout_json();
        last_layout = layout_json.clone();
        if let Ok(layout) = serde_json::from_str::<serde_json::Value>(&layout_json) {
            if layout
                .as_array()
                .and_then(|tracks| {
                    tracks.iter().find(|track| {
                        track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                    })
                })
                .is_some_and(&check)
            {
                return;
            }
        }
        // Poll the published layout without assuming a fixed render latency;
        // the deadline remains the only timing contract for slow CI hosts.
        std::thread::yield_now();
    }
    panic!("track layout update did not complete before {timeout_secs}s timeout for track {track_id}; direct volume: {:?}; last layout: {last_layout}", core.track_volume(track_id));
}

#[test]
fn audio_rate_and_buffer_variants_publish_matching_wav_headers() {
    let _guard = native_engine_test_guard();
    let token = format!(
        "aura-audio-config-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );

    for (index, (sample_rate, buffer_size)) in [
        (44_100_u32, 64_u32),
        (48_000, 256),
        (88_200, 512),
        (96_000, 1_024),
        (44_100, 32),
        (48_000, 128),
        (192_000, 256),
        (48_000, 2_048),
    ]
    .into_iter()
    .enumerate()
    {
        let core = AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.apply_audio_config(sample_rate, buffer_size));
        core.start_recording_capture(sample_rate as f32, 2, 64, 0)
            .expect("capture must start for supported format");
        core.append_recording_preview(&[0.2, -0.2, 0.1, -0.1])
            .expect("capture block must be accepted");
        core.commit_recording_capture_to_track(track_id, None)
            .expect("capture must publish");

        let output = std::env::temp_dir().join(format!("{token}-{index}.wav"));
        assert!(core.bounce_project(output.to_str().unwrap(), 0));
        let bytes = std::fs::read(&output).expect("render must exist");
        assert!(bytes.len() >= 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            sample_rate
        );
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16);
        remove_test_file(output);
    }
}

#[test]
fn repeated_save_reload_render_cycles_keep_project_audio_valid() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1_024));
    core.start_recording_capture(48_000.0, 2, 4096, 0)
        .expect("capture must start");
    let audio = (0..4096usize)
        .flat_map(|frame| {
            let sample = if frame % 24 < 12 { 0.3 } else { -0.2 };
            [sample, sample]
        })
        .collect::<Vec<_>>();
    core.append_recording_preview(&audio)
        .expect("capture must accept audio");
    let track_id = core.add_track(0);
    core.commit_recording_capture_to_track(track_id, None)
        .expect("capture must publish");

    let token = format!(
        "aura-repeated-cycle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let render_path = std::env::temp_dir().join(format!("{token}.wav"));

    let find_region_id = || {
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            .ok()
            .and_then(|layout| layout.as_array().cloned())
            .and_then(|tracks| {
                tracks.into_iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions").cloned())
            .and_then(|regions| regions.as_array().and_then(|items| items.first().cloned()))
            .and_then(|region| region.get("id").and_then(serde_json::Value::as_u64))
            .map(|id| id as u32)
    };

    for cycle in 0..32 {
        let gain = 0.35 + cycle as f32 * 0.05;
        let current_region_id = find_region_id().expect("current region id must resolve");
        assert!(core.set_region_gain(track_id, current_region_id, gain));
        assert!(core.save_project(project_path.to_str().unwrap()));
        assert!(core.load_project(project_path.to_str().unwrap()));

        let restored: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("reloaded layout must be valid JSON");
        let restored_gain = restored
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| regions.first())
            .and_then(|region| region.get("clip_gain"))
            .and_then(serde_json::Value::as_f64)
            .expect("reloaded gain must be present");
        assert!((restored_gain - gain as f64).abs() < 0.0001);

        assert!(core.bounce_project(render_path.to_str().unwrap(), 0));
        assert!(core.validate_render_output(render_path.to_str().unwrap(), true));
        let rendered = std::fs::read(&render_path).expect("render must exist");
        assert!(rendered.len() > 44);
        assert_eq!(&rendered[0..4], b"RIFF");
        assert!(
            rendered[44..].iter().any(|sample| *sample != 0),
            "cycle {cycle} must retain non-zero rendered PCM"
        );
    }

    remove_test_file(&project_path);
    remove_test_file(&render_path);
    for generation in 1..=10 {
        remove_test_file(format!("{}.bak.{}", project_path.display(), generation));
    }
}

#[test]
fn large_project_save_reload_render_cycles_preserve_track_graph() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 512));

    // Exercise the bounded project model at a size large enough to expose
    // accidental singleton state, quadratic snapshot work, and generation
    // drift without making the normal test suite depend on a large fixture.
    let mut track_ids = Vec::with_capacity(64);
    for _ in 0..64 {
        let id = core.add_track(0);
        assert_ne!(id, 0, "large project track allocation must succeed");
        track_ids.push(id);
    }
    let token = format!(
        "aura-large-project-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{token}.aura"));
    let render = std::env::temp_dir().join(format!("{token}.wav"));

    for cycle in 0..8 {
        core.save_project_v2(project.to_str().unwrap(), "Large Project", 120.0)
            .unwrap_or_else(|error| panic!("save cycle {cycle}: {error}"));
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("large project layout must remain valid JSON");
        assert_eq!(layout.as_array().map(Vec::len), Some(64));

        core.load_project_v2(project.to_str().unwrap())
            .unwrap_or_else(|error| panic!("load cycle {cycle}: {error}"));
        let restored: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("restored layout must remain valid JSON");
        let restored_ids = restored
            .as_array()
            .expect("restored layout must be an array")
            .iter()
            .filter_map(|track| track.get("id").and_then(serde_json::Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(restored_ids.len(), track_ids.len());
        assert_eq!(
            restored_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            64
        );

        assert!(
            core.bounce_project(render.to_str().unwrap(), 0),
            "render cycle {cycle}"
        );
        let bytes = std::fs::read(&render).expect("large project render must exist");
        assert!(bytes.len() >= 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
    }

    remove_test_file(project);
    remove_test_file(render);
}

#[test]
fn production_edit_save_reload_and_render_workflow_is_consistent() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-production-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let render_path = std::env::temp_dir().join(format!("{token}.wav"));

    assert!(core.apply_audio_config(48_000, 1024));
    core.start_recording_capture(48_000.0, 2, 4096, 0)
        .expect("recording capture must start");
    let recorded_audio: Vec<f32> = (0..4096u32)
        .flat_map(|frame| {
            let sample = if frame % 32 < 16 { 0.25 } else { -0.25 };
            [sample, sample]
        })
        .collect();
    core.append_recording_preview(&recorded_audio)
        .expect("recorded audio must be accepted");

    let track_id = core.add_track(0);
    assert!(
        core.commit_recording_capture_to_track(track_id, None)
            .expect("recording take must publish")
            > 0
    );

    let layout_before_edit: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be JSON");
    let region_id = layout_before_edit
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("regions"))
        .and_then(serde_json::Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(serde_json::Value::as_u64)
        .expect("recording must publish a region") as u32;

    let waveform = core.get_region_waveform(track_id, region_id);
    assert!(
        !waveform.is_empty(),
        "waveform request must return peak data"
    );
    assert!(
        waveform.iter().any(|sample| *sample > 0.0),
        "waveform peaks must contain the recorded signal"
    );

    let baseline_render_path = std::env::temp_dir().join(format!("{token}-baseline.wav"));
    assert!(core.bounce_project(
        baseline_render_path
            .to_str()
            .expect("baseline render path must be UTF-8"),
        0
    ));
    let baseline_bytes = std::fs::read(&baseline_render_path).expect("baseline render must exist");
    assert!(baseline_bytes[44..].iter().any(|sample| *sample != 0));

    assert!(core.set_region_gain(track_id, region_id, 0.75));
    assert!(core.set_region_fades(track_id, region_id, 0.02, 0.04));
    assert!(core.set_region_warp_ratio(track_id, region_id, 1.1));
    assert!(core.set_region_pitch_semitones(track_id, region_id, 2.0));
    assert!(core.set_region_loop_count(track_id, region_id, 2));
    assert!(core.add_plugin(track_id, 0));
    assert!(core.set_automation_data(track_id, 0, vec![0.0, 0.5, 0.0, 22050.0, 0.75, 0.0]));
    let pre_reload_render_path = std::env::temp_dir().join(format!("{token}-pre.wav"));
    assert!(core.bounce_project(
        pre_reload_render_path
            .to_str()
            .expect("pre-render path must be UTF-8"),
        0
    ));
    let pre_reload_bytes = std::fs::read(&pre_reload_render_path).expect("pre-render must exist");
    assert!(
        pre_reload_bytes[44..].iter().any(|sample| *sample != 0),
        "pre-reload rendered PCM payload must contain audio data"
    );
    assert_ne!(
        baseline_bytes[44..],
        pre_reload_bytes[44..],
        "Flex/Pitch edits must change the rendered PCM payload"
    );

    let before_reload: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("edited layout must be JSON");
    assert!(before_reload.to_string().contains("warp_ratio"));
    assert!(before_reload.to_string().contains("pitch_semitones"));
    assert!(before_reload.to_string().contains("loop_count"));

    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(core.load_project(project_path.to_str().expect("project path must be UTF-8")));
    let after_reload: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
        .expect("reloaded layout must be JSON");
    let mut before_semantic = before_reload.clone();
    let mut after_semantic = after_reload.clone();
    remove_runtime_region_ids(&mut before_semantic);
    remove_runtime_region_ids(&mut after_semantic);
    assert_eq!(
        before_semantic, after_semantic,
        "save/load must preserve semantic edited state"
    );

    assert!(core.bounce_project(render_path.to_str().expect("render path must be UTF-8"), 0));
    let bytes = std::fs::read(&render_path).expect("render output must exist");
    assert!(bytes.len() >= 44);
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert!(
        bytes[44..].iter().any(|sample| *sample != 0),
        "rendered PCM payload must contain audio data"
    );
    assert_eq!(
        pre_reload_bytes[44..],
        bytes[44..],
        "save/load must preserve the edited render output"
    );

    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(baseline_render_path);
    let _ = std::fs::remove_file(pre_reload_render_path);
    let _ = std::fs::remove_file(render_path);
}

#[test]
fn async_render_cancellation_does_not_publish_a_partial_output() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-cancel-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let output = std::env::temp_dir().join(format!("{token}.wav"));
    remove_test_file(&output);

    assert!(core.start_render_async_to(output.to_str().unwrap()));
    let mut cancellation_accepted = false;
    let cancel_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < cancel_deadline {
        if core.cancel_render() {
            cancellation_accepted = true;
            break;
        }
        std::thread::yield_now();
    }
    assert!(
        cancellation_accepted,
        "render must accept cancellation while active"
    );

    let mut state = 0;
    let cancel_state_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < cancel_state_deadline {
        state = core
            .get_bounce_status()
            .map(|(value, _)| value)
            .unwrap_or(0);
        if state == 5 {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(state, 5, "cancelled render must publish Cancelled state");
    assert!(
        !output.exists(),
        "cancelled render must not publish a partial WAV"
    );
    let temporary_prefix = format!(
        "{}.tmp-render-",
        output.file_name().unwrap().to_string_lossy()
    );
    let temporary_outputs = std::fs::read_dir(output.parent().unwrap())
        .expect("temporary directory must be readable")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&temporary_prefix)
        });
    assert!(
        !temporary_outputs,
        "cancelled render must remove its temporary output"
    );

    assert!(core.start_render_async_to(output.to_str().unwrap()));
    let retry_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < retry_deadline {
        if core
            .get_bounce_status()
            .is_some_and(|(value, _)| value == 3 || value == 4)
        {
            break;
        }
        std::thread::yield_now();
    }
    assert!(output.exists(), "a cancelled destination must be reusable");
    remove_test_file(output);
}

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
    assert!(core.save_project(project.to_str().unwrap()));
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
