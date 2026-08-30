use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn region_edit_undo_redo_restores_audio_state() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
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
}
