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

#[test]
fn region_table_mirror_holds_after_every_region_mutator() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let e = engine.as_ref().expect("engine must initialize");
    let source_path = std::env::temp_dir().join(format!(
        "hirari-region-mirror-all-{}-source.wav",
        std::process::id()
    ));
    let source = source_path.to_str().unwrap();
    let tid = e.add_track(0);
    assert!(e.bounce_project(source, 0));
    assert!(e.add_region(tid, source, 0.0));
    let layout: serde_json::Value =
        serde_json::from_str(&e.get_project_layout_json().to_string()).unwrap();
    let rid = layout[0]["regions"][0]["id"].as_u64().expect("region id") as u32;

    // Whether or not a mutator accepts its input, the mirror must still equal
    // the authoritative list, so only the mirror is asserted.
    let check = |what: &str| {
        assert!(e.region_table_mirror_matches(tid), "mirror diverged after {what}");
    };
    let _ = e.set_region_fades(tid, rid, 0.1, 0.2);
    check("fades");
    let _ = e.set_region_reverse(tid, rid, true);
    check("reverse");
    let _ = e.set_region_trim(tid, rid, 0.1, 0.9);
    check("trim");
    let _ = e.set_region_warp_ratio(tid, rid, 1.5);
    check("warp ratio");
    let _ = e.set_region_pitch_preserve_warp(tid, rid, true);
    check("pitch preserve warp");
    let _ = e.set_region_pitch_semitones(tid, rid, 3.0);
    check("pitch semitones");
    let _ = e.set_region_loop_count(tid, rid, 2);
    check("loop count");
    let _ = e.set_region_sync_group(tid, rid, 7);
    check("sync group");
    let _ = e.set_region_comp_ranges(tid, rid, &[0, 100, 4, 4], true);
    check("comp ranges");
    let _ = e.set_region_range_edit(tid, rid, 10, 50, 0.5, 2, 2);
    check("range edit");
    let _ = e.clear_region_range_edits(tid, rid);
    check("clear range edits");
    let _ = e.set_region_trim(tid, rid, f32::NAN, 0.5);
    check("rejected trim");
    let _ = e.set_region_gain(tid, rid + 1000, 0.5);
    check("unknown region");

    let _ = e.remove_region(tid, rid);
    check("remove");
    e.undo();
    check("undo of remove");
    e.redo();
    check("redo of remove");
    e.undo();
    e.undo();
    check("repeated undo");

    let _ = std::fs::remove_file(&source_path);
}
