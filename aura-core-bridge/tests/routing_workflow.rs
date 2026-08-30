use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn sidechain_tap_points_are_accepted_by_the_core_graph() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let source = core.add_track(0);
    let destination = core.add_track(0);
    assert!(core.add_plugin(destination, 0));

    for tap_point in 0..=2 {
        assert!(core.set_sidechain_link(source, destination, 0, tap_point, true));
        assert!(core.set_sidechain_link(source, destination, 0, tap_point, false));
    }
    assert!(!core.set_sidechain_link(source, destination, 0, 3, true));
    assert!(!core.set_sidechain_link(source, source, 0, 0, true));
}

#[test]
fn sidechain_cycle_is_rejected_and_unlink_releases_the_graph() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let a = core.add_track(0);
    let b = core.add_track(0);
    let c = core.add_track(0);
    assert!(core.add_plugin(b, 0));
    assert!(core.add_plugin(c, 0));

    assert!(core.set_sidechain_link(a, b, 0, 2, true));
    assert!(core.set_sidechain_link(b, c, 0, 2, true));
    assert!(!core.set_sidechain_link(c, a, 0, 2, true));

    assert!(core.set_sidechain_link(a, b, 0, 2, false));
    assert!(core.set_sidechain_link(c, a, 0, 2, true));
    assert!(core.set_sidechain_link(c, a, 0, 2, false));
    assert!(core.set_sidechain_link(b, c, 0, 2, false));
}

#[test]
fn sidechain_route_survives_project_save_and_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let source = core.add_track(0);
    let destination = core.add_track(0);
    assert!(core.add_plugin(destination, 0));
    assert!(core.set_sidechain_link(source, destination, 0, 1, true));
    assert!(core.has_sidechain_link(source, destination, 0));
    let layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).expect("layout must be valid JSON");
    let saved_route = layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(destination as u64)
            })
        })
        .and_then(|track| track.get("sidechain_routes"))
        .and_then(serde_json::Value::as_array)
        .and_then(|routes| routes.first())
        .expect("destination layout must expose its sidechain route");
    assert_eq!(
        saved_route
            .get("source_id")
            .and_then(serde_json::Value::as_u64),
        Some(source as u64)
    );
    assert_eq!(
        saved_route
            .get("destination_id")
            .and_then(serde_json::Value::as_u64),
        Some(destination as u64)
    );
    assert_eq!(
        saved_route
            .get("plugin_index")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert_eq!(
        saved_route
            .get("tap_point")
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );

    let path = std::env::temp_dir().join(format!(
        "aura-sidechain-roundtrip-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    assert!(core.save_project(path.to_string_lossy().as_ref()));
    assert!(core.set_sidechain_link(source, destination, 0, 1, false));
    assert!(!core.has_sidechain_link(source, destination, 0));
    assert!(core.load_project(path.to_string_lossy().as_ref()));
    assert!(core.has_sidechain_link(source, destination, 0));
    assert!(core.set_sidechain_link(source, destination, 0, 1, false));
    std::fs::remove_file(path).expect("project cleanup must succeed");
}

#[test]
fn pdc_compensates_a_shorter_path_for_limiter_lookahead() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let delayed = core.add_track(0);
    let dry = core.add_track(0);
    assert!(core.apply_audio_config(48_000, 1_024));
    assert!(core.add_plugin(delayed, 0));
    let single_plugin_latency = core.get_track_latency_ms(delayed);
    assert!(single_plugin_latency > 0.0);
    assert!(core.add_plugin(delayed, 0));
    let double_plugin_latency = core.get_track_latency_ms(delayed);
    assert!(
        double_plugin_latency > single_plugin_latency,
        "serial plugin latency must accumulate"
    );
    assert!(core.set_plugin_bypass(delayed, 1, true));
    let bypassed_latency = core.get_track_latency_ms(delayed);
    assert!(
        (bypassed_latency - single_plugin_latency).abs() < 0.01,
        "bypassing one plugin must recalculate PDC latency"
    );
    assert!(core.set_plugin_bypass(delayed, 1, false));
    assert!(core.remove_plugin(delayed, 1));
    let removed_latency = core.get_track_latency_ms(delayed);
    assert!((removed_latency - single_plugin_latency).abs() < 0.01);
    assert!(core.set_route(delayed, dry, true));

    let delayed_latency = core.get_track_latency_ms(delayed);
    let dry_compensation = core.get_track_pdc_compensation_ms(dry);
    assert!(delayed_latency > 0.0, "limiter look-ahead must be reported");
    assert!(dry_compensation > 0.0, "shorter path must receive PDC");
    assert!(dry_compensation <= delayed_latency + 0.01);

    // PDC is stored in samples and exposed in milliseconds, so changing the
    // project rate must update the displayed value without changing the
    // graph's sample-domain compensation.
    assert!(core.apply_audio_config(96_000, 1_024));
    let pdc_at_96k = core.get_track_pdc_compensation_ms(dry);
    assert!(pdc_at_96k.is_finite() && pdc_at_96k > 0.0);
    assert!(pdc_at_96k < dry_compensation * 0.6);
}

#[test]
fn deleting_a_sidechain_source_removes_the_core_link() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let source = core.add_track(0);
    let destination = core.add_track(0);
    assert!(core.set_sidechain_link(source, destination, 0, 0, true));
    assert!(core.has_sidechain_link(source, destination, 0));
    assert!(core.remove_track(source));
    assert!(!core.has_sidechain_link(source, destination, 0));
}

#[test]
fn bus_and_multiple_sidechain_sources_are_kept_as_distinct_routes() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1_024));
    let bus = core.add_track(3);
    let source = core.add_track(0);
    let destination = core.add_track(0);
    assert!(core.add_plugin(destination, 0));
    assert!(core.add_plugin(destination, 0));
    assert!(core.set_sidechain_link(bus, destination, 0, 0, true));
    assert!(core.set_sidechain_link(source, destination, 1, 2, true));
    assert!(core.has_sidechain_link(bus, destination, 0));
    assert!(core.has_sidechain_link(source, destination, 1));

    let duplicate = core.duplicate_track(source);
    assert!(duplicate > 0 && duplicate != source);
    assert!(!core.has_sidechain_link(duplicate, destination, 1));
}

#[test]
fn sidechain_processing_reduces_program_signal_under_external_trigger() {
    let _guard = native_engine_test_guard();
    let mut compressor =
        aura_core_bridge::sidechain_compressor::SidechainCompressorEngine::new(48_000.0);
    compressor.set_params(0.1, 10.0, 0.1, 20.0);
    let mut left = vec![0.8_f32; 512];
    let mut right = vec![0.8_f32; 512];
    let side_left = vec![1.0_f32; 512];
    let side_right = vec![1.0_f32; 512];
    compressor.process(&mut left, &mut right, Some((&side_left, &side_right)));
    assert!(compressor.current_gain < 1.0);
    assert!(left
        .iter()
        .all(|sample| sample.is_finite() && *sample < 0.8));
    assert!(right
        .iter()
        .all(|sample| sample.is_finite() && *sample < 0.8));
    assert!(compressor.audit_sidechain_compressor());
}

#[test]
fn native_process_block_reaches_the_cpp_graph_and_sanitizes_buffers() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 128));
    assert!(!core.audio_driver_status().is_empty());

    let mut left = vec![f32::NAN; 128];
    let mut right = vec![f32::INFINITY; 128];
    assert!(core.process_audio_block(&mut left, &mut right));
    assert!(left
        .iter()
        .chain(right.iter())
        .all(|sample| sample.is_finite()));

    core.set_test_tone(true);
    left.fill(0.0);
    right.fill(0.0);
    assert!(core.process_audio_block(&mut left, &mut right));
    assert!(left.iter().any(|sample| sample.abs() > 0.001));
    assert_eq!(left, right, "the native test tone must be stereo coherent");
}

#[test]
fn native_process_block_rejects_malformed_buffer_shapes() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    let mut left = vec![0.0_f32; 64];
    let mut right = vec![0.0_f32; 63];
    assert!(!core.process_audio_block(&mut left, &mut right));
    assert!(!core.process_audio_block(&mut [], &mut []));
    let mut oversized_left = vec![0.0_f32; 16_385];
    let mut oversized_right = vec![0.0_f32; 16_385];
    oversized_left.fill(0.25);
    oversized_right.fill(-0.5);
    assert!(!core.process_audio_block(&mut oversized_left, &mut oversized_right));
    assert!(oversized_left.iter().all(|sample| *sample == 0.25));
    assert!(oversized_right.iter().all(|sample| *sample == -0.5));

    // The native graph and Rust bridge share a 16K upper bound.  Exercise the
    // exact boundary so a future change cannot accidentally make the bridge
    // stricter (or looser) than the configured native engine.
    let mut maximum_left = vec![0.0_f32; 16_384];
    let mut maximum_right = vec![0.0_f32; 16_384];
    assert!(core.process_audio_block(&mut maximum_left, &mut maximum_right));
    assert!(maximum_left
        .iter()
        .chain(maximum_right.iter())
        .all(|sample| sample.is_finite()));
}

#[test]
fn unavailable_audio_is_reported_as_a_boundary_not_as_ready() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    let status = core.audio_driver_status();
    assert!(matches!(
        status.as_str(),
        "running" | "initialized" | "start-failed" | "stopped" | "silent-fallback" | "unavailable"
    ));
    if core.is_silent_audio_fallback() {
        assert_ne!(status, "ready");
        assert!(!core.try_set_playing(true));
    }
}

#[test]
fn sidechain_audio_effect_is_measurable_against_no_trigger_baseline() {
    let _guard = native_engine_test_guard();
    let mut with_trigger =
        aura_core_bridge::sidechain_compressor::SidechainCompressorEngine::new(48_000.0);
    let mut without_trigger =
        aura_core_bridge::sidechain_compressor::SidechainCompressorEngine::new(48_000.0);
    with_trigger.set_params(0.2, 10.0, 0.1, 20.0);
    without_trigger.set_params(0.2, 10.0, 0.1, 20.0);

    let program = vec![0.1_f32; 2_048];
    let trigger = vec![1.0_f32; 2_048];
    let mut triggered_left = program.clone();
    let mut triggered_right = program.clone();
    let mut baseline_left = program.clone();
    let mut baseline_right = program.clone();
    with_trigger.process(
        &mut triggered_left,
        &mut triggered_right,
        Some((&trigger, &trigger)),
    );
    without_trigger.process(&mut baseline_left, &mut baseline_right, None);

    let rms = |samples: &[f32]| {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    };
    let triggered_rms = rms(&triggered_left);
    let baseline_rms = rms(&baseline_left);
    assert!(triggered_rms.is_finite() && baseline_rms.is_finite());
    assert!(
        triggered_rms < baseline_rms * 0.8,
        "external sidechain must change rendered program level: triggered={triggered_rms}, baseline={baseline_rms}"
    );
    assert!(with_trigger.audit_sidechain_compressor());
    assert!(without_trigger.audit_sidechain_compressor());
}
