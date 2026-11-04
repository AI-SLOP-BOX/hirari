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

/// Splitting and duplicating a region run through the Rust region-edit host:
/// the host clones and stores the payload, and the undo entries are whole-region
/// restore snapshots instead of C++ lambdas. This pins the full path so a
/// regression in either layer shows up as a missing or extra region.
#[test]
fn split_and_duplicate_regions_round_trip_through_undo() {
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

    let regions = || -> Vec<(u32, u64, u64)> {
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|region| {
                        Some((
                            region.get("id").and_then(serde_json::Value::as_u64)? as u32,
                            region.get("start").and_then(serde_json::Value::as_u64)?,
                            region.get("len").and_then(serde_json::Value::as_u64)?,
                        ))
                    })
                    .collect()
            })
            .expect("track must publish its regions")
    };

    let original = regions();
    assert_eq!(original.len(), 1);
    let (region_id, region_start, region_len) = original[0];
    assert_eq!(region_start, 0);
    assert!(region_len > 2400);

    // 0.1 beat is the split point; ask the tempo map for the sample position
    // so the test does not depend on the project sample rate.
    let split_sample = core.beats_to_samples(0.1);
    assert!(split_sample > 0 && split_sample < region_len);
    assert!(core.split_region(track_id, region_id, 0.1));
    let split = regions();
    assert_eq!(split.len(), 2);
    assert_eq!(split.iter().map(|region| region.2).sum::<u64>(), region_len);
    let right = split
        .iter()
        .find(|region| region.0 != region_id)
        .copied()
        .expect("split must add a right-hand region");
    assert_eq!(right.1, split_sample);
    assert_eq!(right.2, region_len - split_sample);

    core.undo();
    assert_eq!(regions(), original);
    core.redo();
    assert_eq!(regions(), split);

    let duplicate_sample = core.beats_to_samples(0.5);
    let duplicate_id = core.duplicate_region(track_id, region_id, 0.5);
    assert_ne!(duplicate_id, 0);
    let duplicated = regions();
    assert_eq!(duplicated.len(), split.len() + 1);
    let copy = duplicated
        .iter()
        .find(|region| region.0 == duplicate_id)
        .copied()
        .expect("duplicate must be present");
    assert_eq!(copy.1, duplicate_sample);
    let left_len = split
        .iter()
        .find(|region| region.0 == region_id)
        .map(|region| region.2)
        .expect("the left-hand region must remain");
    assert_eq!(copy.2, left_len);
    core.undo();
    assert_eq!(regions(), split);
    core.redo();
    assert_eq!(regions(), duplicated);
}

/// Preset save/load policy now lives in the Rust plugin-edit host: it checks
/// the plugin type, swaps the stored state and records the before/after pair.
/// A mismatched or missing preset must leave the plugin and the history alone.
#[test]
fn plugin_preset_load_swaps_state_and_undoes_through_the_plugin_host() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    assert!(core.add_plugin(track_id, 0));

    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.3));
    let saved = std::env::temp_dir().join(format!(
        "hirari-preset-{}-{}.hrp",
        std::process::id(),
        track_id
    ));
    let saved_path = saved.to_str().unwrap().to_owned();
    assert!(core.save_plugin_preset(track_id, 0, &saved_path));
    assert!(saved.exists(), "save must write the preset file");

    assert!(core.set_plugin_parameter(track_id, 0, 0, 0.8));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.8).abs() < 0.001);

    assert!(core.load_plugin_preset(track_id, 0, &saved_path));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.3).abs() < 0.01);
    core.undo();
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.8).abs() < 0.001);
    core.redo();
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.3).abs() < 0.01);

    // A preset for another plugin type is rejected before anything changes,
    // and the rejected load must not add a history entry.
    let mismatched = std::env::temp_dir().join(format!(
        "hirari-preset-mismatch-{}-{}.hrp",
        std::process::id(),
        track_id
    ));
    let mismatched_path = mismatched.to_str().unwrap().to_owned();
    assert!(unsafe {
        hirari_core_bridge::plugin_preset_file::hirari_plugin_preset_file_save(
            mismatched_path.as_ptr(),
            mismatched_path.len(),
            1,
            [9_u8, 9, 9].as_ptr(),
            3,
        )
    });
    assert!(!core.load_plugin_preset(track_id, 0, &mismatched_path));
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.3).abs() < 0.01);
    core.undo();
    assert!((core.get_plugin_parameter(track_id, 0, 0) - 0.8).abs() < 0.001);

    assert!(!core.load_plugin_preset(track_id, 0, ""));
    assert!(!core.load_plugin_preset(track_id, 9, &saved_path));
    assert!(!core.save_plugin_preset(track_id, 0, ""));

    let _ = std::fs::remove_file(saved);
    let _ = std::fs::remove_file(mismatched);
}

/// Expression-map edits now admit and apply through the Rust edit host: the
/// packed rows and JSON are compared against the stored snapshot, an identical
/// write records nothing, and undo/redo replay the full snapshot by track id.
#[test]
fn expression_map_edits_undo_through_the_rust_edit_host() {
    use hirari_core_bridge::project_contracts::{ExpressionMapEntry, ExpressionMapOutput};

    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let entry = |value: u8| ExpressionMapEntry {
        articulation_id: 1,
        name: "Test".to_owned(),
        keyswitch_pitch: None,
        channel: 0,
        group: 0,
        outputs: vec![ExpressionMapOutput::ControlChange {
            controller: 1,
            value,
        }],
        off_outputs: Vec::new(),
        transition_off_outputs: Vec::new(),
    };
    let map_json = |core: &HirariCore| -> serde_json::Value {
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).expect("layout is json");
        layout[0]["expression_map"].clone()
    };

    let before = map_json(&core);
    assert!(core.set_midi_expression_map(track_id, vec![entry(64)], 0));
    let after = map_json(&core);
    assert_ne!(before, after);

    // A repeated identical write is a no-op: the single undo below must jump
    // straight back to the pre-edit map.
    assert!(core.set_midi_expression_map(track_id, vec![entry(64)], 0));
    core.undo();
    assert_eq!(map_json(&core), before);
    core.redo();
    assert_eq!(map_json(&core), after);

    // Editing a specific articulation gives the action a distinct name, so it
    // records its own entry instead of coalescing into the first edit, and it
    // round-trips by snapshot.
    let second = ExpressionMapEntry {
        articulation_id: 2,
        name: "Second".to_owned(),
        keyswitch_pitch: None,
        channel: 0,
        group: 0,
        outputs: vec![ExpressionMapOutput::ControlChange {
            controller: 2,
            value: 32,
        }],
        off_outputs: Vec::new(),
        transition_off_outputs: Vec::new(),
    };
    assert!(core.set_midi_expression_map(track_id, vec![second], 2));
    let both = map_json(&core);
    core.undo();
    assert_eq!(map_json(&core), after);
    core.redo();
    assert_eq!(map_json(&core), both);
}

#[test]
fn track_lifecycle_edits_round_trip_through_the_native_history() {
    let _guard = native_engine_test_guard();
    let core = HirariCore::new().expect("core must initialize");
    let layout = || -> Vec<u32> {
        let json: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).expect("layout must be JSON");
        json.as_array()
            .map(|tracks| {
                tracks
                    .iter()
                    .filter_map(|track| track.get("id").and_then(serde_json::Value::as_u64))
                    .map(|id| id as u32)
                    .collect()
            })
            .unwrap_or_default()
    };
    let routes = || -> Vec<(u32, u32)> {
        let json: serde_json::Value =
            serde_json::from_str(&core.audio_routes_json()).expect("routes must be JSON");
        json.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|route| {
                        let source = route.get("source_id")?.as_u64()?;
                        let destination = route.get("destination_id")?.as_u64()?;
                        Some((source as u32, destination as u32))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let source = core.add_track(0);
    let destination = core.add_track(0);
    assert!(core.set_route_gain(source, destination, 0.5, true));
    let index = layout()
        .iter()
        .position(|id| *id == destination)
        .expect("a new track must be in the graph");

    // Removing a track captures its send, so undo returns both the track at the
    // index it left and the route it took with it.
    assert!(core.remove_track(destination));
    assert!(!layout().contains(&destination));
    assert!(!routes().contains(&(source, destination)));
    core.undo();
    assert_eq!(
        layout().iter().position(|id| *id == destination),
        Some(index)
    );
    assert!(routes().contains(&(source, destination)));
    core.redo();
    assert!(!layout().contains(&destination));
    assert!(!routes().contains(&(source, destination)));

    // An add records one entry: its undo drops the track, its redo puts it back.
    let added = core.add_track(0);
    core.undo();
    assert!(!layout().contains(&added));
    core.redo();
    assert!(layout().contains(&added));

    // A duplicate is a separate id, and removing it again is one undo step.
    let copy = core.duplicate_track(source);
    assert!(copy != 0 && copy != source);
    assert!(layout().contains(&copy));
    core.undo();
    assert!(!layout().contains(&copy));
    core.redo();
    assert!(layout().contains(&copy));
}
