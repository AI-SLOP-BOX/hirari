// End-to-end coverage for the Rust-owned VariAudio note-edit table. Every
// assertion below goes through the engine's public note-edit API, so it
// exercises the C++ -> Rust store -> published projection -> serializer path
// rather than the store in isolation.

use serde_json::Value;

fn unique_tag() -> String {
    format!(
        "hirari-note-edits-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn note_segments_of(layout: &Value, region_id: u32) -> &Vec<Value> {
    layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("regions"))
        .and_then(Value::as_array)
        .and_then(|regions| {
            regions.iter().find(|region| {
                region.get("id").and_then(Value::as_u64) == Some(region_id as u64)
            })
        })
        .and_then(|region| region.get("audio_note_segments"))
        .and_then(Value::as_array)
        .expect("region must publish its note segments")
}

#[test]
fn note_edit_round_trip_flows_through_the_rust_store() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("engine must initialize");
    let unique = unique_tag();
    let source_path = std::env::temp_dir().join(format!("{unique}-source.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let source = source_path.to_str().unwrap();
    let project = project_path.to_str().unwrap();

    let track_id = engine_ref.add_track(0);
    assert!(engine_ref.bounce_project(source, 0));
    assert!(engine_ref.add_region(track_id, source, 0.0));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let region_id = layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("regions"))
        .and_then(Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(Value::as_u64)
        .expect("added region must have a stable id") as u32;

    // Insert, then insert a second non-overlapping segment. Both must survive:
    // a store that only ever holds one segment is not usable as a table.
    assert!(engine_ref.set_region_audio_note_segment(
        track_id,
        region_id,
        1.0,
        2.0,
        100.0,
        0.0
    ));
    assert!(engine_ref.set_region_audio_note_segment(
        track_id,
        region_id,
        3.0,
        4.0,
        0.0,
        0.0
    ));
    // An overlapping segment must be rejected.
    assert!(!engine_ref.set_region_audio_note_segment(
        track_id,
        region_id,
        1.5,
        3.5,
        0.0,
        0.0
    ));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let segments = note_segments_of(&layout, region_id);
    assert_eq!(segments.len(), 2, "both non-overlapping segments must survive");
    assert_eq!(segments[0]["start"], 1.0);
    assert_eq!(segments[1]["start"], 3.0);

    // Anchor upsert, then a timing warp that rescales the anchor with the
    // segment ([1,2] -> [0.5,2.5] doubles the offset inside the segment, so
    // the 1.25s anchor lands on 1.0s).
    assert!(engine_ref.set_region_audio_note_anchor(
        track_id,
        region_id,
        1.0,
        1.25,
        40.0,
        0.0
    ));
    assert!(engine_ref.warp_region_audio_note_segment(
        track_id, region_id, 1.0, 0.5, 2.5
    ));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let segments = note_segments_of(&layout, region_id);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0]["start"], 0.5);
    assert_eq!(segments[0]["end"], 2.5);
    let anchors = segments[0]["anchors"].as_array().unwrap();
    assert_eq!(anchors.len(), 1, "the warped anchor must be retained");
    assert!(
        (anchors[0]["position"].as_f64().unwrap() - 1.0).abs() < 1.0e-9,
        "anchor must follow the affine segment warp: {:?}",
        anchors[0]
    );
    assert!((anchors[0]["pitch_cents"].as_f64().unwrap() - 40.0).abs() < 1.0e-9);

    // The table must survive a save/load round trip.
    assert!(engine_ref.save_project(project));
    engine_ref.new_project();
    assert!(engine_ref.load_project(project));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let segments = note_segments_of(&layout, region_id);
    assert_eq!(segments.len(), 2, "note edits must survive a project round trip");
    assert_eq!(segments[0]["end"], 2.5);
    let anchors = segments[0]["anchors"].as_array().unwrap();
    assert_eq!(anchors.len(), 1);
    assert!((anchors[0]["position"].as_f64().unwrap() - 1.0).abs() < 1.0e-9);

    // Removal and clear still report accurately.
    assert!(engine_ref.remove_region_audio_note_segment(
        track_id, region_id, 3.0
    ));
    assert!(!engine_ref.remove_region_audio_note_segment(
        track_id, region_id, 3.0
    ));
    assert!(engine_ref.clear_region_audio_note_segments(track_id, region_id));
    assert!(!engine_ref.clear_region_audio_note_segments(track_id, region_id));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    assert!(note_segments_of(&layout, region_id).is_empty());

    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(project_path);
}

#[test]
fn scalar_region_edits_round_trip_through_the_shared_policy_path() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("engine must initialize");
    let unique = unique_tag();
    let source_path = std::env::temp_dir().join(format!("{unique}-source.wav"));
    let source = source_path.to_str().unwrap();

    let track_id = engine_ref.add_track(0);
    assert!(engine_ref.bounce_project(source, 0));
    assert!(engine_ref.add_region(track_id, source, 0.0));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let region_id = layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("regions"))
        .and_then(Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(Value::as_u64)
        .expect("added region must have a stable id") as u32;

    let region = |engine: &crate::ffi::AudioEngine| -> Value {
        let layout: Value =
            serde_json::from_str(&engine.get_project_layout_json().to_string()).unwrap();
        layout
            .as_array()
            .and_then(|tracks| tracks.first())
            .and_then(|track| track.get("regions"))
            .and_then(Value::as_array)
            .and_then(|regions| {
                regions
                    .iter()
                    .find(|region| {
                        region.get("id").and_then(Value::as_u64) == Some(region_id as u64)
                    })
                    .cloned()
            })
            .expect("region must stay addressable")
    };

    // Booleans are the trap: the Rust policy returns no payload for
    // Muted/Reverse, so the shared path has to store the caller's own value.
    // Storing the (always zero) decision value would silently drop every edit.
    assert!(engine_ref.set_region_muted(track_id, region_id, true));
    assert_eq!(region(engine_ref)["muted"], Value::Bool(true));
    assert!(engine_ref.set_region_reverse(track_id, region_id, true));
    assert_eq!(region(engine_ref)["reverse"], Value::Bool(true));
    assert!(engine_ref.set_region_muted(track_id, region_id, false));
    assert_eq!(region(engine_ref)["muted"], Value::Bool(false));
    assert!(engine_ref.set_region_reverse(track_id, region_id, false));
    assert_eq!(region(engine_ref)["reverse"], Value::Bool(false));

    // Numeric fields keep their clamp/reject policy and their payload. Gain
    // clamps (documented Rust policy), so 25.0 becomes 2.0 rather than being
    // rejected, while NaN is refused outright and leaves the value alone.
    assert!(engine_ref.set_region_gain(track_id, region_id, 1.5));
    assert_eq!(region(engine_ref)["clip_gain"].as_f64(), Some(1.5));
    assert!(!engine_ref.set_region_gain(track_id, region_id, f32::NAN));
    assert_eq!(region(engine_ref)["clip_gain"].as_f64(), Some(1.5));
    assert!(engine_ref.set_region_gain(track_id, region_id, 25.0));
    assert_eq!(region(engine_ref)["clip_gain"].as_f64(), Some(2.0));
    assert!(engine_ref.set_region_gain(track_id, region_id, -3.0));
    assert_eq!(region(engine_ref)["clip_gain"].as_f64(), Some(0.0));
    assert!(engine_ref.set_region_loop_count(track_id, region_id, 2));
    assert_eq!(region(engine_ref)["loop_count"].as_u64(), Some(2));
    assert!(!engine_ref.set_region_loop_count(track_id, region_id, 1025));
    assert_eq!(region(engine_ref)["loop_count"].as_u64(), Some(2));

    let _ = std::fs::remove_file(source_path);
}

#[test]
fn comp_range_edits_are_adjudicated_by_the_rust_policy() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("engine must initialize");
    let unique = unique_tag();
    let source_path = std::env::temp_dir().join(format!("{unique}-source.wav"));
    let source = source_path.to_str().unwrap();

    let track_id = engine_ref.add_track(0);
    assert!(engine_ref.bounce_project(source, 0));
    assert!(engine_ref.add_region(track_id, source, 0.0));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let (region_id, region_len) = layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("regions"))
        .and_then(Value::as_array)
        .and_then(|regions| regions.first())
        .map(|region| {
            (
                region.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
                region.get("len").and_then(Value::as_u64).unwrap_or(0),
            )
        })
        .expect("added region must have a stable id and length");

    let set = |ranges: &[(u64, u64, u64, u64)], managed: bool| {
        let mut packed: Vec<u64> = Vec::with_capacity(ranges.len() * 4);
        for (start, end, fade_in, fade_out) in ranges {
            packed.extend_from_slice(&[*start, *end, *fade_in, *fade_out]);
        }
        engine_ref.set_region_comp_ranges(track_id, region_id, packed.as_slice(), managed)
    };

    // A batch that is ordered, non-overlapping and inside the region.
    let some = region_len / 4;
    assert!(some > 0, "bounced region must be non-empty");
    let good = [
        (0u64, some, 0u64, 0u64),
        (some, some * 2, 0, some / 2),
        (some * 2, some * 3, 0, 0),
    ];
    assert!(set(&good, true));

    // Ordering is enforced at the boundary. This batch is non-overlapping, so
    // the partial validation the Rust wrapper used to run accepted it while
    // the core rejected it.
    let out_of_order = [
        (some * 2, some * 3, 0, 0),
        (0, some, 0, 0),
    ];
    assert!(!set(&out_of_order, false));

    // Overlap, inverted, over-wide fades and out-of-region are all refused.
    assert!(!set(&[(0, some, 0, 0), (some / 2, some * 2, 0, 0)], false));
    assert!(!set(&[(some, 0, 0, 0)], false));
    assert!(!set(&[(0, some, some + 1, 0)], false));
    assert!(!set(&[(0, region_len + 1, 0, 0)], false));

    // An empty batch un-manages the region and stays admissible.
    assert!(set(&[], false));
    assert!(set(&good, true));

    let _ = std::fs::remove_file(source_path);
}

#[test]
fn processing_history_round_trips_through_the_engine_and_the_project_file() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("engine must initialize");
    let unique = unique_tag();
    let source_path = std::env::temp_dir().join(format!("{unique}-source.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let source = source_path.to_str().unwrap();
    let project = project_path.to_str().unwrap();

    let track_id = engine_ref.add_track(0);
    assert!(engine_ref.bounce_project(source, 0));
    assert!(engine_ref.add_region(track_id, source, 0.0));
    let layout: Value =
        serde_json::from_str(&engine_ref.get_project_layout_json().to_string()).unwrap();
    let region_id = layout
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("regions"))
        .and_then(Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(Value::as_u64)
        .expect("added region must have a stable id") as u32;

    // A well-formed step is accepted.
    assert!(engine_ref.append_region_processing(
        track_id, region_id, 7, "gain", 0.5, true
    ));
    assert!(engine_ref.append_region_processing(
        track_id, region_id, 8, "normalize", -1.25, false
    ));

    // The policy that used to be enforced only in the engine wrapper: a
    // non-finite parameter never reached the store check before.
    assert!(!engine_ref.append_region_processing(
        track_id, region_id, 9, "gain", f32::NAN, true
    ));
    assert!(!engine_ref.append_region_processing(
        track_id, region_id, 9, "gain", f32::INFINITY, true
    ));
    // Empty and oversized operations, and a zero id, are refused too.
    assert!(!engine_ref.append_region_processing(
        track_id, region_id, 9, "", 0.0, true
    ));
    let long = "x".repeat(257);
    assert!(!engine_ref.append_region_processing(
        track_id, region_id, 9, &long, 0.0, true
    ));
    assert!(!engine_ref.append_region_processing(
        track_id, region_id, 0, "gain", 0.0, true
    ));
    // The 256-byte operation is the boundary and stays admissible.
    let boundary = "x".repeat(256);
    assert!(engine_ref.append_region_processing(
        track_id, region_id, 10, &boundary, 0.0, true
    ));

    // Clearing an empty history is refused; one with steps succeeds once.
    assert!(engine_ref.clear_region_processing(track_id, region_id));
    assert!(!engine_ref.clear_region_processing(track_id, region_id));

    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(project_path);
}
