// The offline render target and the track fader read are Rust paths on the
// bounce session and the layout host ops; the deleted C++ methods only
// forwarded to these stores.
#[test]
fn offline_target_and_volume_read_the_rust_stores_directly() {
    let _guard = native_engine_test_guard();
    let engine = crate::new_audio_engine();
    let e = &*engine;
    let core = crate::get_unified_engine(e);

    // A stale target id cannot activate; zero is the master and always valid.
    assert!(!crate::engine_analysis_reads::set_offline_target(core, 9_999));
    assert_eq!(unsafe { crate::bounce_driver::hirari_bounce_target_active(
        core.bounce_session_state() as *mut std::ffi::c_void,
    ) }, 0);
    assert!(crate::engine_analysis_reads::set_offline_target(core, 0));

    let track = e.add_track(0);
    assert!(crate::engine_analysis_reads::set_offline_target(core, track));
    let session = core.bounce_session_state() as *mut std::ffi::c_void;
    assert_eq!(unsafe { crate::bounce_driver::hirari_bounce_target_active(session) }, 1);
    assert_eq!(unsafe { crate::bounce_driver::hirari_bounce_target_id(session) }, track);

    crate::engine_analysis_reads::clear_offline_target(core);
    assert_eq!(unsafe { crate::bounce_driver::hirari_bounce_target_active(session) }, 0);

    crate::engine_analysis_reads::set_offline_tail_seconds(core, 2.5);
    assert_eq!(
        unsafe { crate::bounce_driver::hirari_bounce_tail_millis(session) },
        2_500
    );
    crate::engine_analysis_reads::set_offline_options(core, true, false);
    assert_eq!(
        unsafe { crate::bounce_driver::hirari_bounce_target_id(session) },
        0,
        "options and tail writes must not disturb the cleared target"
    );

    // The fader read is the layout host ops walk; an unknown id keeps the
    // deleted getter's -1.0 fallback.
    assert_eq!(crate::engine_analysis_reads::track_volume(core, 9_999), -1.0);
    let volume = crate::engine_analysis_reads::track_volume(core, track);
    assert!(volume.is_finite(), "fresh track fader must read as finite");
}
