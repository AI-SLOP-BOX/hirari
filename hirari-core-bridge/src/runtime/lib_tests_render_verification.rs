// Split out of lib_tests_rendering.rs.

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

// The async import pipeline is Rust-owned (region_jobs.rs); these tests drive
// it through the engine so the registry limits, ticket lifecycle, region
// insertion and its single-step undo stay pinned to the public bridge.
#[test]
fn queued_region_import_commits_and_undoes_as_one_step() {
    let _guard = native_engine_test_guard();
    let engine = crate::new_audio_engine();
    let e = &*engine;
    let wav_path = std::env::temp_dir().join(format!(
        "hirari-queued-import-{}.wav",
        std::process::id()
    ));
    write_test_tone_wav(&wav_path, 48_000, 4_800);
    let source = wav_path.to_str().unwrap();
    let track = e.add_track(0);
    assert_ne!(track, 0);

    let ticket = e.queue_region_import(track, source, 0);
    assert_ne!(ticket, 0, "queue must return a ticket");
    assert_eq!(
        e.queue_region_import(track + 99, source, 0),
        0,
        "unknown tracks are rejected before decoding"
    );

    let mut status = 0u8;
    for _ in 0..5_000 {
        status = e.complete_region_import(ticket);
        if status != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(status, 1, "queued import must apply");
    assert_eq!(e.complete_region_import(ticket), 3, "tickets are consumed");

    let layout: serde_json::Value =
        serde_json::from_str(&e.project_layout_json()).unwrap();
    assert_eq!(layout[0]["regions"].as_array().map(Vec::len), Some(1));

    e.undo();
    let layout: serde_json::Value =
        serde_json::from_str(&e.project_layout_json()).unwrap();
    assert_eq!(layout[0]["regions"].as_array().map(Vec::len), Some(0));

    e.redo();
    let layout: serde_json::Value =
        serde_json::from_str(&e.project_layout_json()).unwrap();
    assert_eq!(layout[0]["regions"].as_array().map(Vec::len), Some(1));

    let _ = std::fs::remove_file(&wav_path);
}

// Pitch analysis is a Rust job (region_jobs.rs) that retains the track for the
// worker and records the before/after note tables as one undo pair.
#[test]
fn queued_pitch_analysis_finishes_and_finalizes() {
    let _guard = native_engine_test_guard();
    let engine = crate::new_audio_engine();
    let e = &*engine;
    let wav_path = std::env::temp_dir().join(format!(
        "hirari-queued-analysis-{}.wav",
        std::process::id()
    ));
    write_test_tone_wav(&wav_path, 48_000, 48_000);
    let source = wav_path.to_str().unwrap();
    let track = e.add_track(0);
    assert!(e.add_region(track, source, 0.0));

    let layout: serde_json::Value =
        serde_json::from_str(&e.project_layout_json()).unwrap();
    let region = layout[0]["regions"][0]["id"].as_u64().expect("region id") as u32;

    assert!(!e.start_region_audio_note_analysis(track, region, 1_000.0));
    assert!(e.start_region_audio_note_analysis(track, region, 48_000.0));
    assert!(
        !e.start_region_audio_note_analysis(track, region, 48_000.0),
        "a second analysis for the same region is rejected while running"
    );

    let mut status = 0u8;
    for _ in 0..5_000 {
        status = e.region_audio_note_analysis_status(track, region);
        if status != 1 && status != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(status, 2, "analysis must become ready");
    assert!(e.finalize_region_audio_note_analysis(track, region));
    assert_eq!(e.region_audio_note_analysis_status(track, region), 3);

    let _ = std::fs::remove_file(&wav_path);
}

#[test]
fn step_sequencer_patterns_survive_a_v2_save_and_reload() {
    // The engine now owns step-sequencer pattern display state (lane
    // pitches, resolution, feel) round-tripping with the project document.
    // Track-scoped patterns (region_id == 0) were validated at the API and
    // restored on load so the AmeUI STEP SEQ card cannot silently lose
    // anything between sessions.
    let _guard = native_engine_test_guard();
    let core = crate::HirariCore::new_offline().expect("offline core must initialize");
    let track = core.add_midi_track();
    assert_ne!(track, 0, "offline engine must refuse silently MIDI track creation");

    let patterns = vec![crate::project::StepSequencerPatternContract {
        track_id: track,
        region_id: 0,
        lane_pitches: [36, 38, 42, 46, 40, 45],
        resolution: 32,
        feel: 1,
    }];
    let payload = serde_json::to_string(&patterns).expect("patterns must serialize");
    assert!(core.set_step_sequencer_patterns_json(&payload));

    let unique = format!(
        "hirari-step-seq-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let project_path = std::env::temp_dir().join(format!("{unique}.hirari"));
    let text = project_path.to_string_lossy().into_owned();
    core.save_project_v2(&text, "Step Sequencer", 120.0)
        .expect("v2 save must succeed");

    let restored = crate::HirariCore::new_offline().expect("restored offline core must initialize");
    restored
        .load_project_v2(&text)
        .expect("v2 load must succeed");
    let stored: Vec<serde_json::Value> = serde_json::from_str(
        &restored.step_sequencer_patterns_json(),
    )
    .expect("patterns must be valid JSON");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["track_id"].as_u64(), Some(u64::from(track)));
    assert_eq!(stored[0]["region_id"].as_u64(), Some(0));
    assert_eq!(stored[0]["resolution"].as_u64(), Some(32));
    assert_eq!(stored[0]["feel"].as_u64(), Some(1));
    assert_eq!(stored[0]["lane_pitches"].as_array().map(Vec::len), Some(6));
    let _ = std::fs::remove_file(&project_path);
}
