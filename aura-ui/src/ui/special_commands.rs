use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::slint_ui::{
    clamp_selection_index, ui_error_message, unix_time_millis, AppWindow, UiErrorKind, Z_Track,
};
use crate::ui::track_model::sync_tracks_from_engine;

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    render_started_ms: &AtomicU64,
) -> bool {
    // MixConsole snapshots are available from the command palette as well as
    // the headless command API.  The core owns the capture format; the UI
    // only supplies the human-readable scene name and refreshes its model
    // after an apply.
    if let Some(rest) = command.strip_prefix("MIX SNAPSHOT ") {
        let mut parts = rest.splitn(2, ' ');
        let operation = parts.next().unwrap_or_default();
        let argument = parts.next().unwrap_or_default().trim();
        let result = match operation {
            "CAPTURE" if !argument.is_empty() => core.capture_mix_snapshot_json(argument),
            "APPLY" => match argument.parse::<usize>() {
                Ok(index) => {
                    let result = core.apply_mix_snapshot_json(index);
                    if serde_json::from_str::<serde_json::Value>(&result)
                        .ok()
                        .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                        == Some(true)
                    {
                        sync_tracks_from_engine(tracks, core);
                    }
                    result
                }
                Err(_) => "{\"ok\":false,\"code\":\"invalid_snapshot_index\"}".into(),
            },
            "RECALL" => match argument.parse::<usize>() {
                Ok(index) => core.recall_mix_snapshot_json(index),
                Err(_) => "{\"ok\":false,\"code\":\"invalid_snapshot_index\"}".into(),
            },
            "DIFF" => {
                let mut indices = argument.split_whitespace();
                match (
                    indices.next().and_then(|value| value.parse::<usize>().ok()),
                    indices.next().and_then(|value| value.parse::<usize>().ok()),
                ) {
                    (Some(first), Some(second)) => core.diff_mix_snapshots_json(first, second),
                    _ => "{\"ok\":false,\"code\":\"invalid_snapshot_indices\"}".into(),
                }
            }
            "LIST" if argument.is_empty() => core.mix_snapshot_catalog_json(),
            "REMOVE" => match argument.parse::<usize>() {
                Ok(index) => core.remove_mix_snapshot_json(index),
                Err(_) => "{\"ok\":false,\"code\":\"invalid_snapshot_index\"}".into(),
            },
            _ => "{\"ok\":false,\"code\":\"unknown_snapshot_operation\"}".into(),
        };
        let status = serde_json::from_str::<serde_json::Value>(&result).ok();
        if status
            .as_ref()
            .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
            == Some(true)
        {
            let label = status
                .as_ref()
                .and_then(|value| value.get("name").and_then(serde_json::Value::as_str))
                .unwrap_or(operation);
            ui.set_last_action(format!("MIX SNAPSHOT {operation}: {label}").into());
        } else {
            let code = status
                .as_ref()
                .and_then(|value| value.get("code").and_then(serde_json::Value::as_str))
                .unwrap_or("failed");
            ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Engine,
                    &format!("mix snapshot {operation} failed: {code}"),
                )
                .into(),
            );
        }
        return true;
    }

    // Expose the complete Control Room control-plane to keyboard/AI users;
    // the compact top bar intentionally only has DIM/TALK/MON toggles.
    if let Some(rest) = command.strip_prefix("CONTROL ROOM ") {
        let tokens = rest.split_whitespace().collect::<Vec<_>>();
        let result = match tokens.as_slice() {
            ["DIM", state] => parse_toggle(state).map(|enabled| {
                core.set_control_room_dim(enabled);
                core.control_room_dimmed() == enabled
            }),
            ["TALKBACK", state] => parse_toggle(state).map(|enabled| {
                core.set_control_room_talkback(enabled, 1.0);
                core.control_room_talkback_enabled() == enabled
            }),
            ["TALKBACK", state, gain] => parse_toggle(state)
                .and_then(|enabled| gain.parse::<f32>().ok().map(|gain| (enabled, gain)))
                .map(|(enabled, gain)| {
                    let accepted = gain.is_finite() && (-1.0..=2.0).contains(&gain);
                    if accepted {
                        core.set_control_room_talkback(enabled, gain);
                    }
                    accepted && core.control_room_talkback_enabled() == enabled
                }),
            ["OUTPUT", index, state] => index.parse::<u32>().ok().and_then(|index| {
                parse_toggle(state)
                    .map(|enabled| core.set_control_room_speaker_enabled(index, enabled))
            }),
            ["SELECT", index] => index
                .parse::<u32>()
                .ok()
                .map(|index| core.select_control_room_output(index)),
            ["CUE", id, gain, state] => id
                .parse::<u32>()
                .ok()
                .zip(gain.parse::<f32>().ok())
                .zip(parse_toggle(state))
                .map(|((id, gain), enabled)| {
                    gain.is_finite() && core.upsert_control_room_cue(id, gain, enabled)
                }),
            ["CUE", id, state] => {
                id.parse::<u32>()
                    .ok()
                    .zip(parse_toggle(state))
                    .map(|(id, enabled)| {
                        let gain = core.control_room_cue_gain(id);
                        core.upsert_control_room_cue(id, gain, enabled)
                    })
            }
            ["REFERENCE", state] => {
                parse_toggle(state).map(|enabled| core.set_control_room_reference_enabled(enabled))
            }
            _ => None,
        };
        match result {
            Some(true) => ui.set_last_action(format!("CONTROL ROOM {rest}: OK").into()),
            Some(false) => ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Engine,
                    &format!("CONTROL ROOM {rest}: rejected"),
                )
                .into(),
            ),
            None => ui.set_last_action(
                ui_error_message(UiErrorKind::Engine, "invalid Control Room command").into(),
            ),
        }
        return true;
    }

    if command == "RENDER QUEUE STATUS" {
        let result = core.render_queue_status_json();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&result) {
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                let state = value
                    .get("state")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let progress = value
                    .get("progress")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0)
                    * 100.0;
                ui.set_last_action(
                    format!("RENDER QUEUE: state={state} progress={progress:.1}%").into(),
                );
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "render queue unavailable").into(),
                );
            }
        } else {
            ui.set_last_action(
                ui_error_message(UiErrorKind::Render, "invalid render queue status").into(),
            );
        }
        return true;
    }

    if command == "MIDI CLOCK STATUS" {
        let result = core.midi_clock_status_json();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&result) {
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                let ticks = value
                    .get("ticks")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let bpm = value
                    .get("bpm")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                ui.set_last_action(format!("MIDI CLOCK: {ticks} ticks · {bpm:.2} BPM").into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Engine, "MIDI Clock unavailable").into(),
                );
            }
        }
        return true;
    }
    if let Some(rest) = command.strip_prefix("MIDI CLOCK TICK ") {
        let mut values = rest.split_whitespace();
        let timestamp = values.next().and_then(|value| value.parse::<u64>().ok());
        let bpm = values.next().and_then(|value| value.parse::<f64>().ok());
        match timestamp {
            Some(timestamp) => {
                let result = core.midi_clock_tick_json(timestamp, bpm);
                let ok = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    == Some(true);
                ui.set_last_action(if ok {
                    "MIDI CLOCK TICK ACCEPTED".into()
                } else {
                    ui_error_message(UiErrorKind::Engine, "MIDI Clock tick rejected").into()
                });
            }
            None => ui.set_last_action(
                ui_error_message(UiErrorKind::Engine, "invalid MIDI Clock timestamp").into(),
            ),
        }
        return true;
    }
    if command == "SYNC STATUS" {
        let result = core.external_sync_status_json();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&result) {
            let ok = value.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
            ui.set_last_action(if ok {
                format!(
                    "SYNC: {} / {} / running={}",
                    value["protocol"], value["source"], value["running"]
                )
                .into()
            } else {
                ui_error_message(UiErrorKind::Engine, "external sync unavailable").into()
            });
        }
        return true;
    }
    if let Some(rest) = command.strip_prefix("SYNC CONFIG ") {
        let tokens = rest.split_whitespace().collect::<Vec<_>>();
        if tokens.len() == 3 {
            if let Some(enabled) = parse_toggle(tokens[2]) {
                let result = core.configure_external_sync_json(tokens[0], tokens[1], enabled);
                let ok = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    == Some(true);
                ui.set_last_action(if ok {
                    "EXTERNAL SYNC CONFIGURED".into()
                } else {
                    ui_error_message(UiErrorKind::Engine, "external sync configuration rejected")
                        .into()
                });
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Engine, "invalid sync enable state").into(),
                );
            }
        } else {
            ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Engine,
                    "SYNC CONFIG requires protocol source ON|OFF",
                )
                .into(),
            );
        }
        return true;
    }
    if let Some(rest) = command.strip_prefix("SYNC MMC ") {
        let bytes = rest
            .split_whitespace()
            .filter_map(|part| u8::from_str_radix(part.trim_start_matches("0x"), 16).ok())
            .collect::<Vec<_>>();
        let result = core.external_sync_mmc_json(&bytes);
        let ok = serde_json::from_str::<serde_json::Value>(&result)
            .ok()
            .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
            == Some(true);
        ui.set_last_action(if ok {
            "MMC SYNC ACCEPTED".into()
        } else {
            ui_error_message(UiErrorKind::Engine, "MMC packet rejected").into()
        });
        return true;
    }
    if let Some(rest) = command.strip_prefix("SYNC MTC ") {
        let mut parts = rest.split_whitespace();
        let tc = parts.next().unwrap_or_default();
        let fps = parts.next().and_then(|value| value.parse::<u8>().ok());
        let fields = tc
            .split([':', ';'])
            .filter_map(|value| value.parse::<u8>().ok())
            .collect::<Vec<_>>();
        if fields.len() == 4 {
            if let Some(fps) = fps {
                let result =
                    core.external_sync_mtc_json(fields[0], fields[1], fields[2], fields[3], fps);
                let ok = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    == Some(true);
                ui.set_last_action(if ok {
                    "MTC SYNC ACCEPTED".into()
                } else {
                    ui_error_message(UiErrorKind::Engine, "MTC timecode rejected").into()
                });
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Engine, "MTC requires a frame rate").into(),
                );
            }
        } else {
            ui.set_last_action(
                ui_error_message(UiErrorKind::Engine, "MTC requires HH:MM:SS:FF FPS").into(),
            );
        }
        return true;
    }

    match command {
        "STEM SEPARATION (AI)" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            if let Some(track) = tracks.row_data(selected) {
                core.execute_vocal_remover(track.id.max(0) as u32);
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Engine, "stem separation failed: no track")
                        .into(),
                );
            }
            true
        }
        "EXPORT ATMOS MASTER" => {
            if core.start_render_async() {
                render_started_ms.store(unix_time_millis(), Ordering::Release);
                ui.set_is_rendering(true);
                ui.set_render_progress(0.0);
                ui.set_render_progress_available(false);
                ui.set_render_progress_indeterminate(true);
                ui.set_render_state("QUEUED".into());
                ui.set_render_error("WAITING FOR ENGINE STATUS".into());
                ui.set_last_action("RENDER QUEUED".into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "busy or failed to queue").into(),
                );
            }
            true
        }
        "EXPORT MIDI" => {
            let project_path = ui.get_project_path().to_string();
            if project_path.trim().is_empty() {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "save the project before exporting MIDI",
                    )
                    .into(),
                );
                return true;
            }
            let output = std::path::Path::new(&project_path).with_extension("mid");
            match core.export_midi_file(output.to_string_lossy().as_ref()) {
                Ok(notes) => ui.set_last_action(
                    format!("MIDI EXPORTED: {} notes · {}", notes, output.display()).into(),
                ),
                Err(error) => ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("MIDI export failed: {error}"),
                    )
                    .into(),
                ),
            }
            true
        }
        _ => false,
    }
}

fn parse_toggle(value: &str) -> Option<bool> {
    match value.to_ascii_uppercase().as_str() {
        "ON" | "1" | "TRUE" => Some(true),
        "OFF" | "0" | "FALSE" => Some(false),
        _ => None,
    }
}
