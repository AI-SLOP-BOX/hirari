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
