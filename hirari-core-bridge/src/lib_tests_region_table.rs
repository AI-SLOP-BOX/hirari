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
        serde_json::from_str(&engine_ref.get_core().get_project_layout_json().to_string()).unwrap();
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

// End-to-end coverage for the Rust-owned split/duplicate policy: the engine
// still owns the payload, but every geometry, fade, source-offset and local
// window decision comes back from `region_split.rs`.
#[test]
fn split_and_duplicate_apply_the_rust_policy_through_the_engine() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let e = engine.as_ref().expect("engine must initialize");
    let source_path = std::env::temp_dir().join(format!(
        "hirari-region-split-{}-source.wav",
        std::process::id()
    ));
    let source = source_path.to_str().unwrap();
    let tid = e.add_track(0);
    assert!(e.bounce_project(source, 0));
    assert!(e.add_region(tid, source, 0.0));

    let layout: serde_json::Value =
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
    let region = &layout[0]["regions"][0];
    let rid = region["id"].as_u64().expect("region id") as u32;
    let start = region["start"].as_u64().expect("start");
    let len = region["len"].as_u64().expect("len");
    assert!(len >= 8, "bounced source must have a usable length, got {len}");
    assert!(e.get_sample_rate() > 0.0);

    assert!(e.set_region_fades(tid, rid, 0.05, 0.1));
    assert!(e.set_region_audio_note_segment(tid, rid, 0.0, 0.75, 100.0, -50.0));
    assert!(e.set_region_audio_note_anchor(tid, rid, 0.0, 0.25, 200.0, 50.0));

    let split_offset = len / 2;
    let right_id = e.split_region_with_right_id(tid, rid, (start + split_offset) as f64);
    assert_ne!(right_id, 0, "split must produce a right region");
    assert!(e.region_table_mirror_matches(tid), "mirror after split");

    let layout: serde_json::Value =
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
    let regions = layout[0]["regions"].as_array().expect("regions");
    assert_eq!(regions.len(), 2);
    let left = regions
        .iter()
        .find(|region| region["id"].as_u64() == Some(rid as u64))
        .expect("left region");
    let right = regions
        .iter()
        .find(|region| region["id"].as_u64() == Some(right_id as u64))
        .expect("right region");
    assert_eq!(left["start"].as_u64(), Some(start));
    assert_eq!(left["len"].as_u64(), Some(split_offset));
    assert_eq!(right["start"].as_u64(), Some(start + split_offset));
    assert_eq!(right["len"].as_u64(), Some(len - split_offset));
    // Clip-edge fades stay on the outer edges only.
    assert!(left["fade_in_samples"].as_u64().unwrap() > 0);
    assert_eq!(left["fade_out_samples"].as_u64(), Some(0));
    assert_eq!(right["fade_in_samples"].as_u64(), Some(0));
    assert!(right["fade_out_samples"].as_u64().unwrap() > 0);
    // Source spans follow the split in timeline order.
    assert_eq!(left["source_length"].as_u64(), Some(split_offset));
    assert_eq!(right["source_length"].as_u64(), Some(len - split_offset));
    assert_eq!(right["source_offset"].as_u64(), Some(split_offset));

    // Note edits window at the cut and the right half moves back to zero.
    let left_notes = left["audio_note_segments"].as_array().expect("left notes");
    assert_eq!(left_notes.len(), 1);
    assert_eq!(left_notes[0]["start"].as_f64(), Some(0.0));
    let split_seconds = split_offset as f64 / e.get_sample_rate();
    let left_note_end = left_notes[0]["end"].as_f64().expect("left note end");
    assert!((left_note_end - split_seconds).abs() < 1e-6);
    let right_notes = right["audio_note_segments"].as_array().expect("right notes");
    assert_eq!(right_notes.len(), 1);
    assert_eq!(right_notes[0]["start"].as_f64(), Some(0.0));
    let right_note_end = right_notes[0]["end"].as_f64().expect("right note end");
    assert!((right_note_end - (0.75 - split_seconds)).abs() < 1e-6);

    // Undo/redo restore one/two regions through the existing undo wiring.
    e.undo();
    assert!(e.region_table_mirror_matches(tid), "mirror after split undo");
    let layout: serde_json::Value =
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
    let restored = layout[0]["regions"].as_array().expect("regions");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0]["id"].as_u64(), Some(rid as u64));
    assert_eq!(restored[0]["len"].as_u64(), Some(len));
    e.redo();
    assert!(e.region_table_mirror_matches(tid), "mirror after split redo");
    let layout: serde_json::Value =
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
    assert_eq!(layout[0]["regions"].as_array().map(Vec::len), Some(2));

    // Duplicate keeps the trimmed source span and backs the base up by the
    // timeline trim that Rust computed from `baseSourceOffset`.
    let duplicate_start = start + len * 2;
    let duplicate_id = e.duplicate_region(tid, right_id, duplicate_start);
    assert_ne!(duplicate_id, 0, "duplicate must produce a region");
    assert!(e.region_table_mirror_matches(tid), "mirror after duplicate");
    let layout: serde_json::Value =
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
    let duplicate = layout[0]["regions"]
        .as_array()
        .expect("regions")
        .iter()
        .find(|region| region["id"].as_u64() == Some(duplicate_id as u64))
        .expect("duplicate region");
    assert_eq!(duplicate["start"].as_u64(), Some(duplicate_start));
    assert_eq!(duplicate["len"].as_u64(), Some(len - split_offset));
    // Re-adding the right half at split time normalised its base to the
    // trimmed source, so the duplicate keeps that (zero) trim.
    assert_eq!(duplicate["base_start"].as_u64(), Some(duplicate_start));
    assert_eq!(duplicate["source_offset"].as_u64(), Some(split_offset));

    e.undo();
    assert!(e.region_table_mirror_matches(tid), "mirror after duplicate undo");

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
        serde_json::from_str(&e.get_core().get_project_layout_json().to_string()).unwrap();
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
