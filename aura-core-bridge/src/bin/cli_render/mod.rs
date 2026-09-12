use super::usage;
use aura_core_bridge::command_api::{
    validate_request_envelope, CommandAction, CommandDocument, Permission, ProtocolRequest,
    PROTOCOL_VERSION,
};
use aura_core_bridge::project::ProjectDocument;

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
    let format = raw_args.get(4).map(String::as_str).unwrap_or("wav");
    if format != "wav"
        || (raw_args.len() > 5 && raw_args.get(5).map(String::as_str) != Some("--request-id"))
    {
        usage();
    }
    let request_id = if raw_args.len() == 5 {
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
    let paths = match super::render_project_stems(project_path, output_dir) {
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
    true
}
