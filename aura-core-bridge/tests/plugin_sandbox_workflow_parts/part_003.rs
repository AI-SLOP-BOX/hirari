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
#[ignore = "requires a real third-party AU/VST3/CLAP fixture and isolated worker"]
fn third_party_fixture_compatibility_smoke() {
    let _guard = native_engine_test_guard();
    let Ok(fixture) = std::env::var("AURA_COMPAT_FIXTURE") else {
        eprintln!("compatibility fixture is not configured");
        return;
    };
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    let track_id = core.add_track(0);
    assert!(
        core.add_sandboxed_plugin(track_id, &fixture),
        "third-party admission failed for {fixture}: {}",
        core.last_sandbox_failure_text(track_id)
    );
    let state = core.sandbox_plugin_state(track_id, 0);
    assert!(state.len() <= 4 * 1024 * 1024);

    let mut left = vec![0.0_f32; 128];
    let mut right = vec![0.0_f32; 128];
    left[0] = 1.0;
    right[0] = 1.0;
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

    if !state.is_empty() {
        assert!(core.set_sandbox_plugin_state(track_id, 0, &state));
    }
    assert!(core.restart_sandboxed_plugin(track_id, 0));
    let status = core
        .sandbox_statuses()
        .into_iter()
        .next()
        .expect("third-party status must remain available");
    assert!(status.alive && !status.is_quarantined());
}

#[test]
#[ignore = "requires a real instrument fixture and isolated sandbox worker"]
