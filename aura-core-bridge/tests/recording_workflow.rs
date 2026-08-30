use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn recording_capture_publishes_and_bounces_through_public_api() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let token = format!(
        "aura-record-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    );
    let render_path = std::env::temp_dir().join(format!("{token}.wav"));

    core.arm_recording_capture(48_000.0, 2, 128)
        .expect("recording capture must arm");
    assert_eq!(core.recording_lifecycle_label(), "Armed");
    core.start_recording_capture(48_000.0, 2, 128, 0)
        .expect("recording capture must start");
    core.append_recording_preview(&[0.25, -0.25, 0.1, -0.1])
        .expect("audio block must be accepted");
    let track_id = core.add_track(0);
    let frames = core
        .commit_recording_capture_to_track(track_id, None)
        .expect("recording take must publish");
    assert!(frames > 0);
    assert_eq!(core.recording_lifecycle_label(), "Committed");

    let layout = core.get_project_layout_json();
    assert!(layout.contains("regions"));
    assert!(core.bounce_project(render_path.to_str().unwrap(), 0));

    let bytes = std::fs::read(&render_path).expect("bounce output must exist");
    assert!(bytes.len() >= 44);
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let _ = std::fs::remove_file(render_path);
}

#[test]
fn recording_lifecycle_rejects_duplicate_start_stop_and_invalid_channels() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");

    assert!(core.start_recording_capture(48_000.0, 0, 128, 0).is_err());
    core.start_recording_capture(48_000.0, 1, 128, 0)
        .expect("first recording must start");
    assert!(core.start_recording_capture(48_000.0, 1, 128, 0).is_err());
    core.append_recording_preview(&[0.2, 0.1])
        .expect("mono audio block must be accepted");
    core.stop_recording_preview()
        .expect("first stop must finalize the capture");
    assert!(core.stop_recording_preview().is_err());
}

#[test]
fn native_input_polling_never_reports_success_without_a_real_device() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    if !core.is_silent_audio_fallback() {
        return;
    }
    core.start_recording_capture(48_000.0, 2, 1024, 0)
        .expect("recording session should start before device polling");
    let error = core
        .poll_recording_capture()
        .expect_err("silent fallback must not report a captured input block");
    assert!(error.to_string().contains("audio input is unavailable"));
    let _ = core.stop_recording_preview();
}

#[test]
fn audio_input_health_snapshot_matches_driver_boundary() {
    let _guard = native_engine_test_guard();
    let core = aura_core_bridge::AuraCore::new().expect("core must initialize");
    let (ready, silent, dropped) = core.audio_input_health();
    assert_eq!(silent, core.is_silent_audio_fallback());
    assert_eq!(ready, core.is_audio_device_ready());
    assert!(dropped < u64::MAX);
    if silent {
        assert!(!ready);
    }
}

#[test]
fn multichannel_streaming_capture_persists_disk_spool_before_commit() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1_024));
    core.start_recording_capture(48_000.0, 4, 512, 0)
        .expect("four-channel capture must start");
    let generation_before_reconfigure = core.audio_config_generation();
    assert!(!core.apply_audio_config(96_000, 512));
    assert!(core.recording_preview_active());
    assert_eq!(
        core.audio_config_generation(),
        generation_before_reconfigure
    );

    for block in 0..16 {
        let audio = (0..(512 * 4))
            .map(|sample| {
                let channel = sample % 4;
                let frame = sample / 4;
                0.05 * (1.0 + channel as f32) * if (frame + block) % 2 == 0 { 1.0 } else { -1.0 }
            })
            .collect::<Vec<_>>();
        core.append_recording_preview(&audio)
            .expect("streaming block must be accepted");
    }

    core.stop_recording_preview()
        .expect("multichannel capture must stop");
    assert!(core.apply_audio_config(96_000, 512));
    assert!(core.audio_config_generation() > generation_before_reconfigure);
    assert_eq!(core.recording_lifecycle_label(), "Stopped");
    let spool = core
        .recording_capture_spool_path()
        .expect("stopped capture must expose a disk spool");
    assert!(std::fs::metadata(&spool).unwrap().len() > 44);

    // Stopping finalizes the disk take; commit is intentionally a separate
    // operation for active captures and must not be double-invoked here.
    assert_eq!(core.recording_lifecycle_label(), "Stopped");
    remove_test_file(spool);
}

#[test]
fn crash_recovered_recording_can_be_imported_as_a_region() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let source = std::env::temp_dir().join(format!(
        "aura-capture-recovery-{}-{}.wav.part",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    aura_core_bridge::export::write_wav_pcm16(&source, &[0.25, -0.25], 48_000, 1)
        .expect("recovery source must be a valid WAV");
    let track_id = core.add_track(0);
    let frames = core
        .recover_recording_spool_to_track(source.to_str().unwrap(), track_id)
        .expect("recovery should import a valid spool");
    assert_eq!(frames, 2);
    assert!(core.get_project_layout_json().contains("regions"));
    let _ = std::fs::remove_file(source);
}

#[test]
fn recording_keeps_multiple_disk_takes_and_switches_active_take() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1024));

    core.start_recording_capture(48_000.0, 2, 256, 0)
        .expect("first take must start");
    core.append_recording_preview(&vec![0.15; 128])
        .expect("first take must accept audio");
    core.stop_recording_preview().expect("first take must stop");
    let first_spool = core
        .recording_capture_spool_path()
        .expect("first take must have a disk spool");

    core.start_recording_capture(48_000.0, 2, 256, 0)
        .expect("second take must start");
    core.append_recording_preview(&vec![-0.35; 128])
        .expect("second take must accept audio");
    core.stop_recording_preview()
        .expect("second take must stop");
    let second_spool = core
        .recording_capture_spool_path()
        .expect("second take must have a disk spool");

    assert_eq!(core.recording_take_count(), 2);
    assert_eq!(core.active_recording_take(), 1);
    assert_ne!(first_spool, second_spool);
    let second_waveform = core.recording_capture_waveform(1);
    assert_eq!(second_waveform.len(), 1);
    assert!((second_waveform[0] - 0.35).abs() < 0.01);

    assert!(core.select_recording_take(0));
    assert_eq!(core.active_recording_take(), 0);
    let first_waveform = core.recording_capture_waveform(1);
    assert_eq!(first_waveform.len(), 1);
    assert!((first_waveform[0] - 0.15).abs() < 0.01);
    assert!(!core.select_recording_take(2));

    let _ = std::fs::remove_file(first_spool);
    let _ = std::fs::remove_file(second_spool);
}
