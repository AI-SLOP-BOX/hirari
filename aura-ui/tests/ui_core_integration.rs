use aura_core_bridge::AuraCore;
use std::process::Command;

#[cfg(target_os = "macos")]
#[test]
fn main_thread_ui_callbacks_reach_core() {
    let output = Command::new(env!("CARGO_BIN_EXE_aura-ui"))
        .env("AURA_UI_SMOKE", "plugin-core")
        .env("AURA_NATIVE_TEST_ISOLATION", "1")
        .output()
        .expect("Aura UI smoke binary must start");

    assert!(
        output.status.success(),
        "Aura UI smoke exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AUDIO CONFIG: 48000 Hz · 256 samples"));
    assert!(stdout.contains("plugin_core=true"));
    assert!(stdout.contains("action=PLUGIN PARAM: TRACK"));
}

#[cfg(target_os = "macos")]
#[test]
fn main_thread_production_workflow_reaches_audio_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_aura-ui"))
        .env("AURA_UI_SMOKE", "production-workflow")
        .env("AURA_NATIVE_TEST_ISOLATION", "1")
        .output()
        .expect("Aura UI production workflow must start");

    assert!(
        output.status.success(),
        "Aura UI workflow exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("recording=true"), "{stdout}");
    assert!(stdout.contains("capture_non_silent=true"), "{stdout}");
    assert!(stdout.contains("edit=true"), "{stdout}");
    assert!(stdout.contains("automation=true"), "{stdout}");
    assert!(stdout.contains("plugin=true"), "{stdout}");
    assert!(stdout.contains("preset=true"), "{stdout}");
    assert!(stdout.contains("preset_value=0.350"), "{stdout}");
    assert!(stdout.contains("plugin_state_restored=true"), "{stdout}");
    assert!(stdout.contains("sidechain=true"), "{stdout}");
    assert!(stdout.contains("comping=true"), "{stdout}");
    assert!(stdout.contains("comp_take_id=1"), "{stdout}");
    assert!(stdout.contains("comp_at_1=(1, 0)"), "{stdout}");
    assert!(stdout.contains("project=true"), "{stdout}");
    assert!(stdout.contains("state_restored=true"), "{stdout}");
    assert!(stdout.contains("edit_values_restored=true"), "{stdout}");
    assert!(
        stdout.contains("automation_values_restored=true"),
        "{stdout}"
    );
    assert!(stdout.contains("warp=1.100"), "{stdout}");
    assert!(stdout.contains("pitch=2.000"), "{stdout}");
    assert!(stdout.contains("loop=2"), "{stdout}");
    assert!(stdout.contains("render=true"), "{stdout}");
    assert!(stdout.contains("render_spec_ok=true"), "{stdout}");
    assert!(stdout.contains("render_non_silent=true"), "{stdout}");
}

#[cfg(target_os = "macos")]
#[test]
fn main_thread_template_navigation_handles_empty_peak_models() {
    let output = Command::new(env!("CARGO_BIN_EXE_aura-ui"))
        .env("AURA_UI_SMOKE", "template-navigation")
        .env("AURA_NATIVE_TEST_ISOLATION", "1")
        .output()
        .expect("Aura UI template smoke binary must start");

    assert!(
        output.status.success(),
        "Aura UI template smoke exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("template_navigation tracks=5"), "{stdout}");
    assert!(stdout.contains("TEMPLATE READY: ELECTRONIC"), "{stdout}");
}

/// Keeps the integration suite meaningful on CI hosts without AppKit. The
/// actual Slint callback tests remain macOS-only, while the production audio
/// path is exercised through the public Core API everywhere else.
#[test]
fn cross_platform_core_workflow_reaches_render_output() {
    let core = AuraCore::new().expect("Core must initialize on every supported host");
    let token = format!(
        "aura-ui-core-workflow-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.aura"));
    let render_path = std::env::temp_dir().join(format!("{token}.wav"));
    let track_id = core.add_track(0);

    let recording = core
        .arm_recording_capture(48_000.0, 2, 256)
        .and_then(|()| core.start_recording_capture(48_000.0, 2, 256, 0))
        .and_then(|()| core.append_recording_preview(&[0.2, -0.2, 0.1, -0.1]))
        .and_then(|()| core.commit_recording_capture_to_track(track_id, None));
    assert!(
        recording.is_ok(),
        "recording workflow failed: {recording:?}"
    );

    let region_id = serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
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
        .expect("recording must create a region") as u32;
    assert!(core.set_region_gain(track_id, region_id, 0.8));
    assert!(core.set_region_pitch_semitones(track_id, region_id, 1.0));
    assert!(core.save_project(project_path.to_string_lossy().as_ref()));
    assert!(core.load_project(project_path.to_string_lossy().as_ref()));
    assert!(core.bounce_project(render_path.to_string_lossy().as_ref(), 0));

    let output = std::fs::read(&render_path).expect("render output must exist");
    assert!(output.starts_with(b"RIFF"), "render must be a WAV file");
    assert!(output.len() > 44, "render must contain audio data");

    let _ = core.remove_track(track_id);
    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(render_path);
}
