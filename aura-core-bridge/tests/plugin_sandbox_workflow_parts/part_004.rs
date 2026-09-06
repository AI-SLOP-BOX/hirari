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
