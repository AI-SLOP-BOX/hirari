use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

fn assert_volume_automation(layout_json: &str, track_id: u32) {
    let layout: serde_json::Value =
        serde_json::from_str(layout_json).expect("native layout must be valid JSON");
    let track = layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .expect("recorded track must remain in the native layout");
    let points = track
        .get("volume_automation")
        .and_then(serde_json::Value::as_array)
        .expect("volume automation must be an array");
    assert_eq!(points.len(), 2);
    assert_eq!(
        points[0].get("time").and_then(serde_json::Value::as_f64),
        Some(0.0)
    );
    assert_eq!(
        points[0].get("value").and_then(serde_json::Value::as_f64),
        Some(0.0)
    );
    assert_eq!(
        points[1].get("time").and_then(serde_json::Value::as_f64),
        Some(2048.0)
    );
    assert_eq!(
        points[1].get("value").and_then(serde_json::Value::as_f64),
        Some(1.0)
    );
}

#[test]
fn volume_automation_changes_render_and_survives_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-automation-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let baseline_path = std::env::temp_dir().join(format!("{token}-baseline.wav"));
    let automated_path = std::env::temp_dir().join(format!("{token}-automated.wav"));
    let reloaded_path = std::env::temp_dir().join(format!("{token}-reloaded.wav"));

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
    eprintln!("layout={}", core.get_project_layout_json());
    eprintln!("wave={:?}", core.get_region_waveform(track_id, 1));
    for e in std::fs::read_dir(std::env::temp_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let p = e.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("aura-capture-") && n.ends_with(".wav"))
            .unwrap_or(false)
        {
            if let Ok(v) = std::fs::read(&p) {
                eprintln!(
                    "source {:?} bytes={} nz={}",
                    p,
                    v.len(),
                    v.get(44..)
                        .map(|x| x.iter().any(|b| *b != 0))
                        .unwrap_or(false)
                );
            }
        }
    }

    assert!(core.bounce_project(
        baseline_path.to_str().expect("baseline path must be UTF-8"),
        0
    ));
    let baseline = std::fs::read(&baseline_path).expect("baseline render must exist");
    eprintln!(
        "baseline len={} payload_nonzero={}",
        baseline.len(),
        baseline[44..].iter().any(|b| *b != 0)
    );

    // Automation times are expressed in rendered sample positions. The curve
    // therefore spans several audio blocks and must affect the actual mixer
    // gain, rather than merely being serialized into the project.
    assert!(core.set_automation_data(track_id, 0, vec![0.0, 0.0, 0.0, 2048.0, 1.0, 0.0]));
    let automation_before_save = core.get_project_layout_json();
    assert_volume_automation(&automation_before_save, track_id);
    assert!(core.bounce_project(
        automated_path
            .to_str()
            .expect("automated path must be UTF-8"),
        0
    ));
    let automated = std::fs::read(&automated_path).expect("automated render must exist");
    assert_ne!(
        baseline[44..],
        automated[44..],
        "volume automation must change the rendered PCM payload"
    );

    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(core.load_project(project_path.to_str().expect("project path must be UTF-8")));
    let automation_after_load = core.get_project_layout_json();
    assert_volume_automation(&automation_after_load, track_id);
    assert!(core.bounce_project(
        reloaded_path.to_str().expect("reloaded path must be UTF-8"),
        0
    ));
    let reloaded = std::fs::read(&reloaded_path).expect("reloaded render must exist");
    assert_eq!(automated.len(), reloaded.len());
    let automated_rms = pcm16_rms(&automated);
    let reloaded_rms = pcm16_rms(&reloaded);
    let rms_delta = (automated_rms - reloaded_rms).abs();
    assert!(
        rms_delta <= automated_rms.max(1.0) * 0.02,
        "automation must survive save/load with equivalent rendered level (RMS delta {rms_delta})"
    );

    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(baseline_path);
    let _ = std::fs::remove_file(automated_path);
    let _ = std::fs::remove_file(reloaded_path);
}

#[test]
fn flex_and_pitch_each_change_rendered_audio_and_survive_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-flex-pitch-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let baseline_path = std::env::temp_dir().join(format!("{token}-baseline.wav"));
    let loop_one_path = std::env::temp_dir().join(format!("{token}-loop-one.wav"));
    let loop_two_path = std::env::temp_dir().join(format!("{token}-loop-two.wav"));
    let flex_path = std::env::temp_dir().join(format!("{token}-flex.wav"));
    let pitch_path = std::env::temp_dir().join(format!("{token}-pitch.wav"));
    let reloaded_path = std::env::temp_dir().join(format!("{token}-reloaded.wav"));

    assert!(core.apply_audio_config(48_000, 1024));
    core.start_recording_capture(48_000.0, 2, 4096, 0)
        .expect("recording capture must start");
    let recorded_audio: Vec<f32> = (0..4096u32)
        .flat_map(|frame| {
            let sample = if frame % 64 < 32 { 0.3 } else { -0.2 };
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
    let layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be JSON");
    let region_id = layout
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

    assert!(core.bounce_project(
        baseline_path.to_str().expect("baseline path must be UTF-8"),
        0
    ));
    let baseline = std::fs::read(&baseline_path).expect("baseline render must exist");

    assert!(core.set_region_loop_count(track_id, region_id, 1));
    assert!(core.bounce_project(loop_one_path.to_str().unwrap(), 0));
    let loop_one = std::fs::read(&loop_one_path).expect("one-loop render must exist");
    assert!(core.set_region_loop_count(track_id, region_id, 2));
    assert!(core.bounce_project(loop_two_path.to_str().unwrap(), 0));
    let loop_two = std::fs::read(&loop_two_path).expect("two-loop render must exist");
    assert!(
        loop_two.len() > loop_one.len(),
        "loop count must increase rendered PCM length"
    );

    assert!(core.set_region_warp_ratio(track_id, region_id, 1.35));
    assert!(core.bounce_project(flex_path.to_str().expect("flex path must be UTF-8"), 0));
    let flex = std::fs::read(&flex_path).expect("flex render must exist");
    assert_ne!(baseline[44..], flex[44..], "Flex must change rendered PCM");

    assert!(core.set_region_warp_ratio(track_id, region_id, 1.0));
    assert!(core.set_region_pitch_semitones(track_id, region_id, 5.0));
    assert!(core.bounce_project(pitch_path.to_str().expect("pitch path must be UTF-8"), 0));
    let pitch = std::fs::read(&pitch_path).expect("pitch render must exist");
    assert_ne!(
        baseline[44..],
        pitch[44..],
        "Pitch must change rendered PCM"
    );

    assert!(core.save_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(core.load_project(project_path.to_str().expect("project path must be UTF-8")));
    assert!(core.bounce_project(
        reloaded_path.to_str().expect("reloaded path must be UTF-8"),
        0
    ));
    let reloaded = std::fs::read(&reloaded_path).expect("reloaded render must exist");
    assert_eq!(pitch.len(), reloaded.len());
    let pitch_rms = pcm16_rms(&pitch);
    let reloaded_rms = pcm16_rms(&reloaded);
    assert!(
        (pitch_rms - reloaded_rms).abs() <= pitch_rms.max(1.0) * 0.02,
        "Flex/Pitch settings must survive save/load"
    );

    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(baseline_path);
    let _ = std::fs::remove_file(loop_one_path);
    let _ = std::fs::remove_file(loop_two_path);
    let _ = std::fs::remove_file(flex_path);
    let _ = std::fs::remove_file(pitch_path);
    let _ = std::fs::remove_file(reloaded_path);
}
