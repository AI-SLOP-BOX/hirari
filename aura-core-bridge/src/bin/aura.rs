use aura_core_bridge::command_api::{
    audit_log_json, capabilities, diff, validate, validate_request_envelope, CommandAction,
    CommandDocument, Permission, ProtocolRequest, PROTOCOL_VERSION,
};
use aura_core_bridge::project::ProjectDocument;
use aura_core_bridge::stable_api::{CoreApiV1, MixRenderRequest, WaveContainer};
use std::io::{self, Read};

mod cli_piano;
mod cli_project;
mod cli_render;
fn read_pcm_wav(path: &str) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("expected RIFF/WAVE".into());
    }
    let mut offset = 12usize;
    let mut channels = 0u16;
    let mut bits_per_sample = 0u16;
    let mut data = None;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(size))
            .ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV chunk".into());
        }
        match &bytes[offset..offset + 4] {
            b"fmt " if size >= 16 => {
                let format = u16::from_le_bytes(bytes[offset + 8..offset + 10].try_into().unwrap());
                channels = u16::from_le_bytes(bytes[offset + 10..offset + 12].try_into().unwrap());
                bits_per_sample =
                    u16::from_le_bytes(bytes[offset + 22..offset + 24].try_into().unwrap());
                if format != 1 || channels == 0 || !matches!(bits_per_sample, 16 | 24 | 32) {
                    return Err("only PCM16/24/32 WAV is supported".into());
                }
            }
            b"data" => data = Some((offset + 8, size)),
            _ => {}
        }
        offset = end + (size & 1);
    }
    let (start, size) = data.ok_or("WAV data chunk is missing")?;
    let bytes_per_sample = usize::from(bits_per_sample / 8);
    if channels == 0 || bytes_per_sample == 0 || size % (channels as usize * bytes_per_sample) != 0
    {
        return Err("invalid WAV frame alignment".into());
    }
    let frame_width = channels as usize * bytes_per_sample;
    let mut samples = Vec::with_capacity(size / bytes_per_sample);
    for frame in bytes[start..start + size].chunks_exact(frame_width) {
        for sample in frame.chunks_exact(bytes_per_sample) {
            let value = match bits_per_sample {
                16 => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
                24 => {
                    let raw = i32::from_le_bytes([
                        sample[0],
                        sample[1],
                        sample[2],
                        if sample[2] & 0x80 != 0 { 0xff } else { 0 },
                    ]);
                    raw as f32 / 8_388_608.0
                }
                32 => i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0,
                _ => unreachable!(),
            };
            samples.push(value);
        }
    }
    Ok(samples)
}

fn render_project_stems(
    project_path: &str,
    output_dir: &str,
) -> Result<Vec<std::path::PathBuf>, String> {
    let project = ProjectDocument::load(project_path).map_err(|e| e.to_string())?;
    let base = std::path::Path::new(project_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let output = std::path::Path::new(output_dir);
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let mut stems = Vec::new();
    for track in &project.tracks {
        let regions: Vec<_> = project
            .regions
            .iter()
            .filter(|r| r.track_id == track.id && !r.muted)
            .collect();
        if regions.is_empty() {
            continue;
        }
        let end = regions
            .iter()
            .map(|r| r.start.saturating_add(r.length))
            .max()
            .unwrap_or(0) as usize;
        let mut buffer = vec![0.0f32; end];
        for region in regions {
            let source = base.join(&region.path);
            let samples = read_pcm_wav(source.to_str().ok_or("invalid audio path")?)?;
            let offset = region.source_offset as usize;
            let available = samples.len().saturating_sub(offset);
            let count = available
                .min(region.length as usize)
                .min(buffer.len().saturating_sub(region.start as usize));
            for index in 0..count {
                buffer[region.start as usize + index] += samples[offset + index] * region.clip_gain;
            }
        }
        stems.push((track.name.clone(), buffer));
    }
    if stems.is_empty() {
        return Err("project has no renderable audio regions".into());
    }
    let sample_rate = project.sample_rate.round().clamp(1.0, u32::MAX as f64) as u32;
    aura_core_bridge::export::export_stems_to_wav(output, &stems, sample_rate, 1, 24, false, true)
        .map_err(|e| format!("stem export failed: {e:?}"))
}

fn usage() -> ! {
    eprintln!("usage: aura capabilities | aura audit-log <ledger-path> | aura project init <project> [name] [sample-rate] | aura project inspect <project> | aura project manifest <project> | aura project production init <project> [fps] | aura project production inspect <project> | aura project production revision <project> <message> [author] [--audio-hash HASH] [--vfx-hash HASH] | aura track add <project> <name> [type] | aura track rename <project> <track-id> <name> | aura track delete <project> <track-id> | aura plugin insert <project> <track-id> <plugin-type> | aura mix analyze <left.wav> <right.wav> | aura render mix <project> <output.wav> [wav|wave64] | aura render stems <project> <output-dir> [format] | aura render piano <notes.json> <audio.wav> <output.mp4> [sample-rate] [channels] | aura ci verify <audio.wav> [max-peak] [min-rms] | aura diff | aura validate");
    std::process::exit(2);
}

fn main() {
    // Handle project subcommands from an immutable argv snapshot first.  The
    // legacy match below uses iterator guards; those guards consume values
    // while probing alternatives, so dispatching these read/write operations
    // here keeps `project inspect` and `project manifest` deterministic.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if cli_piano::try_handle(&raw_args) {
        return;
    }
    if cli_project::try_handle(&raw_args) {
        return;
    }
    if cli_render::try_handle(&raw_args) {
        return;
    }
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("capabilities") if args.next().is_none() => {
            println!("{}", serde_json::to_string_pretty(&capabilities()).unwrap());
        }
        Some("audit-log") => {
            let Some(path) = args.next() else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match audit_log_json(path) {
                Ok(value) => println!("{}", serde_json::to_string(&value).unwrap()),
                Err(error) => {
                    eprintln!("{}", serde_json::to_string(&error).unwrap());
                    std::process::exit(1);
                }
            }
        }
        Some("project") if args.next().as_deref() == Some("init") => {
            let Some(path) = args.next() else {
                usage();
            };
            let name = args
                .next()
                .unwrap_or_else(|| "Untitled Aura Project".into());
            let sample_rate = args
                .next()
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(44_100.0);
            if args.next().is_some()
                || !sample_rate.is_finite()
                || !(8_000.0..=384_000.0).contains(&sample_rate)
            {
                usage();
            }
            match ProjectDocument::from_layout_json(name, 120.0, sample_rate, "[]")
                .and_then(|project| project.save_atomic(&path))
            {
                Ok(()) => println!(
                    "{{\"ok\":true,\"operation\":\"project.init\",\"path\":{}}}",
                    serde_json::to_string(&path).unwrap()
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("project") if args.next().as_deref() == Some("inspect") => {
            let Some(path) = args.next() else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path) {
                Ok(project) => {
                    let summary = serde_json::json!({
                        "ok": true,
                        "operation": "project.inspect",
                        "project_id": project.project_id,
                        "schema_version": project.schema_version,
                        "contract_version": project.contract_version,
                        "sample_rate": project.sample_rate,
                        "track_count": project.tracks.len(),
                        "region_count": project.regions.len(),
                        "plugin_count": project.plugin_instances.len(),
                        "midi_note_count": project.midi_notes.len(),
                        "midi_event_count": project.midi_events.len(),
                        "render_target_count": project.render_targets.len(),
                    });
                    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
                }
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("project") if args.next().as_deref() == Some("manifest") => {
            let Some(path) = args.next() else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path)
                .and_then(|project| project.reproducibility_manifest())
            {
                Ok(manifest) => println!("{}", serde_json::to_string_pretty(&manifest).unwrap()),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("track") if args.next().as_deref() == Some("add") => {
            let Some(path) = args.next() else {
                usage();
            };
            let Some(name) = args.next() else {
                usage();
            };
            let track_type = args.next().unwrap_or_else(|| "Audio".into());
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path).and_then(|mut project| {
                let id = project.add_track(name, &track_type)?;
                project.save_atomic(&path)?;
                Ok(id)
            }) {
                Ok(id) => println!("{{\"ok\":true,\"operation\":\"track.add\",\"track_id\":{id}}}"),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("track") if args.next().as_deref() == Some("rename") => {
            let Some(path) = args.next() else {
                usage();
            };
            let Some(track_id) = args.next().and_then(|value| value.parse::<u32>().ok()) else {
                usage();
            };
            let Some(name) = args.next() else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path).and_then(|mut project| {
                project.set_track_name(track_id, name)?;
                project.save_atomic(&path)
            }) {
                Ok(()) => println!(
                    "{{\"ok\":true,\"operation\":\"track.rename\",\"track_id\":{track_id}}}"
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("track") if args.next().as_deref() == Some("delete") => {
            let Some(path) = args.next() else {
                usage();
            };
            let Some(track_id) = args.next().and_then(|value| value.parse::<u32>().ok()) else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path).and_then(|mut project| {
                project.remove_track(track_id)?;
                project.save_atomic(&path)
            }) {
                Ok(()) => println!(
                    "{{\"ok\":true,\"operation\":\"track.delete\",\"track_id\":{track_id}}}"
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("plugin") if args.next().as_deref() == Some("insert") => {
            let Some(path) = args.next() else {
                usage();
            };
            let Some(track_id) = args.next().and_then(|value| value.parse::<u32>().ok()) else {
                usage();
            };
            let Some(plugin_type) = args.next().and_then(|value| value.parse::<u32>().ok()) else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match ProjectDocument::load(&path).and_then(|mut project| {
                let slot = project.insert_builtin_plugin(track_id, plugin_type)?;
                project.save_atomic(&path)?;
                Ok(slot)
            }) {
                Ok(slot) => println!(
                    "{{\"ok\":true,\"operation\":\"plugin.insert\",\"slot_index\":{slot}}}"
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("mix") if args.next().as_deref() == Some("analyze") => {
            let Some(left_path) = args.next() else {
                usage();
            };
            let Some(right_path) = args.next() else {
                usage();
            };
            if args.next().is_some() {
                usage();
            }
            match read_pcm_wav(&left_path)
                .and_then(|left| read_pcm_wav(&right_path).map(|right| (left, right)))
            {
                Ok((left, right)) => println!(
                    "{}",
                    serde_json::to_string(&aura_core_bridge::mix_analyzer::analyze(&left, &right))
                        .unwrap()
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("render") if args.next().as_deref() == Some("mix") => {
            let Some(project_path) = args.next() else {
                usage();
            };
            let Some(output_path) = args.next() else {
                usage();
            };
            let container = match args.next().as_deref() {
                None | Some("wav") => WaveContainer::Wav,
                Some("wave64") => WaveContainer::Wave64,
                Some(_) => usage(),
            };
            if args.next().is_some() {
                usage();
            }
            let result = CoreApiV1::new_headless().and_then(|api| {
                api.render_mix(MixRenderRequest {
                    project_path: project_path.into(),
                    output_path: output_path.into(),
                    container,
                })
            });
            match result {
                Ok(result) => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("render") if args.next().as_deref() == Some("stems") => {
            let Some(project_path) = args.next() else {
                usage();
            };
            let Some(output_dir) = args.next() else {
                usage();
            };
            let format = args.next().unwrap_or_else(|| "wav".into());
            let request_id = match args.next().as_deref() {
                None => format!(
                    "cli-render-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ),
                Some("--request-id") => args.next().unwrap_or_else(|| usage()),
                Some(_) => usage(),
            };
            if args.next().is_some() {
                usage();
            }
            if format != "wav" {
                usage();
            }
            match ProjectDocument::load(&project_path) {
                Ok(project) => {
                    if output_dir.trim().is_empty() || output_dir.len() > 4096 {
                        eprintln!("{{\"ok\":false,\"error\":\"invalid_output_dir\"}}");
                        std::process::exit(1);
                    }
                    let request = ProtocolRequest {
                        protocol: PROTOCOL_VERSION.into(),
                        request_id,
                        client: "aura-cli".into(),
                        command: CommandDocument {
                            schema_version: 1,
                            command_version: 1,
                            transaction: "cli-render-stems".into(),
                            permission: Permission::SystemWrite,
                            expected_generation: None,
                            expected_audio_generation: None,
                            actions: vec![CommandAction::BounceStems {
                                output_dir: output_dir.clone(),
                                format: if format == "wav" {
                                    0
                                } else if format == "wave64" {
                                    1
                                } else {
                                    2
                                },
                                track_ids: Vec::new(),
                                tail_seconds: 2.0,
                                pre_fader: false,
                                include_inserts: true,
                            }],
                        },
                    };
                    if let Err(error) = validate_request_envelope(&request) {
                        eprintln!(
                            "{{\"ok\":false,\"error\":{}}}",
                            serde_json::to_string(&error.code).unwrap()
                        );
                        std::process::exit(1);
                    }
                    let paths = match render_project_stems(&project_path, &output_dir) {
                        Ok(paths) => paths,
                        Err(error) => {
                            eprintln!(
                                "{{\"ok\":false,\"error\":{}}}",
                                serde_json::to_string(&error).unwrap()
                            );
                            std::process::exit(1);
                        }
                    };
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "operation": "render.stems",
                            "project": project.project_id,
                            "track_count": project.tracks.len(),
                            "files": paths,
                            "request": request,
                        }))
                        .unwrap()
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error.to_string()).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("ci") if args.next().as_deref() == Some("verify") => {
            let Some(path) = args.next() else {
                usage();
            };
            let max_peak = args
                .next()
                .map(|v| v.parse::<f32>())
                .transpose()
                .unwrap_or_else(|_| usage())
                .unwrap_or(1.0);
            let min_rms = args
                .next()
                .map(|v| v.parse::<f32>())
                .transpose()
                .unwrap_or_else(|_| usage())
                .unwrap_or(0.0);
            if args.next().is_some() {
                usage();
            }
            match read_pcm_wav(&path)
                .map_err(|e| e.to_string())
                .and_then(|samples| {
                    aura_core_bridge::ci_audio_verify::verify(&samples, max_peak, min_rms)
                        .map_err(str::to_owned)
                }) {
                Ok(report) => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true, "operation": "ci.verify", "path": path,
                        "sample_count": report.sample_count, "peak": report.peak,
                        "rms": report.rms, "sha256": report.sha256,
                    }))
                    .unwrap()
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Some("validate") if args.next().is_none() => {
            let mut input = String::new();
            if io::stdin().read_to_string(&mut input).is_err() {
                std::process::exit(1);
            }
            let mut failed = false;
            for line in input.lines().filter(|line| !line.trim().is_empty()) {
                let result = serde_json::from_str::<ProtocolRequest>(line)
                    .map_err(|error| format!("invalid_json: {error}"))
                    .and_then(|request| {
                        validate_request_envelope(&request).map_err(|error| error.code)
                    });
                match result {
                    Ok(()) => println!("{{\"ok\":true}}"),
                    Err(code) => {
                        println!(
                            "{{\"ok\":false,\"error\":{}}}",
                            serde_json::to_string(&code).unwrap()
                        );
                        failed = true;
                    }
                }
            }
            if failed {
                std::process::exit(1);
            }
        }
        Some("diff") if args.next().is_none() => {
            let mut input = String::new();
            if io::stdin().read_to_string(&mut input).is_err() {
                std::process::exit(1);
            }
            match serde_json::from_str::<CommandDocument>(&input)
                .map_err(|error| error.to_string())
                .and_then(validate)
            {
                Ok(command) => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true,
                        "operation": "dry_run",
                        "dry_run": true,
                        "transaction": command.transaction,
                        "destructive": command.destructive,
                        "mutation_class": command.mutation_class,
                        "actions": diff(&command),
                    }))
                    .unwrap()
                ),
                Err(error) => {
                    eprintln!(
                        "{{\"ok\":false,\"error\":{}}}",
                        serde_json::to_string(&error).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}
