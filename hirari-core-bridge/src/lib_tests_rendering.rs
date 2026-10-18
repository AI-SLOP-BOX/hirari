#[test]
fn ui_telemetry_generation_cache_key_advances_on_project_mutations() {
    // `update_analysis_telemetry` caches the expensive content hash keyed by
    // the O(1) state revision instead of serializing the project every tick.
    // That cache is only correct if the revision actually advances on the
    // mutations the waveform path cares about: without it, a restored or
    // edited project would keep matching requests against a stale generation
    // and show the wrong peaks (or none).
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new_offline().expect("offline core initializes");
    let baseline = core.project_state_revision();
    let track = core.add_audio_track();
    assert_ne!(track, 0);
    let after_add = core.project_state_revision();
    assert_ne!(baseline, after_add, "adding a track must advance the revision");

    assert!(core.set_track_name(track, "Cache Probe"));
    let after_rename = core.project_state_revision();
    assert_ne!(
        after_add, after_rename,
        "renaming a track must advance the revision"
    );

    // A region edit must also advance the revision; region add/remove/move
    // publish snapshots through the same choke point.
    core.start_recording_capture(48_000.0, 2, 128, 0)
        .expect("capture must start");
    let input = vec![0.1f32; 256];
    core.append_recording_preview(&input)
        .expect("capture must accept audio");
    core.commit_recording_capture_to_track(track, None)
        .expect("capture must publish a region");
    let after_region = core.project_state_revision();
    assert_ne!(
        after_rename, after_region,
        "publishing a region must advance the revision"
    );
}

/// Writes a minimal valid stereo-16 PCM WAV for tests: `frames` samples
/// at `sample_rate`, both channels carrying a 440 Hz tone. Hand-rolled
/// writers elsewhere in this file previously emitted a 2-byte fmt-chunk
/// size (valid is 4 bytes) and under-declared the data chunk, which the
/// importer correctly rejected — so fixture audio goes through this one
/// constructor.
#[cfg(test)]
fn write_test_tone_wav(path: &std::path::Path, sample_rate: u32, frames: u32) {
    let mut wav = Vec::with_capacity(44 + frames as usize * 4);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(frames * 4).to_le_bytes());
    for i in 0..frames {
        let sample =
            (i as f32 / sample_rate as f32 * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.5;
        let pcm = (sample.clamp(-1.0, 1.0) * 32_767.0) as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    std::fs::write(path, &wav).expect("fixture wav must be writable");
}

#[test]
fn file_region_bounce_is_audible_in_all_engine_configurations() {
    // A file-backed region must render audibly no matter how the engine is
    // configured. Bounce used a fixed 1024-sample render block while tracks
    // are prepared at the engine block size, and Track::canProcess() skips any
    // block larger than that, so an unconfigured engine (512-sample default)
    // bounced full-length silence. Pin audibility across unconfigured,
    // matched-rate, and rate-mismatched setups.
    let _guard = native_engine_test_guard();
    for (label, rate, file_rate) in [
        ("unconfigured", None, 48_000u32),
        ("matched-48000", Some(48_000u32), 48_000u32),
        ("mismatched-44100-engine-48k-file", Some(44_100u32), 48_000u32),
    ] {
        let core = crate::HirariCore::new().expect("core must initialize");
        if let Some(rate) = rate {
            assert!(core.apply_audio_config(rate, 1_024));
        }
        let unique = format!(
            "hirari-bounce-config-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let wav_path = std::env::temp_dir().join(format!("{unique}.wav"));
        write_test_tone_wav(&wav_path, file_rate, 4_800);
        let track = core.add_audio_track();
        assert_ne!(track, 0);
        assert!(
            core.add_region(track, wav_path.to_str().unwrap(), 0.0),
            "region import must succeed ({label})"
        );
        let out = std::env::temp_dir().join(format!("{unique}-bounce.wav"));
        assert!(core.bounce_project(out.to_str().unwrap(), 0));
        let bytes = std::fs::read(&out).expect("bounce must exist");
        let peak = bytes
            .get(44..)
            .map(|data| {
                data.chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                    .fold(0.0f32, |a, v| a.max(v.abs()))
            })
            .unwrap_or(-1.0);
        assert!(
            peak > 0.01,
            "bounce must be audible ({label}, peak {peak})"
        );
        let _ = std::fs::remove_file(&wav_path);
        let _ = std::fs::remove_file(&out);
    }
}

#[test]
fn cli_render_path_bounce_is_audible() {
    // Replicates `hirari render mix`: a fresh offline core (no device ever
    // opened, so the engine block size is the 512-sample default), load a v2
    // project, then bounce. The bounce loop used a fixed 1024-sample render
    // block while tracks are prepared at the engine block size, and
    // Track::canProcess() silently skips any block larger than that — so every
    // track came out as full-length silence. This pins that a just-loaded
    // project bounces audibly without any interactive reconfiguration.
    let _guard = native_engine_test_guard();
    let unique = format!(
        "hirari-cli-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}-src.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    write_test_tone_wav(&wav_path, 48_000, 48_000);
    let project_text = project_path.to_string_lossy().into_owned();
    let builder = crate::HirariCore::new_offline().expect("offline core must initialize");
    let track = builder.add_audio_track();
    assert_ne!(track, 0);
    assert!(builder.add_region(track, wav_path.to_str().unwrap(), 0.0));
    builder
        .save_project_v2(&project_text, "CLI Render", 120.0)
        .expect("save must succeed");

    let core = crate::HirariCore::new_offline().expect("offline core must initialize");
    // A fresh engine reports the default 512-sample block; that is smaller
    // than the bounce loop's old 1024-sample step, which is the exact
    // configuration that used to render silence.
    assert_eq!(core.get_buffer_size(), 512, "probe must start unconfigured");
    core.load_project_v2(&project_text)
        .expect("load must succeed");
    let out = std::env::temp_dir().join(format!("{unique}-bounce.wav"));
    assert!(core.bounce_project(out.to_str().unwrap(), 0));
    let bytes = std::fs::read(&out).expect("bounce must exist");
    let peak = bytes
        .get(44..)
        .map(|data| {
            data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                .fold(0.0f32, |a, v| a.max(v.abs()))
        })
        .unwrap_or(0.0);
    assert!(
        peak > 0.01,
        "CLI render path must bounce audibly right after load (peak {peak})"
    );
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&project_path);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn native_region_fades_survive_a_v2_save_and_reload() {
    // Saving wrote fades as absolute samples while the loader passed them
    // straight into the normalized native setter, so every natively-created
    // region (default 64-sample fades) failed to reload with "native engine
    // failed to restore region envelope". This round-trips one region
    // through save and load and requires the load to succeed with the
    // region intact.
    let _guard = native_engine_test_guard();
    let unique = format!(
        "hirari-fade-roundtrip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}.wav"));
    let samples = 4_800i32;
    let mut wav = Vec::with_capacity(44 + samples as usize * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&192_000u32.to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples as u32 * 2).to_le_bytes());
    for i in 0..samples {
        let sample = (i as f32 / 48_000.0 * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.4;
        let pcm = (sample.clamp(-1.0, 1.0) * 32_767.0) as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    std::fs::write(&wav_path, &wav).expect("fixture wav must be writable");

    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let project_text = project_path.to_string_lossy().into_owned();
    let core = crate::HirariCore::new().expect("core must initialize");
    let track = core.add_audio_track();
    assert_ne!(track, 0);
    assert!(core.set_track_name(track, "Fade Probe"));
    assert!(core.add_region(track, wav_path.to_str().unwrap(), 0.0));
    core.save_project_v2(&project_text, "Fade Roundtrip", 120.0)
        .expect("v2 save must succeed");

    let restored = crate::HirariCore::new().expect("restored core must initialize");
    restored
        .load_project_v2(&project_text)
        .expect("v2 load must succeed");
    let layout: serde_json::Value =
        serde_json::from_str(&restored.get_project_layout_json()).expect("layout must be JSON");
    let regions: usize = layout
        .as_array()
        .map(|tracks| {
            tracks
                .iter()
                .filter_map(|track| track.get("regions").and_then(|regions| regions.as_array()))
                .map(|regions| regions.len())
                .sum()
        })
        .unwrap_or(0);
    assert_eq!(regions, 1, "the region must survive the round trip");

    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&project_path);
    for suffix in [
        ".midi.json",
        ".midi.json.bak.1",
        ".comping.json",
        ".midi-events.json",
    ] {
        let _ = std::fs::remove_file(format!("{project_text}{suffix}"));
    }
}

#[test]
fn wav_publication_validator_accepts_rf64_ds64_data_size() {
    let mut wav = vec![0u8; 76];
    wav[0..4].copy_from_slice(b"RF64");
    wav[8..12].copy_from_slice(b"WAVE");
    wav[12..16].copy_from_slice(b"ds64");
    wav[16..20].copy_from_slice(&16u32.to_le_bytes());
    wav[28..36].copy_from_slice(&8u64.to_le_bytes());
    wav[36..40].copy_from_slice(b"fmt ");
    wav[40..44].copy_from_slice(&16u32.to_le_bytes());
    wav[44..46].copy_from_slice(&1u16.to_le_bytes());
    wav[46..48].copy_from_slice(&2u16.to_le_bytes());
    wav[48..52].copy_from_slice(&48_000u32.to_le_bytes());
    wav[56..58].copy_from_slice(&4u16.to_le_bytes());
    wav[58..60].copy_from_slice(&16u16.to_le_bytes());
    wav[60..64].copy_from_slice(b"data");
    wav[64..68].copy_from_slice(&u32::MAX.to_le_bytes());
    wav[68..76].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(crate::valid_pcm_or_float_wav(&wav));
}

#[test]
fn wav_publication_validator_handles_unknown_odd_chunks_and_rejects_bad_rf64() {
    let mut wav = vec![0u8; 46 + 8 + 2];
    wav[0..4].copy_from_slice(b"RIFF");
    wav[8..12].copy_from_slice(b"WAVE");
    wav[12..16].copy_from_slice(b"fmt ");
    wav[16..20].copy_from_slice(&16u32.to_le_bytes());
    wav[20..22].copy_from_slice(&1u16.to_le_bytes());
    wav[22..24].copy_from_slice(&1u16.to_le_bytes());
    wav[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    wav[32..34].copy_from_slice(&2u16.to_le_bytes());
    wav[34..36].copy_from_slice(&16u16.to_le_bytes());
    wav[36..40].copy_from_slice(b"JUNK");
    wav[40..44].copy_from_slice(&1u32.to_le_bytes());
    wav[44] = 0x7f;
    // The odd-sized chunk consumes a padding byte before the data chunk.
    wav[46..50].copy_from_slice(b"data");
    wav[50..54].copy_from_slice(&2u32.to_le_bytes());
    assert!(crate::valid_pcm_or_float_wav(&wav));

    let mut bad_rf64 = wav;
    bad_rf64[0..4].copy_from_slice(b"RF64");
    bad_rf64[50..54].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(!crate::valid_pcm_or_float_wav(&bad_rf64));
}

#[test]
fn region_edit_boundaries_are_rejected_at_the_ffi_boundary() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    assert!(!engine.is_null());
    let engine_ref = engine.as_ref().unwrap();

    assert!(!engine_ref.set_region_gain(0, 0, f32::NAN));
    assert!(!engine_ref.set_region_gain(0, 0, 25.0));
    assert!(!engine_ref.set_region_fades(0, 0, -0.1, 0.2));
    assert!(!engine_ref.set_region_fades(0, 0, 0.2, f32::INFINITY));
    assert!(!engine_ref.set_region_trim(0, 0, 0.8, 0.2));
    assert!(!engine_ref.set_region_trim(0, 0, 0.0, 1.1));
    assert!(!engine_ref.set_region_warp_ratio(0, 0, 0.1));
    assert!(!engine_ref.set_region_warp_ratio(0, 0, 4.1));
    assert!(!engine_ref.set_region_pitch_semitones(0, 0, 49.0));
    assert!(!engine_ref.set_region_pitch_semitones(0, 0, f32::NEG_INFINITY));
    assert!(!engine_ref.set_region_loop_count(0, 0, 0));
    assert!(!engine_ref.set_region_loop_count(0, 0, 1025));
}

#[test]
fn vca_assignment_and_gain_are_exposed_through_the_native_bridge() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    let engine_ref = engine.as_ref().expect("native engine handle");
    let track_id = engine_ref.add_track(0);
    assert_ne!(track_id, 0);
    let group_id = 9001;
    engine_ref.clear_vca_groups();
    assert!(engine_ref.add_vca_group(group_id, 1.0));
    assert!(engine_ref.assign_track_to_vca(track_id, group_id));
    assert_eq!(engine_ref.get_vca_track_gain(track_id), 1.0);
    assert!(engine_ref.set_vca_group_gain(group_id, 0.5));
    assert_eq!(engine_ref.get_vca_track_gain(track_id), 0.5);
    let snapshot: serde_json::Value =
        serde_json::from_str(engine_ref.get_vca_snapshot_json().as_str())
            .expect("VCA snapshot must be JSON");
    assert!(snapshot
        .as_array()
        .unwrap()
        .iter()
        .any(|group| { group["id"] == group_id && group["gain"] == 0.5 }));
    assert!(!engine_ref.add_vca_group(group_id, f32::NAN));
}

#[test]
fn vca_groups_round_trip_through_project_v2() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let track_id = core.add_audio_track();
    assert_ne!(track_id, 0);
    let group_id = 9002;
    assert!(core.add_vca_group(group_id, 0.75));
    assert!(core.assign_track_to_vca(track_id, group_id));
    let path = std::env::temp_dir().join(format!(
        "hirari-vca-roundtrip-{}-{}.hirari",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    core.save_project_v2(path.to_str().unwrap(), "VCA Test", 120.0)
        .expect("VCA project save must succeed");
    core.clear_vca_groups();
    let load_result = core.load_project_v2(path.to_str().unwrap());
    assert!(
        load_result.is_ok(),
        "VCA project load failed: {load_result:?}"
    );
    let snapshot: serde_json::Value =
        serde_json::from_str(&core.get_vca_snapshot_json()).expect("VCA snapshot must be JSON");
    assert!(snapshot
        .as_array()
        .unwrap()
        .iter()
        .any(|group| { group["id"] == group_id && group["gain"] == 0.75 }));
    let _ = std::fs::remove_file(path);
}

#[test]
fn malformed_project_is_rejected_without_replacing_current_tracks() {
    let _guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    assert!(!engine.is_null());
    let engine_ref = engine.as_ref().unwrap();
    let unique = format!(
        "hirari-load-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let valid_path = std::env::temp_dir().join(format!("{}.hirari", unique));
    let corrupt_path = std::env::temp_dir().join(format!("{}-corrupt.hirari", unique));
    let valid = valid_path.to_str().unwrap();
    let corrupt = corrupt_path.to_str().unwrap();

    assert!(engine_ref.save_project(valid));
    let mut bytes = std::fs::read(&valid_path).unwrap();
    bytes.push(0xA5); // trailing data must invalidate the project
    std::fs::write(&corrupt_path, bytes).unwrap();

    assert!(!engine_ref.load_project(corrupt));

    let _ = std::fs::remove_file(valid_path);
    let _ = std::fs::remove_file(corrupt_path);
}

#[test]
fn sidecar_failure_rolls_back_native_project_state() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let unique = format!(
        "hirari-sidecar-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{unique}.hirari"));
    let sidecar = std::path::PathBuf::from(format!("{}.midi-events.json", project.display()));
    let project_text = project.to_str().unwrap();

    let saved_track = core.add_track(0);
    assert_ne!(saved_track, 0);
    assert!(core.set_track_name(saved_track, "saved-state"));
    assert!(core.save_project(project_text));

    let current_track = core.add_track(0);
    assert_ne!(current_track, 0);
    assert!(core.set_track_name(current_track, "current-state"));
    let before = core.get_project_layout_json();
    std::fs::write(&sidecar, b"{malformed").unwrap();

    assert!(!core.load_project(project_text));
    assert_eq!(core.get_project_layout_json(), before);

    let _ = std::fs::remove_file(project);
    let _ = std::fs::remove_file(sidecar);
}

#[test]
fn ui_midi_sidecar_failure_rolls_back_the_project_save() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let unique = format!(
        "hirari-ui-midi-save-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let project = std::env::temp_dir().join(format!("{unique}.hirari"));
    let project_text = project.to_str().unwrap();
    let ui_sidecar = std::path::PathBuf::from(format!("{project_text}.midi.json"));

    let saved_track = core.add_track(0);
    assert_ne!(saved_track, 0);
    assert!(core.set_track_name(saved_track, "saved-state"));
    assert!(core.save_project(project_text));
    let previous_project = std::fs::read(&project).unwrap();

    let current_track = core.add_track(0);
    assert_ne!(current_track, 0);
    assert!(core.set_track_name(current_track, "current-state"));
    std::fs::create_dir(&ui_sidecar).unwrap();

    assert!(!core.save_project_with_ui_midi_notes(project_text, "[]"));
    assert_eq!(std::fs::read(&project).unwrap(), previous_project);
    let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json()).unwrap();
    assert_eq!(layout.as_array().unwrap().len(), 2);
    assert_eq!(layout[1]["name"], "current-state");

    let _ = std::fs::remove_dir_all(&ui_sidecar);
    assert!(core.save_project_with_ui_midi_notes(project_text, "[]"));
    assert_eq!(std::fs::read_to_string(&ui_sidecar).unwrap(), "[]");
    assert!(core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":2}]"#));
    assert!(core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":3}]"#));
    let backup_one: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{project_text}.midi.json.bak.1")).unwrap())
            .unwrap();
    let backup_two: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{project_text}.midi.json.bak.2")).unwrap())
            .unwrap();
    assert_eq!(backup_one["notes"], serde_json::json!([{"generation":2}]));
    assert!(backup_one["hirari_project_checksum"].as_u64().is_some());
    assert!(backup_one["hirari_project_modified_ns"].as_str().is_some());
    assert_eq!(backup_two["notes"], serde_json::json!([]));
    assert!(backup_two["hirari_project_checksum"].as_u64().is_some());
    assert!(backup_two["hirari_project_modified_ns"].as_str().is_some());

    let current_sidecar = std::fs::read(&ui_sidecar).unwrap();
    let comping_sidecar = std::path::PathBuf::from(format!("{project_text}.comping.json"));
    std::fs::remove_file(&comping_sidecar).unwrap();
    std::fs::create_dir(&comping_sidecar).unwrap();
    assert!(!core.save_project_with_ui_midi_notes(project_text, r#"[{"generation":4}]"#));
    assert_eq!(std::fs::read(&ui_sidecar).unwrap(), current_sidecar);
    assert!(serde_json::from_slice::<Vec<serde_json::Value>>(&current_sidecar).is_ok());
    std::fs::remove_dir_all(&comping_sidecar).unwrap();

    let _ = std::fs::remove_file(&ui_sidecar);
    let _ = std::fs::remove_file(&project);
    for suffix in [
        ".bak.1",
        ".bak.2",
        ".bak.3",
        ".comping.json",
        ".comping.json.bak.1",
        ".comping.json.bak.2",
        ".comping.json.bak.3",
        ".midi-events.json",
        ".midi-events.json.bak.1",
        ".midi-events.json.bak.2",
        ".midi-events.json.bak.3",
        ".midi.json",
        ".midi.json.bak.1",
        ".midi.json.bak.2",
        ".midi.json.bak.3",
    ] {
        let _ = std::fs::remove_file(format!("{project_text}{suffix}"));
    }
}

#[test]
fn test_non_finite_tempo_input_is_ignored() {
    let _guard = tempo_test_guard();
    let _engine_guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    assert!(!engine.is_null());
    let engine_ref = engine.as_ref().unwrap();
    // The native engine is a process-wide singleton; establish a local
    // baseline so this test is independent of parallel test ordering.
    engine_ref.set_tempo(120.0);
    let initial_tempo = engine_ref.get_tempo();

    engine_ref.set_tempo(f32::NAN);
    assert_eq!(engine_ref.get_tempo(), initial_tempo);
    engine_ref.set_tempo(f32::INFINITY);
    assert_eq!(engine_ref.get_tempo(), initial_tempo);
    engine_ref.set_tempo(f32::NEG_INFINITY);
    assert_eq!(engine_ref.get_tempo(), initial_tempo);
}

#[test]
fn test_out_of_range_tempo_is_bounded() {
    let _guard = tempo_test_guard();
    let _engine_guard = native_engine_test_guard();
    let engine = crate::ffi::new_audio_engine();
    assert!(!engine.is_null());
    let engine_ref = engine.as_ref().unwrap();

    engine_ref.set_tempo(0.0);
    assert_eq!(engine_ref.get_tempo(), 20.0);
    engine_ref.set_tempo(301.0);
    assert_eq!(engine_ref.get_tempo(), 300.0);
}

#[test]
fn project_diagnostic_api_preserves_structured_error_codes() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let empty: serde_json::Value = serde_json::from_str(&core.save_project_diagnostic_json(" "))
        .expect("empty path diagnostic must be JSON");
    assert_eq!(empty["code"], "invalid_path");

    let missing = std::env::temp_dir().join(format!(
        "hirari-diagnostic-missing-{}-{}.hirari",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let missing_json: serde_json::Value =
        serde_json::from_str(&core.load_project_diagnostic_json(missing.to_str().unwrap()))
            .expect("missing path diagnostic must be JSON");
    assert_eq!(missing_json["code"], "project_not_found");

    let saved = std::env::temp_dir().join(format!(
        "hirari-diagnostic-save-{}-{}.hirari",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let saved_json: serde_json::Value =
        serde_json::from_str(&core.save_project_diagnostic_json(saved.to_str().unwrap()))
            .expect("successful save diagnostic must be JSON");
    assert_eq!(saved_json["ok"], true);
    assert_eq!(saved_json["path"], saved.to_string_lossy().as_ref());
    assert!(saved_json["generation"].as_u64().is_some());
    let _ = std::fs::remove_file(&saved);
    let _ = std::fs::remove_file(format!("{}.comping.json", saved.display()));
    let _ = std::fs::remove_file(format!("{}.midi.json", saved.display()));
}

#[test]
fn project_load_rollback_failure_has_a_distinct_diagnostic_code() {
    use crate::{project_load_failure_code, project_load_rollback_complete, ProjectLoadFailure};

    assert!(!project_load_rollback_complete(false, true, true));
    assert!(!project_load_rollback_complete(true, false, true));
    assert!(!project_load_rollback_complete(true, true, false));
    assert!(project_load_rollback_complete(true, true, true));
    assert_eq!(
        project_load_failure_code(ProjectLoadFailure::RollbackFailed),
        "project_load_rollback_failed"
    );
}

#[test]
fn recording_and_midi_diagnostics_reject_invalid_commands_structurally() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");

    let take: serde_json::Value =
        serde_json::from_str(&core.select_recording_take_diagnostic_json(4))
            .expect("recording diagnostic must be JSON");
    assert_eq!(take["code"], "recording_session_inactive");

    let swing: serde_json::Value =
        serde_json::from_str(&core.apply_midi_swing_diagnostic_json(f32::NAN, 2.0))
            .expect("swing diagnostic must be JSON");
    assert_eq!(swing["code"], "invalid_midi_swing");
    assert_eq!(swing["retryable"], false);

    let humanize: serde_json::Value =
        serde_json::from_str(&core.humanize_midi_diagnostic_json(9.0, 127, 42))
            .expect("humanize diagnostic must be JSON");
    assert_eq!(humanize["code"], "invalid_midi_humanize");

    let malformed: serde_json::Value =
        serde_json::from_str(&core.set_midi_events_diagnostic_json("not-json"))
            .expect("MIDI snapshot diagnostic must be JSON");
    assert_eq!(malformed["code"], "invalid_midi_snapshot");

    let invalid_event: serde_json::Value =
        serde_json::from_str(&core.set_midi_events_diagnostic_json(
            r#"[{"beat":0.0,"channel":16,"kind":{"ControlChange":{"controller":1,"value":1}}}]"#,
        ))
        .expect("invalid MIDI event diagnostic must be JSON");
    assert_eq!(invalid_event["code"], "invalid_midi_event");

    let invalid_move: serde_json::Value =
        serde_json::from_str(&core.move_midi_notes_range_diagnostic_json(0, 0, 100, -1))
            .expect("MIDI move diagnostic must be JSON");
    assert_eq!(invalid_move["code"], "invalid_midi_move_range");

    let invalid_transpose: serde_json::Value =
        serde_json::from_str(&core.transpose_midi_notes_range_diagnostic_json(0, 0, 100, 1))
            .expect("MIDI transpose diagnostic must be JSON");
    assert_eq!(invalid_transpose["code"], "invalid_midi_transpose");
}

#[test]
fn comping_diagnostics_reject_malformed_and_empty_mutations() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");

    let malformed: serde_json::Value =
        serde_json::from_str(&core.set_comp_segments_diagnostic_json("not-json"))
            .expect("comping segment diagnostic must be JSON");
    assert_eq!(malformed["code"], "invalid_comp_segments_json");

    let empty: serde_json::Value =
        serde_json::from_str(&core.set_comp_segments_diagnostic_json("[]"))
            .expect("empty segment diagnostic must be JSON");
    assert_eq!(empty["ok"], true);

    let snapshot: serde_json::Value =
        serde_json::from_str(&core.restore_comping_snapshot_diagnostic_json("{}"))
            .expect("comping snapshot diagnostic must be JSON");
    assert_eq!(snapshot["code"], "invalid_comping_snapshot_json");
}

#[test]
fn track_scalar_diagnostics_reject_invalid_and_stale_targets() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let invalid: serde_json::Value =
        serde_json::from_str(&core.set_volume_diagnostic_json(1, f32::NAN))
            .expect("invalid scalar diagnostic must be JSON");
    assert_eq!(invalid["code"], "invalid_parameter");

    let stale: serde_json::Value =
        serde_json::from_str(&core.set_pan_diagnostic_json(999_999, 0.25))
            .expect("stale track diagnostic must be JSON");
    assert_eq!(stale["code"], "track_not_found_or_rejected");
    assert_eq!(stale["affected_object"], "track:999999");
    assert!(stale["generation"].as_u64().is_some());
}

#[test]
fn track_toggle_and_lifecycle_diagnostics_are_structured() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let created: serde_json::Value = serde_json::from_str(&core.add_track_diagnostic_json(0))
        .expect("track creation diagnostic must be JSON");
    let track_id = created["track_id"].as_u64().expect("created track id");
    assert_eq!(created["ok"], true);

    let renamed: serde_json::Value =
        serde_json::from_str(&core.set_track_name_diagnostic_json(track_id as u32, "Lead Vocal"))
            .expect("track rename diagnostic must be JSON");
    assert_eq!(renamed["ok"], true);

    let duplicate: serde_json::Value =
        serde_json::from_str(&core.duplicate_track_diagnostic_json(track_id as u32))
            .expect("track duplicate diagnostic must be JSON");
    assert_eq!(duplicate["ok"], true);
    assert_ne!(duplicate["track_id"], track_id);

    let mixing: serde_json::Value =
        serde_json::from_str(&core.execute_auto_mixing_diagnostic_json())
            .expect("auto mixing diagnostic must be JSON");
    assert_eq!(mixing["ok"], true);

    let arrangement: serde_json::Value =
        serde_json::from_str(&core.execute_auto_arrangement_diagnostic_json())
            .expect("auto arrangement diagnostic must be JSON");
    assert_eq!(arrangement["ok"], true);

    let scale: serde_json::Value =
        serde_json::from_str(&core.set_project_scale_diagnostic_json(0, 0))
            .expect("project scale diagnostic must be JSON");
    assert_eq!(scale["ok"], true);
    let invalid_scale: serde_json::Value =
        serde_json::from_str(&core.set_project_scale_diagnostic_json(12, 0))
            .expect("invalid project scale diagnostic must be JSON");
    assert_eq!(invalid_scale["code"], "invalid_project_scale");

    let muted: serde_json::Value =
        serde_json::from_str(&core.set_mute_diagnostic_json(track_id as u32, true))
            .expect("mute diagnostic must be JSON");
    assert_eq!(muted["ok"], true);

    let removed: serde_json::Value =
        serde_json::from_str(&core.remove_track_diagnostic_json(track_id as u32))
            .expect("remove diagnostic must be JSON");
    assert_eq!(removed["ok"], true);

    let missing: serde_json::Value =
        serde_json::from_str(&core.set_solo_diagnostic_json(track_id as u32, true))
            .expect("missing toggle diagnostic must be JSON");
    assert_eq!(missing["code"], "track_not_found_or_rejected");

    let invalid_name: serde_json::Value =
        serde_json::from_str(&core.set_track_name_diagnostic_json(0, "  "))
            .expect("invalid name diagnostic must be JSON");
    assert_eq!(invalid_name["code"], "invalid_track_name");
}

#[test]
fn plugin_mutation_diagnostics_preserve_target_context() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let track_id = core.add_track(0);
    let added: serde_json::Value =
        serde_json::from_str(&core.add_plugin_diagnostic_json(track_id, 0))
            .expect("plugin add diagnostic must be JSON");
    assert_eq!(added["ok"], true);

    let bypassed: serde_json::Value =
        serde_json::from_str(&core.set_plugin_bypass_diagnostic_json(track_id, 0, true))
            .expect("plugin bypass diagnostic must be JSON");
    assert_eq!(bypassed["ok"], true);

    let removed: serde_json::Value =
        serde_json::from_str(&core.remove_plugin_diagnostic_json(track_id, 0))
            .expect("plugin remove diagnostic must be JSON");
    assert_eq!(removed["ok"], true);

    let missing: serde_json::Value =
        serde_json::from_str(&core.remove_plugin_diagnostic_json(track_id, 0))
            .expect("missing plugin diagnostic must be JSON");
    assert_eq!(missing["code"], "plugin_not_found_or_rejected");
    assert_eq!(
        missing["affected_object"],
        format!("track:{track_id}/plugin:0")
    );
}

#[test]
fn routing_diagnostics_reject_invalid_graph_inputs_explicitly() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let self_loop: serde_json::Value =
        serde_json::from_str(&core.set_route_diagnostic_json(4, 4, true))
            .expect("route diagnostic must be JSON");
    assert_eq!(self_loop["code"], "route_self_loop");

    let bad_gain: serde_json::Value =
        serde_json::from_str(&core.set_feedback_route_diagnostic_json(1, 2, f32::NAN, true))
            .expect("feedback diagnostic must be JSON");
    assert_eq!(bad_gain["code"], "invalid_route_gain");

    let bad_tap: serde_json::Value =
        serde_json::from_str(&core.set_sidechain_link_diagnostic_json(1, 2, 0, 99, true))
            .expect("sidechain diagnostic must be JSON");
    assert_eq!(bad_tap["code"], "invalid_sidechain_tap");
}

#[test]
fn audio_device_diagnostics_preserve_configuration_boundaries() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    let bad_rate: serde_json::Value =
        serde_json::from_str(&core.apply_audio_config_diagnostic_json(22_050, 128))
            .expect("audio config diagnostic must be JSON");
    assert_eq!(bad_rate["code"], "unsupported_sample_rate");

    let bad_buffer: serde_json::Value =
        serde_json::from_str(&core.apply_audio_config_diagnostic_json(48_000, 127))
            .expect("audio config diagnostic must be JSON");
    assert_eq!(bad_buffer["code"], "unsupported_buffer_size");

    let reconnect: serde_json::Value =
        serde_json::from_str(&core.try_reconnect_audio_device_diagnostic_json())
            .expect("reconnect diagnostic must be JSON");
    assert!(reconnect["status"].is_string());
    assert!(reconnect["audio_generation"].as_u64().is_some());
}

#[test]
fn tpdf_dither_is_reproducible_and_separates_the_two_channels() {
    // The TPDF dither used to derive its seed from the wall clock. Every bounce
    // therefore rendered a different noise sequence and two bounces of the same
    // project produced different files. The sequence is fixed now; this asserts
    // the contract at the unit level so the renderer cannot regress to
    // clock-derived dither.
    let left_a = {
        let mut engine = crate::tpdf_dither::TpdfDitherEngine::new();
        (0..512).map(|_| engine.next_sample()).collect::<Vec<f32>>()
    };
    let left_b = {
        let mut engine = crate::tpdf_dither::TpdfDitherEngine::new();
        (0..512).map(|_| engine.next_sample()).collect::<Vec<f32>>()
    };
    assert_eq!(left_a, left_b, "two engines built the same way must agree");

    let right = {
        let mut engine = crate::tpdf_dither::TpdfDitherEngine::with_seed(
            crate::tpdf_dither::RIGHT_CHANNEL_DITHER_SEED,
        );
        (0..512).map(|_| engine.next_sample()).collect::<Vec<f32>>()
    };
    assert_ne!(
        left_b, right,
        "left and right dither must not share one noise sequence"
    );

    // A zero seed is a legal input and must still produce a usable, non-trivial
    // sequence rather than the degenerate all-zero output of a stuck xorshift.
    let mut spun = crate::tpdf_dither::TpdfDitherEngine::with_seed(0);
    let any_nonzero = (0..64).any(|_| spun.next_sample() != 0.0);
    assert!(any_nonzero, "a zero seed must still produce noise");
}

#[test]
fn offline_bounce_of_the_same_project_is_reproducible() {
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new().expect("core must initialize");
    assert!(core.apply_audio_config(48_000, 1_024));
    let track_id = core.add_audio_track();
    assert_ne!(track_id, 0);

    let first = std::env::temp_dir().join(format!(
        "hirari-bounce-repro-a-{}.wav",
        std::process::id()
    ));
    let second = std::env::temp_dir().join(format!(
        "hirari-bounce-repro-b-{}.wav",
        std::process::id()
    ));
    assert!(core.bounce_project(first.to_str().unwrap(), 0));
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(core.bounce_project(second.to_str().unwrap(), 0));

    let first_bytes = std::fs::read(&first).expect("first bounce must exist");
    let second_bytes = std::fs::read(&second).expect("second bounce must exist");
    assert_eq!(
        first_bytes, second_bytes,
        "bouncing the same project twice must produce the same file"
    );

    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
}

#[test]
fn cli_render_stems_path_is_audible_at_the_projects_sample_rate() {
    // Replicates `hirari render stems`: the same offline core, v2 load, then
    // `bounce_stems_diagnostic_json` — the native bounce graph the UI's
    // EXPORT STEMS uses. It replaces a private re-implementation that re-read
    // region WAVs with `read_pcm_wav` and summed raw samples: for the demo
    // project that renderer wrote digital silence (peak 0.0000) while
    // `render mix` of the same project reached peak 0.99, and it ran at the
    // engine's hardcoded 44100 regardless of the project's rate. Assert both
    // properties the replacement must hold: audible stems, at the project's
    // own rate.
    let _guard = native_engine_test_guard();
    let unique = format!(
        "hirari-cli-stems-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}-src.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let output_dir = std::env::temp_dir().join(format!("{unique}-stems"));
    // 48 kHz is deliberately not the engine's default 44100, so a renderer
    // that ignores the document's rate cannot pass by accident. A fresh core
    // sits at 44100 and `save_project_v2` records the *engine's* rate, so the
    // builder has to be reconfigured through the same `apply_audio_config`
    // path the audio settings UI uses; otherwise this would silently produce
    // a 44.1 kHz document and the rate assertion below would prove nothing.
    write_test_tone_wav(&wav_path, 48_000, 48_000);
    let project_text = project_path.to_string_lossy().into_owned();
    let builder = crate::HirariCore::new_offline().expect("offline core must initialize");
    assert!(
        builder.apply_audio_config(48_000, 512),
        "builder core must accept a 48 kHz configuration"
    );
    assert_eq!(
        builder.get_sample_rate().round() as u32,
        48_000,
        "builder core must report the configured rate"
    );
    let track = builder.add_audio_track();
    assert_ne!(track, 0);
    assert!(builder.add_region(track, wav_path.to_str().unwrap(), 0.0));
    builder
        .save_project_v2(&project_text, "CLI Stems", 120.0)
        .expect("save must succeed");

    // The document itself must be 48 kHz; if the save had recorded the
    // engine's default the rest of this test would only re-check 44.1 kHz.
    let document = crate::project::ProjectDocument::load(&project_text)
        .expect("saved document must load");
    assert_eq!(
        document.sample_rate.round() as u32,
        48_000,
        "the saved project document must carry the configured rate"
    );

    std::fs::create_dir_all(&output_dir).expect("stem output dir must be created");
    let output_text = output_dir.to_string_lossy().into_owned();
    let core = crate::HirariCore::new_offline().expect("offline core must initialize");
    core.load_project_v2(&project_text)
        .expect("load must succeed");
    assert_eq!(
        core.get_sample_rate().round() as u32,
        48_000,
        "the loaded core must run at the document's rate before bouncing"
    );

    let diagnostic = core.bounce_stems_diagnostic_json(&output_text, 0, &[], 2.0, false, true);
    let value: serde_json::Value =
        serde_json::from_str(&diagnostic).expect("stems diagnostic must be valid JSON");
    assert_eq!(
        value.get("ok").and_then(serde_json::Value::as_bool),
        Some(true),
        "stems bounce must report success, got {diagnostic}"
    );
    let outputs = value
        .get("outputs")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !outputs.is_empty(),
        "stems bounce must produce at least one file, got {diagnostic}"
    );

    let stem_path = outputs[0].as_str().expect("output path must be a string");
    let bytes = std::fs::read(stem_path).expect("stem file must exist");
    assert!(
        bytes.len() > 44,
        "stem file must carry audio data, got {} bytes",
        bytes.len()
    );
    let stem_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    assert_eq!(
        stem_rate, 48_000,
        "stems must render at the project's sample rate"
    );
    let peak = bytes
        .get(44..)
        .map(|data| {
            data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                .fold(0.0f32, |a, v| a.max(v.abs()))
        })
        .unwrap_or(0.0);
    assert!(
        peak > 0.01,
        "stems must be audible (peak {peak}); digital silence means the \
         renderer bypassed the engine graph"
    );

    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&project_path);
    let _ = std::fs::remove_dir_all(&output_dir);
}

/// The CLI and the GUI used to carry separate stem filename sanitizers, and
/// only the GUI prefixed Windows device names or matched collisions the way a
/// destination volume resolves them.  Measured on the published names rather
/// than on a sanitizer in isolation: `CON` left the CLI as the unusable
/// `CON.wav`, and `Drums`/`DRUMS` reached it as two paths a case-insensitive
/// volume collapses onto one inode — losing a stem while reporting two
/// outputs.  This pins what `render stems` writes on disk.
#[test]
fn stem_filenames_follow_one_policy_for_reserved_and_case_colliding_names() {
    let _guard = native_engine_test_guard();
    let unique = format!(
        "hirari-stem-names-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}-src.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let output_dir = std::env::temp_dir().join(format!("{unique}-stems"));

    write_test_tone_wav(&wav_path, 48_000, 48_000);
    let project_text = project_path.to_string_lossy().into_owned();
    let builder = crate::HirariCore::new_offline().expect("offline core must initialize");
    assert!(builder.apply_audio_config(48_000, 512));
    let reserved = builder.add_audio_track();
    let first = builder.add_audio_track();
    let second = builder.add_audio_track();
    assert_ne!(reserved, 0);
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    assert!(builder.set_track_name(reserved, "CON"));
    assert!(builder.set_track_name(first, "Drums"));
    assert!(builder.set_track_name(second, "DRUMS"));
    for track in [reserved, first, second] {
        assert!(builder.add_region(track, wav_path.to_str().unwrap(), 0.0));
    }
    builder
        .save_project_v2(&project_text, "Stem Filenames", 120.0)
        .expect("save must succeed");

    std::fs::create_dir_all(&output_dir).expect("stem output dir must be created");
    let output_text = output_dir.to_string_lossy().into_owned();
    let core = crate::HirariCore::new_offline().expect("offline core must initialize");
    core.load_project_v2(&project_text)
        .expect("load must succeed");

    let diagnostic = core.bounce_stems_diagnostic_json(&output_text, 0, &[], 2.0, false, true);
    let value: serde_json::Value =
        serde_json::from_str(&diagnostic).expect("stems diagnostic must be valid JSON");
    assert_eq!(
        value.get("ok").and_then(serde_json::Value::as_bool),
        Some(true),
        "stems bounce must report success, got {diagnostic}"
    );
    let outputs: Vec<String> = value
        .get("outputs")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        outputs.len() >= 3,
        "every named track must publish a stem, got {diagnostic}"
    );

    let stems: std::collections::HashSet<String> = outputs
        .iter()
        .filter_map(|path| {
            std::path::Path::new(path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        stems.contains("track-CON"),
        "Windows device names must be prefixed, got {diagnostic}"
    );
    assert!(
        !stems.contains("CON"),
        "no stem may be published as CON, got {diagnostic}"
    );

    // Order-independent: whichever of the two collides first keeps its own
    // casing, and the loser takes a `-2` suffix.
    let drums: Vec<String> = stems
        .iter()
        .filter(|stem| stem.to_ascii_lowercase().starts_with("drums"))
        .cloned()
        .collect();
    let mut drums_lower: Vec<String> = drums.iter().map(|stem| stem.to_ascii_lowercase()).collect();
    drums_lower.sort();
    assert_eq!(
        drums_lower,
        vec!["drums".to_owned(), "drums-2".to_owned()],
        "case-only collisions must be suffixed, got {diagnostic}"
    );

    let lowered: std::collections::HashSet<String> =
        stems.iter().map(|stem| stem.to_ascii_lowercase()).collect();
    assert_eq!(
        lowered.len(),
        stems.len(),
        "two outputs must never differ only by case, got {diagnostic}"
    );

    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&project_path);
    let _ = std::fs::remove_dir_all(&output_dir);
}

/// Every `<format>` spelling `render stems` advertises must reach the engine
/// and produce the container it names. The dispatcher used to reject all four
/// but `wav` — `format != "wav"` printed usage and exited 2 — while the usage
/// line one row above advertised `wav|wav-pcm24|wave64|wav-f32`, so the
/// documented forms failed for no reason other than the missing branch. What
/// makes this a regression rather than a formality is the numbering: the
/// engine's `format` is 0=Pcm16, 1=Wave64, 2=Float32, 3=Pcm24, which is *not*
/// the order the names appear in, so a dispatcher that passes an index rather
/// than the mapped container silently writes the wrong encoding.
#[test]
fn cli_render_stems_format_names_reach_the_engine_as_the_container_they_name() {
    let _guard = native_engine_test_guard();
    let unique = format!(
        "hirari-stem-format-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let wav_path = std::env::temp_dir().join(format!("{unique}-src.wav"));
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    write_test_tone_wav(&wav_path, 44_100, 44_100);
    let project_text = project_path.to_string_lossy().into_owned();
    let builder = crate::HirariCore::new_offline().expect("offline core must initialize");
    let track = builder.add_audio_track();
    assert_ne!(track, 0);
    assert!(builder.add_region(track, wav_path.to_str().unwrap(), 0.0));
    builder
        .save_project_v2(&project_text, "Stem Formats", 120.0)
        .expect("save must succeed");

    // (format name, engine format number, expected bytes-per-sample in the
    // written file). The engine's numbering is 0=Pcm16, 1=Wave64, 2=Float32,
    // 3=Pcm24 — deliberately not the order the names appear in the usage
    // line, so an index passed through un-mapped writes the wrong container.
    let cases: [(&str, u32, u32); 4] = [
        ("wav", 0, 2),
        ("wave64", 1, 4),
        ("wav-f32", 2, 4),
        ("wav-pcm24", 3, 3),
    ];
    for (name, engine_format, expected_bytes_per_sample) in cases {
        let output_dir = std::env::temp_dir().join(format!("{unique}-{name}"));
        std::fs::create_dir_all(&output_dir).expect("stem output dir must be created");
        let core = crate::HirariCore::new_offline().expect("offline core must initialize");
        core.load_project_v2(&project_text)
            .expect("load must succeed");
        let diagnostic = core.bounce_stems_diagnostic_json(
            output_dir.to_string_lossy().as_ref(),
            engine_format,
            &[],
            2.0,
            false,
            true,
        );
        let value: serde_json::Value =
            serde_json::from_str(&diagnostic).expect("stems diagnostic must be valid JSON");
        assert_eq!(
            value.get("ok").and_then(serde_json::Value::as_bool),
            Some(true),
            "format {name} must bounce, got {diagnostic}"
        );
        let outputs = value
            .get("outputs")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(!outputs.is_empty(), "format {name} must emit a file");
        let stem_path = outputs[0].as_str().expect("output path must be a string");
        let bytes = std::fs::read(stem_path).expect("stem file must exist");

        // The two containers have unrelated layouts, and the Wave64 fields
        // are read off `write_wave64_stream_header`: a 16-byte RIFF GUID, an
        // 8-byte size, a 16-byte WAVE GUID, then 16-byte chunk GUIDs with
        // 8-byte sizes.
        //
        // Fixed offsets are not usable here at all. A WAVE file may carry a
        // `JUNK` chunk ahead of `fmt` (28 bytes in the file this was derived
        // from), so byte 20 is not reliably the format tag — it is the id of
        // whatever chunk comes first. Asserting a constant offset would test
        // the padding rather than the encoding, and would pass or fail for a
        // reason unrelated to the format requested. Walk the chunks instead.
        let (bits, format_tag) = if name == "wave64" {
            let mut offset = 40usize;
            let mut tag = 0u16;
            let mut depth = 0u16;
            while offset + 24 <= bytes.len() {
                let size = u64::from_le_bytes(bytes[offset + 16..offset + 24].try_into().unwrap())
                    as usize;
                let body = offset + 24;
                if &bytes[offset..offset + 4] == b"fmt " {
                    tag = u16::from_le_bytes(bytes[body..body + 2].try_into().unwrap());
                    depth = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
                    break;
                }
                if size < 24 {
                    break;
                }
                offset += size;
            }
            (depth, tag)
        } else {
            let mut offset = 12usize;
            let mut tag = 0u16;
            let mut depth = 0u16;
            while offset + 8 <= bytes.len() {
                let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap())
                    as usize;
                let body = offset + 8;
                if &bytes[offset..offset + 4] == b"fmt " {
                    tag = u16::from_le_bytes(bytes[body..body + 2].try_into().unwrap());
                    depth = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
                    break;
                }
                if size < 8 {
                    break;
                }
                offset = body + size + (size & 1);
            }
            (depth, tag)
        };
        assert_eq!(
            bits as u32,
            expected_bytes_per_sample * 8,
            "format {name} must write {expected_bytes_per_sample}-byte samples, got {bits} bits"
        );

        // The container family matters as much as the width: engine formats 1
        // and 3 both land at 4 bytes on disk only in the float case, so a
        // dispatcher that passed an index instead of the mapped container
        // would still write a well-formed file, just not the one asked for.
        // WAVE64 is specified as IEEE float, so its tag is 3 like wav-f32 —
        // not the 1 a plain PCM file carries, and not a value to guess.
        let expected_tag = if name == "wav-f32" || name == "wave64" { 3 } else { 1 };
        assert_eq!(
            format_tag, expected_tag,
            "format {name} must write the container it names (fmt tag {expected_tag}), got \
             {format_tag} at width {bits} bits"
        );
        let _ = std::fs::remove_dir_all(&output_dir);
    }

    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&project_path);
}
