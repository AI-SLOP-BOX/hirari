// The analysis reads now live entirely in Rust (engine_analysis_reads.rs):
// they sample the engine's own Rust-owned telemetry, meter, track-table and
// region stores through the engine's store-handle getters. These tests drive a
// live engine because the handles under test are the ones C++ installs.
struct AnalysisEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: cxx::UniquePtr<crate::ffi::AudioEngine>,
}

impl AnalysisEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        Self {
            _guard: guard,
            engine,
        }
    }

    fn engine(&self) -> &crate::ffi::AudioEngine {
        self.engine.as_ref().unwrap()
    }
}

#[test]
fn telemetry_meter_and_spectral_reads_sample_the_rust_stores() {
    let fixture = AnalysisEngine::new();
    let engine = fixture.engine();
    let core = crate::ffi::get_unified_engine(engine);

    // The block exists from engine construction, so the spectrum read returns
    // the full 512-bin published buffer even before audio has run.
    let spectrum = crate::engine_analysis_reads::spectrum(core);
    assert_eq!(spectrum.len(), 512);
    assert!(spectrum.iter().all(|bin| *bin == 0.0));

    // A fresh publication carries no track peaks.
    assert!(crate::engine_analysis_reads::track_peaks_l(core).is_empty());
    assert!(crate::engine_analysis_reads::track_peaks_r(core).is_empty());

    let meter = crate::engine_analysis_reads::master_meter_data(core);
    assert!(meter.integrated.is_finite() && meter.short_term.is_finite());
    assert!(meter.true_peak_l.is_finite() && meter.true_peak_r.is_finite());
    // The deleted C++ MeterData never assigned correlation; the read keeps
    // reporting zero so UI advice stays byte-identical.
    assert_eq!(meter.correlation, 0.0);

    let dashboard = crate::engine_analysis_reads::intelligence_dashboard_json(core);
    let parsed: serde_json::Value = serde_json::from_str(&dashboard).unwrap();
    assert_eq!(parsed["available"], serde_json::json!(true));
    assert_eq!(parsed["correlation"], serde_json::json!(0.0));

    let telemetry = crate::engine_analysis_reads::runtime_telemetry(engine, core);
    assert_eq!(telemetry.len(), 10);
    assert_eq!(telemetry[1], 0.0);
    assert_eq!(telemetry[2], 0.0);
    assert_eq!(telemetry[5], 0.0);
    assert_eq!(telemetry[7], 0.0, "no driver runs under the test guard");
    assert_eq!(telemetry[8], 0.0, "transport is stopped");
    assert_eq!(telemetry[9], core.get_playhead() as f32);
}

#[test]
fn song_structure_walks_the_published_graph_and_falls_back_to_thirty_seconds() {
    let fixture = AnalysisEngine::new();
    let engine = fixture.engine();
    let core = crate::ffi::get_unified_engine(engine);

    // No regions yet: the read falls back to one zero start and a thirty-second
    // project length, exactly as the deleted C++ accessor did.
    let baseline = crate::engine_analysis_reads::song_structure_json(engine, core);
    let parsed: serde_json::Value = serde_json::from_str(&baseline).unwrap();
    assert_eq!(parsed["available"], serde_json::json!(true));
    let baseline_sections = parsed["sections"].as_array().unwrap();
    assert!(
        !baseline_sections.is_empty(),
        "the fallback length must produce sections: {baseline}"
    );
    let fallback_end = core.get_sample_rate() as u64 * 30;
    assert_eq!(
        baseline_sections.last().unwrap()["end_sample"]
            .as_u64()
            .unwrap(),
        fallback_end
    );

    // A published region replaces the fallback length with its own span, read
    // through the Rust track table and region store without a C++ gather.
    let unique = format!(
        "hirari-song-structure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}.wav"));
    write_test_tone_wav(&wav_path, 44_100, 44_100);
    let track = engine.add_track(0);
    assert_ne!(track, 0);
    assert!(engine.add_region(track, wav_path.to_str().unwrap(), 0.0));

    let with_region = crate::engine_analysis_reads::song_structure_json(engine, core);
    let parsed: serde_json::Value = serde_json::from_str(&with_region).unwrap();
    let sections = parsed["sections"].as_array().unwrap();
    assert!(!sections.is_empty());
    let region_end = sections.last().unwrap()["end_sample"].as_u64().unwrap();
    assert_ne!(region_end, fallback_end, "the region span must replace the fallback length");
    assert!(region_end > 0);

    let _ = std::fs::remove_file(wav_path);
}
