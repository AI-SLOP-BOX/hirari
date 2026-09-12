use super::usage;
use aura_core_bridge::production_session::{ProductionRevision, ProductionSession};
use aura_core_bridge::project::ProjectDocument;

pub(super) fn try_handle(raw_args: &[String]) -> bool {
    if raw_args.first().map(String::as_str) == Some("project") {
        match raw_args.get(1).map(String::as_str) {
            Some("production") if raw_args.get(2).map(String::as_str) == Some("revision") => {
                let Some(path) = raw_args.get(3) else {
                    usage();
                };
                let Some(message) = raw_args.get(4) else {
                    usage();
                };
                if message.trim().is_empty() || message.len() > 512 || raw_args.len() > 10 {
                    usage();
                }
                let author = raw_args.get(5).cloned().unwrap_or_else(|| "unknown".into());
                if author.trim().is_empty() || author.len() > 256 {
                    usage();
                }
                let mut audio_snapshot = None;
                let mut visual_snapshot = None;
                let mut option_index = 6;
                while option_index < raw_args.len() {
                    let Some(value) = raw_args.get(option_index + 1) else {
                        usage();
                    };
                    if value.is_empty() || value.len() > 256 {
                        usage();
                    }
                    match raw_args[option_index].as_str() {
                        "--audio-hash" if audio_snapshot.is_none() => {
                            audio_snapshot = Some(value.clone())
                        }
                        "--vfx-hash" if visual_snapshot.is_none() => {
                            visual_snapshot = Some(value.clone())
                        }
                        _ => usage(),
                    }
                    option_index += 2;
                }
                let mut session = match ProductionSession::load_sidecar(path) {
                    Ok(Some(session)) => session,
                    Ok(None) => {
                        eprintln!("{{\"ok\":false,\"error\":\"production_session_missing\"}}");
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
                let revision = session
                    .revisions
                    .last()
                    .map(|item| item.revision.saturating_add(1))
                    .unwrap_or(1);
                session.revisions.push(ProductionRevision {
                    revision,
                    message: message.clone(),
                    author,
                    timestamp_unix: chrono_unix_seconds(),
                    audio_snapshot,
                    visual_snapshot,
                });
                match session.save_sidecar(path) {
                        Ok(sidecar) => println!("{{\"ok\":true,\"operation\":\"project.production.revision\",\"revision\":{},\"sidecar\":{}}}", revision, serde_json::to_string(sidecar.to_string_lossy().as_ref()).unwrap()),
                        Err(error) => { eprintln!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(&error.to_string()).unwrap()); std::process::exit(1); }
                    }
                return true;
            }
            Some("production")
                if matches!(
                    raw_args.get(2).map(String::as_str),
                    Some("inspect") | Some("init")
                ) =>
            {
                let Some(path) = raw_args.get(3) else {
                    usage();
                };
                if raw_args[2] == "inspect" {
                    if raw_args.len() != 4 {
                        usage();
                    }
                    match ProductionSession::load_sidecar(path) {
                        Ok(Some(session)) => {
                            println!("{}", serde_json::to_string_pretty(&session).unwrap())
                        }
                        Ok(None) => println!(
                            "{{\"ok\":true,\"production_session\":null,\"project\":{}}}",
                            serde_json::to_string(path).unwrap()
                        ),
                        Err(error) => {
                            eprintln!(
                                "{{\"ok\":false,\"error\":{}}}",
                                serde_json::to_string(&error.to_string()).unwrap()
                            );
                            std::process::exit(1);
                        }
                    }
                } else {
                    if raw_args.len() > 5 {
                        usage();
                    }
                    let fps = raw_args
                        .get(4)
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(24.0);
                    if !fps.is_finite() || !(1.0..=240.0).contains(&fps) {
                        usage();
                    }
                    let project = match ProjectDocument::load(path) {
                        Ok(project) => project,
                        Err(error) => {
                            eprintln!(
                                "{{\"ok\":false,\"error\":{}}}",
                                serde_json::to_string(&error.to_string()).unwrap()
                            );
                            std::process::exit(1);
                        }
                    };
                    let Some(session) = ProductionSession::new(
                        &project.project_id,
                        aura_core_bridge::production_timeline::TimelineRate {
                            sample_rate: project.sample_rate,
                            frame_rate: fps,
                        },
                        f64::from(project.metadata.bpm),
                    ) else {
                        usage();
                    };
                    match session.save_sidecar(path) {
                            Ok(sidecar) => println!("{{\"ok\":true,\"operation\":\"project.production.init\",\"sidecar\":{}}}", serde_json::to_string(sidecar.to_string_lossy().as_ref()).unwrap()),
                            Err(error) => { eprintln!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(&error.to_string()).unwrap()); std::process::exit(1); }
                        }
                }
                return true;
            }
            Some("inspect") | Some("manifest") | Some("load") => {
                let Some(path) = raw_args.get(2) else {
                    usage();
                };
                if raw_args.len() != 3 {
                    usage();
                }
                let result = ProjectDocument::load(path).and_then(|project| {
                    if raw_args[1] == "inspect" {
                        Ok(serde_json::json!({
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
                        }))
                    } else if raw_args[1] == "manifest" {
                        project.reproducibility_manifest()
                    } else {
                        Ok(serde_json::json!({
                            "ok": true,
                            "operation": "project.load",
                            "path": path,
                            "track_count": project.tracks.len(),
                            "region_count": project.regions.len(),
                        }))
                    }
                });
                match result {
                    Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
                    Err(error) => {
                        eprintln!(
                            "{{\"ok\":false,\"error\":{}}}",
                            serde_json::to_string(&error.to_string()).unwrap()
                        );
                        std::process::exit(1);
                    }
                }
                return true;
            }
            _ => {}
        }
    }
    if raw_args.first().map(String::as_str) == Some("track") {
        match raw_args.get(1).map(String::as_str) {
            Some("rename") | Some("delete") => {
                let (Some(path), Some(id_text)) = (raw_args.get(2), raw_args.get(3)) else {
                    usage();
                };
                let Some(track_id) = id_text.parse::<u32>().ok() else {
                    usage();
                };
                let result = if raw_args[1] == "rename" {
                    if raw_args.len() != 5 {
                        usage();
                    }
                    ProjectDocument::load(path).and_then(|mut project| {
                        project.set_track_name(track_id, raw_args[4].clone())?;
                        project.save_atomic(path)
                    })
                } else {
                    if raw_args.len() != 4 {
                        usage();
                    }
                    ProjectDocument::load(path).and_then(|mut project| {
                        project.remove_track(track_id)?;
                        project.save_atomic(path)
                    })
                };
                match result {
                    Ok(()) => println!(
                        "{{\"ok\":true,\"operation\":\"track.{}\",\"track_id\":{track_id}}}",
                        raw_args[1]
                    ),
                    Err(error) => {
                        eprintln!(
                            "{{\"ok\":false,\"error\":{}}}",
                            serde_json::to_string(&error.to_string()).unwrap()
                        );
                        std::process::exit(1);
                    }
                }
                return true;
            }
            _ => {}
        }
    }
    false
}

fn chrono_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}
