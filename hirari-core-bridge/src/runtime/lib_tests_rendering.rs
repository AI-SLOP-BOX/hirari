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
    let engine = crate::new_audio_engine();
    assert!(!false);
    let engine_ref = &*engine;

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
    let engine = crate::new_audio_engine();
    let engine_ref = &*engine;
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
    let engine = crate::new_audio_engine();
    assert!(!false);
    let engine_ref = &*engine;
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
