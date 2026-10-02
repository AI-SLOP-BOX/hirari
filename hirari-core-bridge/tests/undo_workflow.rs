use hirari_core_bridge::HirariCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn region_edit_undo_redo_restores_audio_state() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    core.start_recording_capture(48_000.0, 2, 128, 0)
        .expect("capture must start");
    let audio = (0..4096)
        .flat_map(|index| {
            if index % 2 == 0 {
                [0.4, -0.4]
            } else {
                [0.2, -0.2]
            }
        })
        .collect::<Vec<_>>();
    core.append_recording_preview(&audio)
        .expect("capture must accept audio");
    core.commit_recording_capture_to_track(track_id, None)
        .expect("capture must publish");
    std::thread::yield_now();
    let layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be valid JSON");
    let region_id = layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("regions"))
        .and_then(serde_json::Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(serde_json::Value::as_u64)
        .expect("capture must publish a region") as u32;
    let region_gain = || {
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            .ok()
            .and_then(|layout| layout.as_array().cloned())
            .and_then(|tracks| {
                tracks.into_iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions").cloned())
            .and_then(|regions| regions.as_array().and_then(|items| items.first().cloned()))
            .and_then(|region| region.get("clip_gain").and_then(serde_json::Value::as_f64))
    };
    assert!((region_gain().unwrap_or_default() - 1.0).abs() < 0.0001);
    assert!(core.set_region_gain(track_id, region_id, 0.25));
    assert!((region_gain().unwrap_or_default() - 0.25).abs() < 0.0001);

    core.undo();
    assert!((region_gain().unwrap_or_default() - 1.0).abs() < 0.0001);
    core.redo();
    assert!((region_gain().unwrap_or_default() - 0.25).abs() < 0.0001);

    assert!(core.set_region_range_edit(track_id, region_id, 10, 100, 0.5, 4, 6));
    let range_count = || {
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            .ok()
            .and_then(|layout| layout.as_array().cloned())
            .and_then(|tracks| {
                tracks.into_iter().find(|t| {
                    t.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions").cloned())
            .and_then(|regions| regions.as_array().and_then(|items| items.first().cloned()))
            .and_then(|region| {
                region
                    .get("range_edits")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default()
    };
    assert_eq!(range_count(), 1);
    core.undo();
    assert_eq!(range_count(), 0);
    core.redo();
    assert_eq!(range_count(), 1);
    assert!(core.set_region_range_edit(track_id, region_id, 200, 300, 0.8, 0, 0));
    assert_eq!(range_count(), 2);
    assert!(core.clear_region_range_edits(track_id, region_id));
    assert_eq!(range_count(), 0);
    core.undo();
    assert_eq!(range_count(), 2);
    core.redo();
    assert_eq!(range_count(), 0);
}

/// A region range edit must store the exact values that were requested.
///
/// The Rust kernel appends the new entry through a raw pointer into reserved
/// capacity and reports the resulting count. If the caller then grows the
/// vector with resize(), the new element is value-initialized and the stored
/// edit is silently replaced by a zeroed struct, while the call still reports
/// success. That made every range edit read back as {0,0,0,0,0}, which in turn
/// made undo/redo of range edits unrecoverable.
#[test]
fn range_edit_stores_requested_values_exactly() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    core.start_recording_capture(48_000.0, 2, 128, 0)
        .expect("capture must start");
    let audio = (0..4096)
        .flat_map(|index| {
            if index % 2 == 0 {
                [0.4, -0.4]
            } else {
                [0.2, -0.2]
            }
        })
        .collect::<Vec<_>>();
    core.append_recording_preview(&audio)
        .expect("capture must accept audio");
    core.commit_recording_capture_to_track(track_id, None)
        .expect("capture must publish");
    let layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be valid JSON");
    let region_id = layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("regions"))
        .and_then(serde_json::Value::as_array)
        .and_then(|regions| regions.first())
        .and_then(|region| region.get("id"))
        .and_then(serde_json::Value::as_u64)
        .expect("capture must publish a region") as u32;

    assert!(core.set_region_range_edit(track_id, region_id, 10, 100, 0.5, 4, 6));

    let region_edits = || -> Vec<serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            .ok()
            .and_then(|layout| layout.as_array().cloned())
            .and_then(|tracks| {
                tracks.into_iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions").cloned())
            .and_then(|regions| regions.as_array().and_then(|items| items.first().cloned()))
            .and_then(|region| region.get("range_edits").cloned())
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
    };

    let stored = region_edits();
    assert_eq!(stored.len(), 1, "one range edit must be stored");
    let edit = &stored[0];
    assert_eq!(edit.get("start").and_then(|v| v.as_u64()), Some(10));
    assert_eq!(edit.get("end").and_then(|v| v.as_u64()), Some(100));
    assert_eq!(
        edit.get("gain").and_then(|v| v.as_f64()),
        Some(0.5),
        "the requested gain must be stored, not a default or zeroed value"
    );
    assert_eq!(edit.get("fade_in").and_then(|v| v.as_u64()), Some(4));
    assert_eq!(edit.get("fade_out").and_then(|v| v.as_u64()), Some(6));
}
