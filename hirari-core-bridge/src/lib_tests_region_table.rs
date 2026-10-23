// Differential coverage for the Rust region-table mirror: after each edit the
// engine reports whether the mirror equals the authoritative C++ region list.

#[test]
fn region_table_mirror_tracks_the_cpp_region_list_through_edits() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("engine must initialize");
    let source_path = std::env::temp_dir().join(format!(
        "hirari-region-mirror-{}-source.wav",
        std::process::id()
    ));
    let source = source_path.to_str().unwrap();

    let track_id = engine_ref.add_track(0);
    assert!(engine_ref.region_table_mirror_matches(track_id), "empty track");
    assert!(!engine_ref.region_table_mirror_matches(track_id + 99), "unknown track");

    assert!(engine_ref.bounce_project(source, 0));
    assert!(engine_ref.add_region(track_id, source, 0.0));
    assert!(engine_ref.add_region(track_id, source, 4.0));
    assert!(engine_ref.region_table_mirror_matches(track_id), "after adds");

    let layout: serde_json::Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let region_id = layout[0]["regions"][0]["id"].as_u64().expect("region id") as u32;

    assert!(engine_ref.set_region_gain(track_id, region_id, 0.5));
    assert!(engine_ref.region_table_mirror_matches(track_id), "after gain");
    assert!(engine_ref.set_region_muted(track_id, region_id, true));
    assert!(engine_ref.region_table_mirror_matches(track_id), "after mute");
    assert!(engine_ref.move_region(track_id, region_id, 2.0));
    assert!(engine_ref.region_table_mirror_matches(track_id), "after move");
    assert!(engine_ref.remove_region(track_id, region_id));
    assert!(engine_ref.region_table_mirror_matches(track_id), "after remove");

    let _ = std::fs::remove_file(&source_path);
}
