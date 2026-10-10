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
