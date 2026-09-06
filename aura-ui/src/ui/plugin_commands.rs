use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::process::Command;
use std::rc::Rc;

use crate::slint_ui::{
    clamp_selection_index, clear_plugin_blacklist, load_plugin_favorites,
    scan_installed_plugin_count, sync_tracks_from_engine, ui_error_message, ui_error_with_action,
    AppWindow, UiErrorKind, Z_Plugin_Entry, Z_Track,
};

fn launch_openutau(source: &std::path::Path) -> Result<(), String> {
    aura_core_bridge::openutau::validate_source(&source.to_string_lossy())
        .map_err(|error| format!("INVALID SCORE · {error}"))?;
    let app = aura_core_bridge::openutau::application_path();
    if !app.is_dir() {
        return Err(format!("OPENUTAU NOT FOUND · {}", app.display()));
    }
    Command::new("open")
        .arg("-a")
        .arg(&app)
        .arg(source)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("OPENUTAU LAUNCH FAILED · {error}"))
}

fn set_openutau_note_preview(ui: &AppWindow, source: &std::path::Path) {
    let notes = aura_core_bridge::openutau::note_preview(&source.to_string_lossy());
    ui.set_openutau_notes(slint::ModelRc::new(VecModel::from(
        notes
            .into_iter()
            .map(Into::into)
            .collect::<Vec<slint::SharedString>>(),
    )));
}

pub fn refresh_plugin_catalog_for_ui(core: &AuraCore, ui: &AppWindow) -> usize {
    let favorites = load_plugin_favorites();
    let value = serde_json::from_str::<serde_json::Value>(&core.installed_plugin_catalog_json())
        .unwrap_or_default();
    let mut entries = vec![
        Z_Plugin_Entry {
            id: "aura.internal.limiter".into(),
            name: "Aura Limiter".into(),
            format: "INTERNAL".into(),
            path: "Aura/Limiter".into(),
            capability: "native realtime dynamics".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            installed: true,
            sandboxed: false,
            favorite: favorites.contains("aura.internal.limiter"),
            visible: true,
        },
        Z_Plugin_Entry {
            id: "aura.internal.compressor".into(),
            name: "Aura Compressor".into(),
            format: "INTERNAL".into(),
            path: "Aura/Compressor".into(),
            capability: "native realtime dynamics · assistant-ready".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            installed: true,
            sandboxed: false,
            favorite: favorites.contains("aura.internal.compressor"),
            visible: true,
        },
    ];
    entries.extend(
        value
            .get("plugins")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|plugin| {
                Some(Z_Plugin_Entry {
                    id: plugin.get("id")?.as_str()?.into(),
                    name: plugin.get("name")?.as_str()?.into(),
                    format: plugin.get("format")?.as_str()?.to_ascii_uppercase().into(),
                    path: plugin.get("path")?.as_str()?.into(),
                    capability: plugin.get("capability")?.as_str()?.into(),
                    binary_hash: plugin.get("binary_hash")?.as_str()?.into(),
                    binary_bytes: plugin.get("binary_bytes")?.as_u64()?.min(i32::MAX as u64) as i32,
                    installed: plugin.get("installed")?.as_bool()?,
                    sandboxed: plugin.get("sandboxed")?.as_bool()?,
                    favorite: favorites.contains(plugin.get("id")?.as_str()?),
                    visible: true,
                })
            })
            .collect::<Vec<_>>(),
    );
    let count = entries.len();
    ui.set_plugin_catalog(slint::ModelRc::new(slint::VecModel::from(entries)));
    crate::ui::plugin::filter_plugin_catalog(ui, ui.get_plugin_catalog_query().as_str());
    ui.set_plugin_catalog_status(format!("{} PLUGINS · CLAP/VST3/AU", count).into());
    count
}

pub fn insert_catalog_plugin(
    alias: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
) {
    let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
    let Some(track) = tracks.row_data(selected) else {
        ui.set_last_action("PLUGIN INSERT: NO TARGET TRACK".into());
        return;
    };
    let result = if alias == "aura.internal.compressor" || alias == "Aura/Compressor" {
        core.add_plugin_diagnostic_json(track.id.max(0) as u32, 1)
    } else if alias == "aura.internal.limiter" || alias == "Aura/Limiter" {
        core.add_plugin_diagnostic_json(track.id.max(0) as u32, 0)
    } else {
        core.add_named_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, alias)
    };
    let value = serde_json::from_str::<serde_json::Value>(&result).unwrap_or_default();
    let ok = value
        .get("result")
        .and_then(|v| v.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if ok {
        sync_tracks_from_engine(tracks, core);
        ui.set_last_action(
            if alias.starts_with("aura.internal.") || alias.starts_with("Aura/") {
                "PLUGIN INSERTED · NATIVE DSP READY".into()
            } else {
                "PLUGIN INSERTED · SANDBOX READY".into()
            },
        );
    } else {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .get("result")
                    .and_then(|v| v.get("code"))
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or("plugin_host_unavailable");
        ui.set_last_action(format!("PLUGIN INSERT FAILED · {code}").into());
    }
}

fn copy_plugin_bundle(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_plugin_bundle(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, target).map(|_| ())
    }
}

fn install_target_path(
    destination_dir: &std::path::Path,
    source: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let name = source.file_name()?.to_owned();
    let target = destination_dir.join(&name);
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    );
    let temporary =
        destination_dir.join(format!(".aura-plugin-{nonce}-{}", name.to_string_lossy()));
    Some((temporary, target))
}

fn install_selected_plugin(core: &AuraCore, ui: &AppWindow) {
    let Some(source) = rfd::FileDialog::new()
        .add_filter("Aura Plugin", &["clap", "vst3", "component"])
        .pick_file()
    else {
        ui.set_last_action("PLUGIN INSTALL CANCELLED".into());
        return;
    };
    let Some(format) = source
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
    else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · UNKNOWN FORMAT".into());
        return;
    };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        ui.set_last_action("PLUGIN INSTALL FAILED · HOME DIRECTORY UNAVAILABLE".into());
        return;
    };
    let destination_dir = match format.as_str() {
        "clap" => home.join("Library/Audio/Plug-Ins/CLAP"),
        "vst3" => home.join("Library/Audio/Plug-Ins/VST3"),
        "component" => home.join("Library/Audio/Plug-Ins/Components"),
        _ => {
            ui.set_last_action("PLUGIN INSTALL REJECTED · UNSUPPORTED FORMAT".into());
            return;
        }
    };
    let Ok(metadata) = std::fs::symlink_metadata(&source) else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · SOURCE NOT FOUND".into());
        return;
    };
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        ui.set_last_action("PLUGIN INSTALL REJECTED · SYMLINK OR INVALID SOURCE".into());
        return;
    }
    let Some((temporary, target)) = install_target_path(&destination_dir, &source) else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · INVALID FILE NAME".into());
        return;
    };
    if target.exists() {
        ui.set_last_action("PLUGIN INSTALL REJECTED · EXISTING FILE NOT OVERWRITTEN".into());
        return;
    }
    let install_result = std::fs::create_dir_all(&destination_dir)
        .and_then(|()| copy_plugin_bundle(&source, &temporary))
        .and_then(|()| std::fs::rename(&temporary, &target));
    if let Err(error) = install_result {
        let _ = std::fs::remove_dir_all(&temporary);
        let _ = std::fs::remove_file(&temporary);
        ui.set_last_action(format!("PLUGIN INSTALL FAILED · {error}").into());
        return;
    }
    let count = refresh_plugin_catalog_for_ui(core, ui);
    ui.set_bot_view(16);
    ui.set_mx_open(true);
    ui.set_last_action(format!("PLUGIN INSTALLED · {} AVAILABLE", count).into());
}

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
) -> bool {
    if crate::ui::plugin::invoke_extension_from_palette(command, core, ui) {
        return true;
    }
    if let Some(path) = command.strip_prefix("INSERT PLUGIN PATH ") {
        let path = path.trim();
        if path.is_empty() {
            ui.set_last_action("PLUGIN INSERT FAILED · MISSING PATH".into());
            return true;
        }
        let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
        let Some(track) = tracks.row_data(selected) else {
            ui.set_last_action("PLUGIN INSERT FAILED · NO TARGET TRACK".into());
            return true;
        };
        let result = core.add_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, path);
        let value = serde_json::from_str::<serde_json::Value>(&result).unwrap_or_default();
        let ok = value.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
        if ok {
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action("PLUGIN INSERTED · PATH ADMITTED BY SANDBOX".into());
        } else {
            let code = value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("plugin_admission_failed");
            ui.set_last_action(format!("PLUGIN INSERT FAILED · {code}").into());
        }
        return true;
    }
    match command {
        command if command.strip_prefix("INSERT PLUGIN ").is_some() => {
            let alias = command
                .strip_prefix("INSERT PLUGIN ")
                .unwrap_or_default()
                .trim();
            if alias.is_empty() {
                ui.set_last_action("PLUGIN INSERT FAILED · MISSING ID".into());
            } else {
                insert_catalog_plugin(alias, core, ui, tracks);
            }
            true
        }
        "INSTALL PLUGIN" | "ADD PLUGIN FILE" => {
            install_selected_plugin(core, ui);
            true
        }
        "REMOVE ACTIVE PLUGIN" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "remove failed: no track selected")
                        .into(),
                );
                return true;
            };
            let plugin_index = ui.get_fx_active_id();
            if plugin_index < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "remove failed: no plugin selected")
                        .into(),
                );
                return true;
            }
            let removed = core.remove_plugin(track.id.max(0) as u32, plugin_index as u32);
            if removed {
                sync_tracks_from_engine(tracks, core);
                ui.set_fx_active_id(-1);
                ui.set_last_action(format!("REMOVED PLUGIN {}", plugin_index).into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "plugin removal rejected by Core").into(),
                );
            }
            true
        }
        "CLEAR PLUGIN BLACKLIST" | "RESET PLUGIN BLACKLIST" => {
            ui.set_last_action(if clear_plugin_blacklist() {
                "PLUGIN BLACKLIST CLEARED · RESCAN READY".into()
            } else {
                ui_error_message(UiErrorKind::Plugin, "could not clear plugin blacklist").into()
            });
            true
        }
        "RETRY PLUGINS" | "RECOVER PLUGINS" => {
            let recovered = core.recover_sandboxed_plugins();
            let remaining = core
                .sandbox_statuses()
                .iter()
                .filter(|status| !status.alive || status.failure != 0 || status.is_quarantined())
                .count();
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action(
                if remaining == 0 {
                    format!("PLUGIN SANDBOX: {} RECOVERED", recovered)
                } else {
                    format!(
                        "PLUGIN SANDBOX: {} RECOVERED · {} FAILURE(S) REMAIN",
                        recovered, remaining
                    )
                }
                .into(),
            );
            true
        }
        "scan_plugins" | "SCAN PLUGINS" => {
            let (discovered, rejected) = scan_installed_plugin_count();
            let catalog_count = refresh_plugin_catalog_for_ui(core, ui);
            ui.set_bot_view(16);
            ui.set_mx_open(true);
            ui.set_pr_open(false);
            ui.set_last_action(format!("PLUGIN CATALOG: {catalog_count} AVAILABLE · {discovered} DISCOVERED · {rejected} REJECTED").into());
            true
        }
        "LOAD VITAL" | "INSERT VITAL" | "LOAD SURGE XT" | "INSERT SURGE XT" => {
            let alias = if command.contains("VITAL") {
                "Vital"
            } else {
                "Surge XT"
            };
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(format!("{alias}: NO TARGET TRACK").into());
                return true;
            };
            let result =
                core.add_named_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, alias);
            let ok = serde_json::from_str::<serde_json::Value>(&result)
                .ok()
                .and_then(|value| value.get("result").cloned().or(Some(value)))
                .and_then(|value| value.get("ok").cloned())
                .and_then(|value| value.as_bool())
                == Some(true);
            if ok {
                sync_tracks_from_engine(tracks, core);
                ui.set_last_action(format!("{alias}: LOADED IN AURA SANDBOX").into());
            } else {
                ui.set_last_action(format!("{alias}: LOAD FAILED · {result}").into());
            }
            true
        }
        "OPENUTAU IMPORT" | "IMPORT OPENUTAU" => {
            let Some(source) = rfd::FileDialog::new()
                .add_filter("OpenUtau score", &["ustx", "ust"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU IMPORT CANCELLED".into());
                return true;
            };
            let Some(render) = rfd::FileDialog::new()
                .add_filter("Rendered vocal", &["wav", "aif", "aiff"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU IMPORT CANCELLED: RENDER IS REQUIRED".into());
                return true;
            };
            let diagnostic = core.openutau_import_diagnostic_json(
                &source.to_string_lossy(),
                &render.to_string_lossy(),
            );
            let parsed = serde_json::from_str::<serde_json::Value>(&diagnostic).ok();
            if parsed
                .as_ref()
                .and_then(|value| value.get("ok"))
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                ui.set_last_action(format!("OPENUTAU IMPORT REJECTED: {diagnostic}").into());
                return true;
            }
            let vocal_track_id = core.add_track(4);
            if vocal_track_id == 0
                || !core.add_region(vocal_track_id, &render.to_string_lossy(), 0.0)
            {
                ui.set_last_action("OPENUTAU IMPORT FAILED: COULD NOT CREATE VOCAL REGION".into());
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action(
                format!(
                    "OPENUTAU VOCAL IMPORTED: {} · SOURCE KEPT: {}",
                    render
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("render"),
                    source
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("score"),
                )
                .into(),
            );
            true
        }
        "OPENUTAU EDIT" | "OPENUTAU SCORE" => {
            let Some(source) = rfd::FileDialog::new()
                .add_filter("OpenUtau score", &["ustx", "ust"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU EDIT CANCELLED".into());
                return true;
            };
            ui.set_openutau_score_path(source.to_string_lossy().into_owned().into());
            set_openutau_note_preview(ui, &source);
            ui.set_openutau_status("OPENUTAU SCORE LOADED · READY TO EDIT".into());
            ui.set_bot_view(26);
            ui.set_mx_open(true);
            ui.set_pr_open(false);
            true
        }
        "OPENUTAU LAUNCH" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            ui.set_openutau_status(
                launch_openutau(std::path::Path::new(&source))
                    .map(|()| "OPENUTAU EDITOR LAUNCHED · EDIT THEN RENDER".to_owned())
                    .unwrap_or_else(|error| error)
                    .into(),
            );
            true
        }
        "OPENUTAU RENDER" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            ui.set_openutau_status(
                launch_openutau(std::path::Path::new(&source))
                    .map(|()| "OPENUTAU READY · EXPORT WAV/AIFF, THEN IMPORT".to_owned())
                    .unwrap_or_else(|error| error)
                    .into(),
            );
            true
        }
        "OPENUTAU APPLY TUNING" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            let source_path = std::path::Path::new(&source);
            let Some(stem) = source_path.file_stem().and_then(|v| v.to_str()) else {
                ui.set_openutau_status("TUNING FAILED · INVALID SCORE PATH".into());
                return true;
            };
            let output = source_path.with_file_name(format!("{stem}_aura_tuned.ustx"));
            let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/tune_openutau_chorus.py");
            let result = Command::new("python3")
                .arg(script)
                .arg(&source)
                .arg(&output)
                .arg(format!("{:.2}", ui.get_openutau_scoop() * 16.0))
                .arg(format!("{:.2}", ui.get_openutau_vibrato() * 18.0 + 4.0))
                .arg(format!("{:.2}", ui.get_openutau_vibrato() * 34.0 + 16.0))
                .arg(format!("{:.2}", ui.get_openutau_dynamics()))
                .arg(format!("{:.2}", ui.get_openutau_consonants()))
                .arg(format!(
                    "{:.2}",
                    220.0 - ui.get_openutau_vibrato_rate() * 160.0
                ))
                .output();
            if result.as_ref().is_ok_and(|value| value.status.success()) {
                let _ = core.set_openutau_tuning(
                    &source,
                    &output.to_string_lossy(),
                    ui.get_openutau_scoop(),
                    ui.get_openutau_vibrato(),
                    ui.get_openutau_dynamics(),
                    ui.get_openutau_consonants(),
                );
                ui.set_openutau_score_path(output.to_string_lossy().into_owned().into());
                set_openutau_note_preview(ui, &output);
                ui.set_openutau_status(
                    "AURA TUNING APPLIED · PITCH / VIBRATO / DYNAMICS READY".into(),
                );
            } else {
                ui.set_openutau_status("TUNING FAILED · CHECK PYTHON / USTX".into());
            }
            true
        }
        "OPENUTAU IMPORT RENDER" => {
            let source = ui.get_openutau_score_path().to_string();
            let render = ui.get_openutau_render_path().to_string();
            let diagnostic = core.openutau_import_diagnostic_json(&source, &render);
            let ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(|v| v.as_bool()))
                == Some(true);
            if !ok {
                ui.set_openutau_status(format!("IMPORT REJECTED · {diagnostic}").into());
                return true;
            }
            let vocal_track_id = core.add_track(4);
            if vocal_track_id == 0 || !core.add_region(vocal_track_id, &render, 0.0) {
                ui.set_openutau_status("IMPORT FAILED · VOCAL TRACK NOT CREATED".into());
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_openutau_status("VOCAL RENDER IMPORTED INTO AURA".into());
            ui.set_last_action("OPENUTAU VOCAL TRACK UPDATED".into());
            true
        }
        "ADD LIMITER" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "add limiter failed: no track").into(),
                );
                return true;
            };
            let track_id = track.id.max(0) as u32;
            if !core.add_plugin(track_id, 0) {
                ui.set_last_action(
                    ui_error_with_action(
                        UiErrorKind::Plugin,
                        "リミッタープラグインを読み込めません",
                        "プラグインを再スキャンするか、プラグイン設定を確認してください",
                    )
                    .into(),
                );
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action("ADDED: Aura Limiter".into());
            true
        }
        _ => false,
    }
}
