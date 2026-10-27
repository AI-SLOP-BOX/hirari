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

#[test]
fn track_rename_records_one_undo_entry_and_rejects_invalid_names() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let track_name = || {
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .expect("track must have a name")
    };
    let original = track_name();
    assert!(core.set_track_name(track_id, "Renamed"));
    assert_eq!(track_name(), "Renamed");
    // Renaming to the current value succeeds without recording an edit.
    assert!(core.set_track_name(track_id, "Renamed"));
    core.undo();
    assert_eq!(track_name(), original);
    core.redo();
    assert_eq!(track_name(), "Renamed");

    assert!(!core.set_track_name(track_id, ""));
    assert!(!core.set_track_name(track_id, "bad\0name"));
    assert!(!core.set_track_name(track_id, &"x".repeat(257)));
    assert_eq!(track_name(), "Renamed");
    core.undo();
    assert_eq!(track_name(), original);
}

#[test]
fn plugin_chain_edits_undo_through_the_native_engine() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_plugin(track_id, 0));
    assert!(core.add_plugin(track_id, 1));
    let plugin_types = || {
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("plugin_types"))
            .and_then(serde_json::Value::as_array)
            .map(|types| {
                types
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .collect::<Vec<_>>()
            })
            .expect("track must publish its plugin types")
    };
    assert_eq!(plugin_types(), vec![0, 1]);

    assert!(!core.get_plugin_bypass(track_id, 0));
    assert!(core.set_plugin_bypass(track_id, 0, true));
    assert!(core.get_plugin_bypass(track_id, 0));
    core.undo();
    assert!(!core.get_plugin_bypass(track_id, 0));
    core.redo();
    assert!(core.get_plugin_bypass(track_id, 0));
    // An out-of-range index is rejected and must not record an edit.
    assert!(!core.set_plugin_bypass(track_id, 9, true));

    let default_value = core.get_plugin_parameter(track_id, 0, 0);
    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.7));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.7).abs() < 0.001);
    core.undo();
    assert!((core.get_plugin_parameter(track_id, 0, 0) - default_value).abs() < 0.001);
    core.redo();
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.7).abs() < 0.001);
    // Non-finite values are rejected without touching the plugin or history.
    assert!(!core.set_plugin_parameter(track_id, 0, 0, f32::NAN));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.7).abs() < 0.001);

    assert!(core.move_plugin(track_id, 0, 1));
    assert_eq!(plugin_types(), vec![1, 0]);
    core.undo();
    assert_eq!(plugin_types(), vec![0, 1]);
    core.redo();
    assert_eq!(plugin_types(), vec![1, 0]);
    // Moving onto the same index is not an edit.
    assert!(!core.move_plugin(track_id, 0, 0));
    assert_eq!(plugin_types(), vec![1, 0]);
    // The rejected moves above must not have added history entries.
    core.undo();
    assert_eq!(plugin_types(), vec![0, 1]);
}

#[test]
fn track_mix_edits_undo_through_the_native_engine() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let track_json = || {
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .cloned()
            .expect("track must exist")
    };
    let f32_field = |name: &str| {
        track_json()
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .expect("field must be numeric")
    };
    let bool_field = |name: &str| {
        track_json()
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .expect("field must be boolean")
    };

    assert!((f32_field("volume") - 1.0).abs() < 0.0001);
    assert!(core.set_volume(track_id, 0.4));
    assert!((f32_field("volume") - 0.4).abs() < 0.0001);
    core.undo();
    assert!((f32_field("volume") - 1.0).abs() < 0.0001);
    core.redo();
    assert!((f32_field("volume") - 0.4).abs() < 0.0001);
    // Out-of-domain values must not reach the track or the undo history.
    assert!(!core.set_volume(track_id, 2.5));
    assert!((f32_field("volume") - 0.4).abs() < 0.0001);

    assert!((f32_field("pan")).abs() < 0.0001);
    core.set_track_pan(track_id, -0.25);
    assert!((f32_field("pan") + 0.25).abs() < 0.0001);
    core.undo();
    assert!(f32_field("pan").abs() < 0.0001);

    assert!(!bool_field("mute"));
    core.set_mute(track_id, true);
    assert!(bool_field("mute"));
    core.undo();
    assert!(!bool_field("mute"));

    assert!(!bool_field("solo"));
    core.set_solo(track_id, true);
    assert!(bool_field("solo"));
    core.undo();
    assert!(!bool_field("solo"));
}

#[test]
fn master_gain_edits_validate_and_undo_through_the_native_engine() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    assert!((core.master_gain() - 1.0).abs() < 0.0001);

    // Setting the current value again is accepted but adds no history entry:
    // one undo must land on the value from before the first edit.
    assert!(core.set_master_gain(0.5));
    assert!((core.master_gain() - 0.5).abs() < 0.0001);
    assert!(core.set_master_gain(0.5));
    core.undo();
    assert!((core.master_gain() - 1.0).abs() < 0.0001);
    core.redo();
    assert!((core.master_gain() - 0.5).abs() < 0.0001);

    for invalid in [-0.01, 2.01, f32::NAN, f32::INFINITY] {
        assert!(!core.set_master_gain(invalid));
        assert!((core.master_gain() - 0.5).abs() < 0.0001);
    }
    assert!(core.set_master_gain(2.0));
    assert!((core.master_gain() - 2.0).abs() < 0.0001);
}

/// Replacing a region's audio must take effect and round-trip through undo and
/// redo. The swap and the snapshots that restore it live in different layers,
/// so this pins the whole path: decode, payload hand-off, and the RESTORE undo
/// entries the host replays.
#[test]
fn replace_region_audio_swaps_the_source_and_round_trips_through_undo() {
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

    let region_state = || {
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        let region = layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| regions.first())
            .cloned()
            .expect("recording must publish a region");
        (
            region
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .expect("region id") as u32,
            region
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            region
                .get("source_length")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            region
                .get("len")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        )
    };
    let (region_id, before_path, before_source, before_len) = region_state();
    assert_eq!(before_source, 4096);

    let replacement = std::env::temp_dir().join(format!(
        "hirari-replace-{}-{}.wav",
        std::process::id(),
        track_id
    ));
    let tone = (0..2048)
        .map(|index| (index as f32 * 0.01).sin() * 0.3)
        .collect::<Vec<_>>();
    hirari_core_bridge::export::write_wav_pcm16(&replacement, &tone, 44_100, 1)
        .expect("replacement fixture must be writable");
    let replacement_path = replacement.to_str().unwrap().to_owned();

    assert!(core.replace_region_audio(track_id, region_id, &replacement_path));
    let (_, after_path, after_source, after_len) = region_state();
    assert_eq!(after_path, replacement_path);
    assert_eq!(after_source, 2048);
    assert_ne!(after_len, before_len);

    core.undo();
    let (_, undone_path, undone_source, undone_len) = region_state();
    assert_eq!(undone_path, before_path);
    assert_eq!(undone_source, before_source);
    assert_eq!(undone_len, before_len);

    core.redo();
    let (_, redone_path, redone_source, _) = region_state();
    assert_eq!(redone_path, replacement_path);
    assert_eq!(redone_source, after_source);

    let _ = std::fs::remove_file(replacement);
}
