use super::usage;
use hirari_core_bridge::command_api::{
    validate_request_envelope, CommandAction, CommandDocument, Permission, ProtocolRequest,
    PROTOCOL_VERSION,
};
use hirari_core_bridge::project::ProjectDocument;

/// Handle render subcommands from an immutable argv slice. The legacy
/// iterator dispatcher consumes values while evaluating match guards, which
/// made `render stems` fall through to usage even with valid arguments.
pub(super) fn try_handle(raw_args: &[String]) -> bool {
    if raw_args.first().map(String::as_str) != Some("render")
        || raw_args.get(1).map(String::as_str) != Some("stems")
    {
        return false;
    }
    let (Some(project_path), Some(output_dir)) = (raw_args.get(2), raw_args.get(3)) else {
        usage();
    };
    // `render stems <project> <output-dir>` is the documented minimal form,
    // so the format argument is optional and defaults to `wav`. The previous
    // dispatcher only accepted 5 or 7 arguments, which made the documented
    // 4-argument form print usage and exit 2 even though it is valid.
    let format = raw_args.get(4).map(String::as_str).unwrap_or("wav");
    if format != "wav"
        || (raw_args.len() > 5 && raw_args.get(5).map(String::as_str) != Some("--request-id"))
    {
        usage();
    }
    let request_id = if raw_args.len() == 4 || raw_args.len() == 5 {
        format!(
            "cli-render-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    } else if raw_args.len() == 7 {
        raw_args[6].clone()
    } else {
        usage();
    };
    if output_dir.trim().is_empty() || output_dir.len() > 4096 {
        eprintln!("{{\"ok\":false,\"error\":\"invalid_output_dir\"}}");
        std::process::exit(1);
    }
    let project = match ProjectDocument::load(project_path) {
        Ok(project) => project,
        Err(error) => {
            eprintln!(
                "{{\"ok\":false,\"error\":{}}}",
                serde_json::to_string(&error.to_string()).unwrap()
            );
            std::process::exit(1);
        }
    };
    let request = ProtocolRequest {
        protocol: PROTOCOL_VERSION.into(),
        request_id,
        client: "hirari-cli".into(),
        command: CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "cli-render-stems".into(),
            permission: Permission::SystemWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::BounceStems {
                output_dir: output_dir.clone(),
                format: 0,
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
    // Stems must come from the same native bounce graph the UI's EXPORT STEMS
    // uses, otherwise CLI stems quietly omit everything the engine adds: track
    // routing, gain/pan, inserts, fades and MIDI-played regions. The previous
    // implementation was a private re-implementation that re-read region WAVs
    // with `read_pcm_wav` and summed raw samples, so its output disagreed with
    // the product's own render of the same project.
    if let Err(error) = std::fs::create_dir_all(output_dir) {
        eprintln!(
            "{{\"ok\":false,\"error\":{}}}",
            serde_json::to_string(&error.to_string()).unwrap()
        );
        std::process::exit(1);
    }
    let core = match hirari_core_bridge::HirariCore::new_offline() {
        Ok(core) => core,
        Err(error) => {
            eprintln!(
                "{{\"ok\":false,\"error\":{}}}",
                serde_json::to_string(&error.to_string()).unwrap()
            );
            std::process::exit(1);
        }
    };
    if let Err(error) = core.load_project_v2(project_path) {
        eprintln!(
            "{{\"ok\":false,\"error\":{}}}",
            serde_json::to_string(&error.to_string()).unwrap()
        );
        std::process::exit(1);
    }
    // format 0 = wav, all tracks, 2s tail, post-fader with inserts — the same
    // values carried by the `BounceStems` action in the request envelope above.
    let diagnostic = core.bounce_stems_diagnostic_json(output_dir, 0, &[], 2.0, false, true);
    let paths: Vec<String> = match serde_json::from_str::<serde_json::Value>(&diagnostic) {
        Ok(value) if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) => value
            .get("outputs")
            .and_then(serde_json::Value::as_array)
            .map(|outputs| {
                outputs
                    .iter()
                    .filter_map(|output| output.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        Ok(value) => {
            // Report the engine's structured failure verbatim rather than a
            // generic message: the codes name the actual cause (for example
            // `stem_output_exists` when the destination already holds stems).
            eprintln!("{}", serde_json::to_string_pretty(&value).unwrap());
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!(
                "{{\"ok\":false,\"error\":{}}}",
                serde_json::to_string(&error.to_string()).unwrap()
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
    true
}
