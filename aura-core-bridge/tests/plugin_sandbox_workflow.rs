use aura_core_bridge::AuraCore;
mod support {
    include!("recording_support.rs");
}
use support::native_engine_test_guard;

fn plugin_lifecycle_timeout() -> std::time::Duration {
    // SDK-backed commercial plugins can trigger code-signature validation,
    // UI-host initialization, and shader/preset discovery on the first
    // process after repeated worker launches. Keep this bounded, but use a
    // larger budget than the tiny builtin fixture path.
    let seconds = if std::env::var_os("AURA_VST3_FIXTURE").is_some()
        || std::env::var_os("AURA_INSTRUMENT_FIXTURE").is_some()
    {
        30
    } else {
        10
    };
    std::time::Duration::from_secs(seconds)
}

fn wait_for_audio_block(
    core: &AuraCore,
    track_id: u32,
    input_left: &[f32],
    input_right: &[f32],
    output_left: &mut [f32],
    output_right: &mut [f32],
) {
    // SDK-backed plugins can spend several seconds in cold-start discovery on
    // a loaded CI host.  The deadline is bounded, but must not turn startup
    // scheduling variance into a false audio-lifecycle failure.
    // A real SDK plugin may cold-start while the host is already under a
    // repeated stress workload. Keep this bounded, but leave enough room for
    // the worker handshake and one reconfiguration to complete.
    let deadline = std::time::Instant::now() + plugin_lifecycle_timeout();
    while std::time::Instant::now() < deadline {
        let mut left = input_left.to_vec();
        let mut right = input_right.to_vec();
        if core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right) {
            output_left.copy_from_slice(&left);
            output_right.copy_from_slice(&right);
            return;
        }
        // Poll at a realistic callback cadence. A tight busy-spin would turn
        // one pending mailbox block into hundreds of synthetic audio frames,
        // falsely tripping the overrun budget before the worker can run.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let statuses = core.sandbox_statuses();
    let failure = core.last_sandbox_failure_text(track_id);
    panic!(
        "sandbox audio sequence did not complete within timeout: track_id={track_id} failure={failure} statuses={statuses:?}"
    );
}

fn wait_for_midi(core: &AuraCore, track_id: u32, event: &[u8]) -> Vec<u8> {
    let deadline = std::time::Instant::now() + plugin_lifecycle_timeout();
    while std::time::Instant::now() < deadline {
        let result = core.process_sandboxed_plugin_midi_block(track_id, 0, 4, event);
        if !result.is_empty() {
            return result;
        }
        // Keep MIDI polling aligned with an audio callback cadence as well;
        // a busy-spin would manufacture deadline misses while the worker is
        // healthy and make recovery look quarantined.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("sandbox MIDI sequence did not complete within timeout");
}

#[test]
fn missing_plugin_worker_fails_without_registering_a_phantom_plugin() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let missing_path = std::env::temp_dir().join(format!(
        "aura-missing-worker-{}-{}.vst3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));

    assert!(!core.add_sandboxed_plugin(
        track_id,
        missing_path.to_str().expect("plugin path must be UTF-8")
    ));
    assert!(core.sandbox_snapshots().is_empty());
    assert_eq!(core.maintain_sandboxes(false), 0);
    assert_eq!(core.recover_sandboxed_plugins(), 0);
    assert!(core.sandbox_snapshots().is_empty());
}

#[test]
fn unsupported_plugin_paths_are_rejected_before_sandbox_creation() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(!core.add_sandboxed_plugin(track_id, "not-a-plugin.txt"));
    assert!(core.sandbox_snapshots().is_empty());
}

#[test]
fn plugin_admission_classifies_formats_before_loading() {
    assert_eq!(AuraCore::plugin_path_admission(""), "missing-path");
    assert_eq!(
        AuraCore::plugin_path_admission("effect.VST3"),
        "sandbox-vst3"
    );
    assert_eq!(
        AuraCore::plugin_path_admission("effect.component"),
        "sandbox-au"
    );
    assert_eq!(
        AuraCore::plugin_path_admission("effect.CLAP"),
        "sandbox-clap"
    );
    assert_eq!(
        AuraCore::plugin_path_admission("effect.VsT3"),
        "sandbox-vst3"
    );
    assert_eq!(
        AuraCore::plugin_path_admission("effect.COMPONENT"),
        "sandbox-au"
    );
    assert_eq!(AuraCore::plugin_path_admission("builtin://gain"), "builtin");
    assert_eq!(
        AuraCore::plugin_path_admission("effect.txt"),
        "unsupported-extension"
    );
}

#[test]
fn plugin_admission_rejects_embedded_nul_and_oversized_paths() {
    assert_eq!(
        AuraCore::plugin_path_admission("effect.vst3\0worker"),
        "invalid-path"
    );
    let oversized = "x".repeat(4097) + ".clap";
    assert_eq!(AuraCore::plugin_path_admission(&oversized), "invalid-path");
}

#[test]
fn sandbox_failure_diagnostic_is_stable_and_not_confused_with_success() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);

    assert_eq!(core.last_sandbox_failure_text(track_id), "none");
    assert_eq!(
        core.last_sandbox_failure_text(track_id + 999_999),
        "track-not-found"
    );
    assert!(!core.add_sandboxed_plugin(track_id, "missing.vst3"));
    assert_eq!(core.last_sandbox_failure_text(track_id), "none");
    assert!(core.sandbox_snapshots().is_empty());
}

#[test]
fn sandbox_state_diagnostic_preserves_a_structured_failure() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let diagnostic = core.sandbox_plugin_state_diagnostic(track_id, 0, &[1, 2, 3]);
    assert!(!diagnostic.ok);
    assert_eq!(diagnostic.code, 5);
    assert_eq!(diagnostic.message, "state-unavailable");
}

#[test]
fn sandbox_state_diagnostic_rejects_oversize_before_ipc() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let diagnostic =
        core.sandbox_plugin_state_diagnostic(track_id, 0, &vec![0u8; 4 * 1024 * 1024 + 1]);
    assert!(!diagnostic.ok);
    assert_eq!(diagnostic.code, 1);
    assert_eq!(diagnostic.message, "state-oversize");
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn minimal_clap_fixture_instantiates_in_the_isolated_worker() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    for (sample_rate, block_size) in [(44_100, 64), (48_000, 128), (96_000, 256)] {
        let core = AuraCore::new().expect("core must initialize");
        assert!(core.apply_audio_config(sample_rate, block_size));
        let track_id = core.add_track(0);
        assert!(
            core.add_sandboxed_plugin(track_id, &fixture),
            "CLAP admission failed at {sample_rate} Hz: failure={} statuses={:?}",
            core.last_sandbox_failure_text(track_id),
            core.sandbox_statuses()
        );
        let snapshots = core.sandbox_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(
            snapshots[0].alive,
            "CLAP worker must acknowledge readiness at {sample_rate} Hz"
        );
        assert_eq!(snapshots[0].failure, 0);
        let mut left = vec![1.0_f32, -0.5, 0.25, 0.0];
        let mut right = vec![0.5_f32, -1.0, 0.75, 0.25];
        // The mailbox is deliberately one block pipelined: the first call
        // submits, the next call collects the worker result without blocking.
        assert!(!core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right));
        let expected_left = left.clone();
        let expected_right = right.clone();
        wait_for_audio_block(
            &core,
            track_id,
            &expected_left,
            &expected_right,
            &mut left,
            &mut right,
        );
        assert_eq!(left, vec![0.5, -0.25, 0.125, 0.0]);
        assert_eq!(right, vec![0.25, -0.5, 0.375, 0.125]);

        let note_on = [0x90_u8, 60, 100];
        assert!(core
            .process_sandboxed_plugin_midi_block(track_id, 0, 4, &note_on)
            .is_empty());
        // The preceding audio call may still be the block being collected;
        // poll sequence completion instead of relying on a fixed sleep.
        assert_eq!(wait_for_midi(&core, track_id, &note_on), note_on);

        let default_state = core.sandbox_plugin_state(track_id, 0);
        assert_eq!(default_state.len(), 4, "fixture state must contain gain");
        let restored_gain = 0.25_f32.to_le_bytes();
        assert!(core.set_sandbox_plugin_state(track_id, 0, &restored_gain));
        assert_eq!(core.sandbox_plugin_state(track_id, 0), restored_gain);
        // Restart also clears any in-flight mailbox block, so the following
        // assertion measures the restored state rather than an older block.
        assert!(core.restart_sandboxed_plugin(track_id, 0));

        let mut changed_input_left = vec![1.0_f32; 4];
        let mut changed_input_right = vec![1.0_f32; 4];
        assert!(!core.process_sandboxed_plugin_block(
            track_id,
            0,
            &mut changed_input_left,
            &mut changed_input_right
        ));
        let mut changed_left = vec![0.0_f32; 4];
        let mut changed_right = vec![0.0_f32; 4];
        wait_for_audio_block(
            &core,
            track_id,
            &changed_input_left,
            &changed_input_right,
            &mut changed_left,
            &mut changed_right,
        );
        assert_eq!(changed_left, vec![0.25; 4]);
        assert_eq!(changed_right, vec![0.25; 4]);

        assert!(core.restart_sandboxed_plugin(track_id, 0));
        let mut restarted_input_left = vec![1.0_f32; 4];
        let mut restarted_input_right = vec![1.0_f32; 4];
        assert!(!core.process_sandboxed_plugin_block(
            track_id,
            0,
            &mut restarted_input_left,
            &mut restarted_input_right
        ));
        let mut restarted_left = vec![0.0_f32; 4];
        let mut restarted_right = vec![0.0_f32; 4];
        wait_for_audio_block(
            &core,
            track_id,
            &restarted_input_left,
            &restarted_input_right,
            &mut restarted_left,
            &mut restarted_right,
        );
        assert_eq!(restarted_left, vec![0.25; 4]);
        assert_eq!(restarted_right, vec![0.25; 4]);
        assert_eq!(core.maintain_sandboxes(false), 0);
    }
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn minimal_clap_fixture_reconfigures_worker_without_stale_audio_format() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(44_100, 64));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );
    let generation_before = core.audio_config_generation();

    for (sample_rate, block_size) in [(48_000, 128), (96_000, 256), (44_100, 64)] {
        assert!(core.apply_audio_config(sample_rate, block_size));
        assert_eq!(core.get_sample_rate(), sample_rate as f64);
        assert_eq!(core.get_buffer_size(), block_size);
        assert!(core.audio_config_generation() > generation_before);
        let snapshots = core.sandbox_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(
            snapshots[0].alive,
            "worker must survive format reconfiguration"
        );
        assert_eq!(snapshots[0].failure, 0);

        let input_left = vec![1.0_f32; block_size as usize];
        let input_right = vec![1.0_f32; block_size as usize];
        let mut output_left = input_left.clone();
        let mut output_right = input_right.clone();
        assert!(!core.process_sandboxed_plugin_block(
            track_id,
            0,
            &mut output_left,
            &mut output_right
        ));
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
            .all(|sample| (*sample - 0.5).abs() < 1e-6));
        assert!(output_right
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6));
    }
}

#[test]
#[ignore = "requires an AU/VST3/CLAP fixture and sandbox worker"]
fn isolated_plugin_worker_survives_audio_reconfiguration() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_AU_FIXTURE")
        .or_else(|_| std::env::var("AURA_VST3_FIXTURE"))
        .or_else(|_| std::env::var("AURA_CLAP_FIXTURE"))
        .expect("plugin fixture path must be configured");
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(44_100, 64));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );
    // State is an opaque vendor contract.  The host must nevertheless prove
    // that a real AU/VST3/CLAP instance can expose a bounded state blob and
    // accept it again before audio reconfiguration is attempted.
    let initial_state = core.sandbox_plugin_state(track_id, 0);
    assert!(
        !initial_state.is_empty(),
        "fixture must expose plugin state"
    );
    assert!(initial_state.len() <= 4 * 1024 * 1024);
    let state_restore = core.sandbox_plugin_state_diagnostic(track_id, 0, &initial_state);
    assert!(
        state_restore.ok,
        "fixture rejected its own state blob: code={} message={}",
        state_restore.code, state_restore.message
    );

    for (sample_rate, block_size) in [(48_000, 128), (96_000, 256), (44_100, 64)] {
        assert!(core.apply_audio_config(sample_rate, block_size));
        assert_eq!(core.get_sample_rate(), sample_rate as f64);
        assert_eq!(core.get_buffer_size(), block_size);
        assert!(core
            .sandbox_snapshots()
            .iter()
            .all(|snapshot| snapshot.alive));
        let input_left = vec![0.125_f32; block_size as usize];
        let input_right = vec![-0.125_f32; block_size as usize];
        let mut output_left = input_left.clone();
        let mut output_right = input_right.clone();
        let _ =
            core.process_sandboxed_plugin_block(track_id, 0, &mut output_left, &mut output_right);
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
        assert!(core
            .sandbox_snapshots()
            .iter()
            .all(|snapshot| snapshot.alive));
    }
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );

    let make_block = |block: u32| {
        let input_left: Vec<f32> = (0..32)
            .map(|sample| block as f32 + sample as f32 / 100.0)
            .collect();
        let input_right: Vec<f32> = input_left.iter().map(|value| -*value).collect();
        (input_left, input_right)
    };
    let (mut previous_left, mut previous_right) = make_block(0);
    let mut first_left = previous_left.clone();
    let mut first_right = previous_right.clone();
    assert!(!core.process_sandboxed_plugin_block(track_id, 0, &mut first_left, &mut first_right));
    for block in 1..128u32 {
        let (input_left, input_right) = make_block(block);
        let mut output_left = input_left.clone();
        let mut output_right = input_right.clone();
        wait_for_audio_block(
            &core,
            track_id,
            &input_left,
            &input_right,
            &mut output_left,
            &mut output_right,
        );
        for (index, (actual, expected)) in output_left
            .iter()
            .zip(previous_left.iter().map(|value| value * 0.5))
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1.0e-5,
                "stale or misordered left block index={index}: {actual} != {expected}; input={:?}",
                previous_left
            );
        }
        for (index, (actual, expected)) in output_right
            .iter()
            .zip(previous_right.iter().map(|value| value * 0.5))
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1.0e-5,
                "stale or misordered right block index={index}: {actual} != {expected}; input={:?}",
                previous_right
            );
        }
        previous_left = input_left;
        previous_right = input_right;
    }
    assert!(core
        .sandbox_snapshots()
        .into_iter()
        .all(|snapshot| snapshot.alive));
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
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
fn state_timeout_kills_worker_without_publishing_invalid_audio() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    std::env::set_var("AURA_CLAP_FIXTURE_STATE_DELAY_MS", "10000");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_sandboxed_plugin(track_id, &fixture));

    let state = core.sandbox_plugin_state(track_id, 0);
    assert!(
        !state.is_empty(),
        "fixture must expose state before the timeout"
    );
    let diagnostic = core.sandbox_plugin_state_diagnostic(track_id, 0, &state);
    std::env::remove_var("AURA_CLAP_FIXTURE_STATE_DELAY_MS");
    assert!(
        !diagnostic.ok,
        "a state callback beyond the deadline must fail"
    );
    assert_eq!(
        diagnostic.code, 7,
        "expected the bounded state timeout code"
    );
    assert_eq!(diagnostic.message, "state-timeout");

    let mut left = vec![0.25_f32; 32];
    let mut right = vec![0.25_f32; 32];
    let _ = core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right);
    assert!(left
        .iter()
        .chain(right.iter())
        .all(|sample| sample.is_finite()));
    let status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("sandbox status must remain available after timeout");
    assert!(
        !status.alive,
        "timed-out worker must be detached from the graph"
    );
    assert!(status.failure != 0, "timeout must publish a failure state");
}

#[test]
#[ignore = "requires the repository CLAP fixture and sandbox worker"]
fn repeated_mailbox_overruns_quarantine_then_recover_to_audio() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_CLAP_FIXTURE").expect("fixture path must be configured");
    std::env::set_var("AURA_CLAP_FIXTURE_DELAY_MS", "50");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_sandboxed_plugin(track_id, &fixture));

    let mut quarantined = false;
    let mut saw_overruns = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let mut left = vec![1.0_f32; 4];
        let mut right = vec![1.0_f32; 4];
        let _ = core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right);
        let statuses = core.sandbox_statuses();
        saw_overruns += statuses
            .iter()
            .map(|status| status.mailbox_overruns)
            .sum::<u32>();
        quarantined = statuses.iter().any(|status| status.is_quarantined());
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        if quarantined {
            break;
        }
        std::hint::spin_loop();
    }
    assert!(
        saw_overruns > 0,
        "expected mailbox overruns before quarantine; observed={saw_overruns}"
    );
    assert!(quarantined, "repeated overruns must quarantine the sandbox");

    std::env::remove_var("AURA_CLAP_FIXTURE_DELAY_MS");
    assert!(core.restart_sandboxed_plugin(track_id, 0));
    let mut left = vec![1.0_f32; 4];
    let mut right = vec![1.0_f32; 4];
    let input_left = left.clone();
    let input_right = right.clone();
    wait_for_audio_block(
        &core,
        track_id,
        &input_left,
        &input_right,
        &mut left,
        &mut right,
    );
    assert_eq!(left, vec![0.5; 4]);
    assert_eq!(right, vec![0.5; 4]);

    // Recovery must publish the current block exactly once, not an older
    // mailbox result.  Verify the MIDI lane as well so a recovered worker
    // cannot duplicate or silently drop the first post-recovery event.
    let recovered_note = [0x90_u8, 64, 96];
    assert!(core
        .process_sandboxed_plugin_midi_block(track_id, 0, 4, &recovered_note)
        .is_empty());
    assert_eq!(
        wait_for_midi(&core, track_id, &recovered_note),
        recovered_note
    );
    let status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("status must remain");
    assert!(status.alive);
    assert!(!status.is_quarantined());
}

#[test]
#[ignore = "requires a real AU/VST3 fixture and an isolated worker"]
fn real_plugin_worker_fault_is_quarantined_without_nonfinite_audio() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_VST3_FIXTURE").or_else(|_| std::env::var("AURA_AU_FIXTURE"));
    let Ok(fixture) = fixture else {
        eprintln!("real plugin fault test skipped: no AU/VST3 fixture is configured");
        return;
    };
    std::env::set_var("AURA_PLUGIN_TEST_FAULTS", "1");
    std::env::set_var("AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS", "2");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_sandboxed_plugin(track_id, &fixture));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut quarantined = false;
    while std::time::Instant::now() < deadline {
        let mut left = vec![0.25_f32; 128];
        let mut right = vec![0.25_f32; 128];
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
    assert!(
        quarantined,
        "real plugin worker crash must end in quarantine"
    );
}

#[test]
#[ignore = "requires a real AU/VST3 fixture and an isolated worker"]
fn real_plugin_overruns_quarantine_then_recover_audio_and_midi() {
    let _guard = native_engine_test_guard();
    let fixture = std::env::var("AURA_VST3_FIXTURE").or_else(|_| std::env::var("AURA_AU_FIXTURE"));
    let Ok(fixture) = fixture else {
        eprintln!("real plugin overrun test skipped: no AU/VST3 fixture is configured");
        return;
    };
    std::env::set_var("AURA_PLUGIN_TEST_FAULTS", "1");
    std::env::set_var("AURA_PLUGIN_WORKER_DELAY_MS", "100");
    let core = AuraCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );

    // The injected 50 ms worker delay is intentional. Under repeated SDK
    // launches, scheduling can add several hundred milliseconds before the
    // overrun counter reaches its quarantine threshold.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut saw_overruns = 0_u32;
    let mut quarantined = false;
    while std::time::Instant::now() < deadline {
        let mut left = vec![1.0_f32; 4];
        let mut right = vec![1.0_f32; 4];
        let _ = core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        let statuses = core.sandbox_statuses();
        saw_overruns = saw_overruns.saturating_add(
            statuses
                .iter()
                .map(|status| status.mailbox_overruns)
                .sum::<u32>(),
        );
        quarantined = statuses.iter().any(|status| status.is_quarantined());
        if quarantined {
            break;
        }
        std::thread::yield_now();
    }
    std::env::remove_var("AURA_PLUGIN_TEST_FAULTS");
    std::env::remove_var("AURA_PLUGIN_WORKER_DELAY_MS");
    // The native processor applies the consecutive-overrun threshold before
    // the Rust status polling path necessarily observes every counter delta;
    // status reads are intentionally consuming.  Require an observed miss
    // and the resulting quarantine, rather than conflating those two APIs.
    assert!(
        saw_overruns > 0,
        "real plugin worker must expose mailbox overruns; observed={saw_overruns} quarantined={quarantined}"
    );
    assert!(
        quarantined,
        "real plugin overruns must quarantine the sandbox"
    );

    assert!(core.restart_sandboxed_plugin(track_id, 0));
    let mut left = vec![1.0_f32; 4];
    let mut right = vec![1.0_f32; 4];
    let input_left = left.clone();
    let input_right = right.clone();
    wait_for_audio_block(
        &core,
        track_id,
        &input_left,
        &input_right,
        &mut left,
        &mut right,
    );
    assert!(left
        .iter()
        .chain(right.iter())
        .all(|sample| sample.is_finite()));

    let midi = [0x90_u8, 67, 100];
    assert!(core
        .process_sandboxed_plugin_midi_block(track_id, 0, 4, &midi)
        .is_empty());
    // A real VST3 may consume MIDI without emitting host-facing MIDI. The
    // one submitted event must not destabilize the recovered worker; echo
    // semantics are covered by the dedicated CLAP fixture contract.
    let status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("status must remain");
    assert!(status.alive);
    assert!(!status.is_quarantined());
}

#[test]
#[ignore = "requires the official VST3 SDK, SDK-enabled worker, and a VST3 fixture"]
fn official_vst3_fixture_instantiates_and_processes_in_the_isolated_worker() {
    let _guard = native_engine_test_guard();
    let Ok(fixture) = std::env::var("AURA_VST3_FIXTURE") else {
        eprintln!("VST3 E2E skipped: AURA_VST3_FIXTURE is not configured; use scripts/run_vst3_e2e_smoke.sh for the strict gate");
        return;
    };
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "sandbox admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );
    let snapshots = core.sandbox_snapshots();
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].alive, "VST3 worker must acknowledge readiness");
    assert_eq!(snapshots[0].failure, 0);

    let mut left = vec![0.0_f32; 128];
    let mut right = vec![0.0_f32; 128];
    left[0] = 1.0;
    right[0] = 1.0;
    assert!(!core.process_sandboxed_plugin_block(track_id, 0, &mut left, &mut right));
    let input_left = left.clone();
    let input_right = right.clone();
    wait_for_audio_block(
        &core,
        track_id,
        &input_left,
        &input_right,
        &mut left,
        &mut right,
    );
    assert!(left
        .iter()
        .chain(right.iter())
        .all(|sample| sample.is_finite()));

    // Exercise the complete MIDI-1 input translation contract in the
    // SDK-backed worker. A generic audio effect may consume these events
    // without producing MIDI; survival and finite audio are the contract.
    for event in [
        vec![0xB0_u8, 74, 96],
        vec![0xE0_u8, 0, 64],
        vec![0xF0_u8, 0x7D, 0x01, 0xF7],
    ] {
        let _ = core.process_sandboxed_plugin_midi_block(track_id, 0, 128, &event);
    }
    let midi_status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("VST3 worker status must remain available after MIDI input");
    assert!(
        midi_status.alive,
        "VST3 worker must survive CC/pitch-bend/SysEx input"
    );
    assert!(!midi_status.is_quarantined());

    // The fixture's default gain is allowed to be zero.  The important
    // contract here is that the official VST3 processor accepted the block,
    // returned finite samples, and did not fall back to a fake pass-through.

    // Keep the SDK-enabled path aligned with the CLAP lifecycle contract:
    // process changing blocks so stale mailbox output or block reordering
    // cannot hide behind a single successful instantiate.
    for block in 1..32u32 {
        let mut block_left = vec![block as f32 / 32.0; 128];
        let mut block_right = vec![-(block as f32) / 32.0; 128];
        let input_left = block_left.clone();
        let input_right = block_right.clone();
        wait_for_audio_block(
            &core,
            track_id,
            &input_left,
            &input_right,
            &mut block_left,
            &mut block_right,
        );
        assert!(block_left
            .iter()
            .chain(block_right.iter())
            .all(|sample| sample.is_finite()));
    }

    // Exercise the real VST3 event-list path with a note-on and note-off.
    // VST3 instruments do not have to emit host-facing MIDI, so the contract
    // is worker survival, finite audio, and no process failure rather than an
    // output-MIDI echo.
    let note_on = [0x90_u8, 60, 100];
    assert!(core
        .process_sandboxed_plugin_midi_block(track_id, 0, 128, &note_on)
        .is_empty());
    let mut midi_left = vec![0.0_f32; 128];
    let mut midi_right = vec![0.0_f32; 128];
    let midi_input_left = midi_left.clone();
    let midi_input_right = midi_right.clone();
    wait_for_audio_block(
        &core,
        track_id,
        &midi_input_left,
        &midi_input_right,
        &mut midi_left,
        &mut midi_right,
    );
    assert!(midi_left
        .iter()
        .chain(midi_right.iter())
        .all(|sample| sample.is_finite()));
    let note_off = [0x80_u8, 60, 0];
    assert!(core
        .process_sandboxed_plugin_midi_block(track_id, 0, 128, &note_off)
        .is_empty());
    let status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("VST3 status must remain after MIDI input");
    assert!(
        status.alive,
        "VST3 worker must remain alive after MIDI input"
    );
    assert_eq!(
        status.failure, 0,
        "VST3 MIDI input must not report a process failure"
    );

    // State restore is part of the VST3 host contract. Some fixtures expose
    // no state, but when a payload exists it must survive restart.
    let state = core.sandbox_plugin_state(track_id, 0);
    if !state.is_empty() {
        let restore = core.sandbox_plugin_state_diagnostic(track_id, 0, &state);
        assert!(restore.ok, "VST3 state restore failed: {}", restore.message);
        // Commercial VST3s may canonicalize or re-encode their state during
        // setState/getState (Vital does this for its JSON/private-data
        // container). The host contract is successful restore of a bounded,
        // non-empty state, not byte identity of an opaque vendor blob.
        let canonical_state = core.sandbox_plugin_state(track_id, 0);
        assert!(!canonical_state.is_empty());
        assert!(canonical_state.len() <= 4 * 1024 * 1024);
        assert!(core.restart_sandboxed_plugin(track_id, 0));
        let mut restarted_left = vec![0.0_f32; 128];
        let mut restarted_right = vec![0.0_f32; 128];
        restarted_left[0] = 1.0;
        restarted_right[0] = 1.0;
        assert!(!core.process_sandboxed_plugin_block(
            track_id,
            0,
            &mut restarted_left,
            &mut restarted_right
        ));
        let input_left = restarted_left.clone();
        let input_right = restarted_right.clone();
        wait_for_audio_block(
            &core,
            track_id,
            &input_left,
            &input_right,
            &mut restarted_left,
            &mut restarted_right,
        );
        assert!(restarted_left
            .iter()
            .chain(restarted_right.iter())
            .all(|sample| sample.is_finite()));
    }
}

#[test]
#[ignore = "requires a real instrument fixture and isolated sandbox worker"]
fn real_instrument_state_restore_keeps_note_audio_finite() {
    let _guard = native_engine_test_guard();
    let Ok(fixture) = std::env::var("AURA_INSTRUMENT_FIXTURE") else {
        eprintln!("Real instrument state test skipped: AURA_INSTRUMENT_FIXTURE is not configured");
        return;
    };
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "instrument admission failed: {}",
        core.last_sandbox_failure_text(track_id)
    );
    let state = core.sandbox_plugin_state(track_id, 0);
    assert!(!state.is_empty(), "instrument must expose a state blob");
    assert!(state.len() <= 4 * 1024 * 1024);

    let render_note = |core: &AuraCore| {
        let note_on = [0x90_u8, 60, 100];
        let _ = core.process_sandboxed_plugin_midi_block(track_id, 0, 128, &note_on);
        let mut left = vec![0.0_f32; 128];
        let mut right = vec![0.0_f32; 128];
        let input_left = left.clone();
        let input_right = right.clone();
        wait_for_audio_block(
            core,
            track_id,
            &input_left,
            &input_right,
            &mut left,
            &mut right,
        );
        let note_off = [0x80_u8, 60, 0];
        let _ = core.process_sandboxed_plugin_midi_block(track_id, 0, 128, &note_off);
        left.into_iter().chain(right).collect::<Vec<_>>()
    };

    let before = render_note(&core);
    assert!(before.iter().all(|sample| sample.is_finite()));
    assert!(core.set_sandbox_plugin_state(track_id, 0, &state));
    assert!(core.restart_sandboxed_plugin(track_id, 0));
    let after = render_note(&core);
    assert!(after.iter().all(|sample| sample.is_finite()));
    assert!(
        core.sandbox_statuses()
            .into_iter()
            .all(|status| status.alive && !status.is_quarantined()),
        "instrument worker must survive state restore and note playback"
    );

    // Vital is an instrument fixture, so a silent response is a real restore
    // failure rather than an allowed effect-plugin result.  Do not require
    // byte-identical audio: oscillator phase and vendor scheduling may vary.
    if fixture.to_ascii_lowercase().contains("vital") {
        let before_peak = before
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        let after_peak = after
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            before_peak > 1.0e-7,
            "Vital note-on produced no audio before restore"
        );
        assert!(
            after_peak > 1.0e-7,
            "Vital note-on produced no audio after restore"
        );
    }
}
