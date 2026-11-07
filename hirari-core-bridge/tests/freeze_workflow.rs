use hirari_core_bridge::engine_edit_ext::TrackEditExt;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn offline_engine_freezes_restores_and_unfreezes_through_the_rust_host() {
    let _guard = native_engine_test_guard();
    let engine = hirari_core_bridge::new_audio_engine_offline();
    let engine = &*engine;
    let track = engine.add_track(0);
    assert_ne!(track, 0);

    assert!(!engine.is_track_frozen(track));
    assert!(engine.freeze_track(track, 512, 48_000));
    assert!(engine.is_track_frozen(track));
    // Admission limits are rejected before the callback drain.
    assert!(!engine.freeze_track(track, 0, 48_000));
    assert!(!engine.freeze_track(track, 512, 0));
    assert!(!engine.freeze_track(track, 64 * 1024 * 1024 + 1, 48_000));

    let path = std::env::temp_dir().join(format!(
        "hirari-freeze-{}-{}.wav",
        std::process::id(),
        track
    ));
    let path = path.to_str().expect("temporary path must be UTF-8");
    assert!(engine.freeze_track_to_file(track, path, 512, 48_000));
    assert!(std::path::Path::new(path).exists());
    assert!(engine.unfreeze_track(track));
    assert!(!engine.is_track_frozen(track));
    assert!(engine.restore_track_freeze_from_file(track, path, 512, 48_000));
    assert!(engine.is_track_frozen(track));
    assert!(engine.set_track_freeze_cache_path(track, path));

    assert!(engine.unfreeze_track(track));
    assert!(!engine.is_track_frozen(track));
    assert!(!engine.unfreeze_track(424_242));
    assert!(!engine.is_track_frozen(424_242));

    let _ = std::fs::remove_file(path);
}
