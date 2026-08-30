use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::scan_installed_plugin_count;
use crate::slint_ui::{
    ui_error_message, AppWindow, PluginActions, UiErrorKind, Z_Extension_Entry, Z_Track,
};
use crate::ui::track_model::sync_tracks_from_engine;

pub(crate) fn apply_plugin_parameter(
    core: &AuraCore,
    track_id: i32,
    plugin_index: i32,
    parameter_id: i32,
    value: f32,
) -> bool {
    if track_id < 0 || plugin_index < 0 || parameter_id < 0 || !value.is_finite() {
        return false;
    }
    core.set_plugin_parameter(
        track_id as u32,
        plugin_index as u32,
        parameter_id as u32,
        (value / 100.0).clamp(0.0, 1.0),
    )
}

pub(crate) fn filter_plugin_catalog(ui: &AppWindow, query: &str) {
    let needle = query.trim().to_ascii_lowercase();
    let favorites_only = matches!(
        needle.as_str(),
        "fav" | "favorite" | "favorites" | "お気に入り"
    );
    let catalog = ui.get_plugin_catalog();
    let mut filtered = Vec::with_capacity(catalog.row_count());
    for row in 0..catalog.row_count() {
        let Some(mut entry) = catalog.row_data(row) else {
            continue;
        };
        let haystack = format!(
            "{} {} {} {} {} {}",
            entry.name,
            entry.format,
            entry.capability,
            entry.path,
            entry.binary_hash,
            entry.binary_bytes
        )
        .to_ascii_lowercase();
        entry.visible = if favorites_only {
            entry.favorite
        } else {
            needle.is_empty() || haystack.contains(&needle)
        };
        filtered.push(entry);
    }
    ui.set_plugin_catalog(slint::ModelRc::new(VecModel::from(filtered)));
}

pub(crate) fn refresh_extension_catalog(ui: &AppWindow, core: &AuraCore) -> usize {
    let project_path = ui.get_project_path().to_string();
    let project = std::path::Path::new(&project_path);
    let root = if project.is_dir() {
        project.join(".aura").join("extensions")
    } else {
        project
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(".aura")
            .join("extensions")
    };
    let raw = core.extension_catalog_json(root.to_string_lossy().as_ref());
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        ui.set_extension_catalog_status("EXTENSIONS INVALID RESPONSE".into());
        ui.set_extension_catalog(slint::ModelRc::new(VecModel::from(
            Vec::<Z_Extension_Entry>::new(),
        )));
        return 0;
    };
    let mut entries = Vec::new();
    if let Some(commands) = value.get("commands").and_then(serde_json::Value::as_array) {
        for command in commands {
            let extension_id = command
                .get("extension_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let command_id = command
                .get("command_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let permissions = command
                .get("permissions")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            entries.push(Z_Extension_Entry {
                extension_id: extension_id.into(),
                command_id: command_id.into(),
                title: command
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(command_id)
                    .into(),
                kind: command
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("read_only")
                    .into(),
                execution: command
                    .get("execution")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("sandboxed")
                    .into(),
                permissions: permissions.into(),
                enabled: command
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                visible: true,
            });
        }
    }
    let errors = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    ui.set_extension_catalog(slint::ModelRc::new(VecModel::from(entries)));
    ui.set_extension_catalog_status(
        if errors == 0 {
            format!("{} COMMANDS READY", ui.get_extension_catalog().row_count())
        } else {
            format!(
                "{} COMMANDS · {} DISCOVERY ISSUE(S)",
                ui.get_extension_catalog().row_count(),
                errors
            )
        }
        .into(),
    );
    ui.get_extension_catalog().row_count()
}

fn extension_root_for_project(ui: &AppWindow) -> std::path::PathBuf {
    let project_path = ui.get_project_path().to_string();
    let project = std::path::Path::new(&project_path);
    if project.is_dir() {
        project.join(".aura").join("extensions")
    } else {
        project
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(".aura")
            .join("extensions")
    }
}

pub(crate) fn invoke_extension_from_palette(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
) -> bool {
    let Some(arguments) = command.strip_prefix("EXTENSION ") else {
        return false;
    };
    let mut parts = arguments.trim().splitn(2, char::is_whitespace);
    let Some(qualified) = parts.next().filter(|value| !value.is_empty()) else {
        ui.set_last_action("EXTENSION FAILED · USE EXTENSION ID.COMMAND {JSON}".into());
        return true;
    };
    let Some((extension_id, command_id)) = qualified.split_once('.') else {
        ui.set_last_action("EXTENSION FAILED · COMMAND MUST BE ID.COMMAND".into());
        return true;
    };
    let payload = parts.next().unwrap_or("{}").trim();
    let raw = core.invoke_extension_json(
        extension_root_for_project(ui).to_string_lossy().as_ref(),
        extension_id,
        command_id,
        payload,
        5_000,
    );
    let value = serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_default();
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        ui.set_last_action(format!("EXTENSION {} · COMPLETED", qualified).into());
    } else {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("extension_failed");
        ui.set_last_action(format!("EXTENSION {} · FAILED · {}", qualified, code).into());
    }
    true
}

fn run_extension_from_ui(
    extension_id: &str,
    command_id: &str,
    payload: &str,
    core: &AuraCore,
    ui: &AppWindow,
) {
    let raw = core.invoke_extension_json(
        extension_root_for_project(ui).to_string_lossy().as_ref(),
        extension_id,
        command_id,
        payload,
        5_000,
    );
    let value = serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_default();
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        ui.set_last_action(format!("EXTENSION {}.{} · COMPLETED", extension_id, command_id).into());
    } else {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("extension_failed");
        ui.set_last_action(
            format!(
                "EXTENSION {}.{} · FAILED · {}",
                extension_id, command_id, code
            )
            .into(),
        );
    }
}

fn toggle_extension_from_ui(extension_id: &str, enabled: bool, core: &AuraCore, ui: &AppWindow) {
    let raw = core.set_extension_enabled_json(
        extension_root_for_project(ui).to_string_lossy().as_ref(),
        extension_id,
        enabled,
    );
    let value = serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_default();
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        let count = refresh_extension_catalog(ui, core);
        ui.set_last_action(
            format!(
                "EXTENSION {} · {} · {} COMMANDS",
                extension_id,
                if enabled { "ENABLED" } else { "DISABLED" },
                count
            )
            .into(),
        );
    } else {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("extension_activation_failed");
        ui.set_last_action(format!("EXTENSION {} · FAILED · {}", extension_id, code).into());
    }
}

fn install_extension_from_ui(core: &AuraCore, ui: &AppWindow) {
    let Some(source) = rfd::FileDialog::new()
        .set_title("Choose Aura extension folder")
        .pick_folder()
    else {
        return;
    };
    let root = extension_root_for_project(ui);
    match aura_core_bridge::extensions::install_from_directory(&source, &root) {
        Ok(id) => {
            let count = refresh_extension_catalog(ui, core);
            ui.set_last_action(format!("EXTENSION {} · INSTALLED · {} COMMANDS", id, count).into());
        }
        Err(error) => ui.set_last_action(format!("EXTENSION INSTALL FAILED · {}", error).into()),
    }
}

pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    ui.global::<PluginActions>().on_catalog_query_changed({
        let weak = weak.clone();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                filter_plugin_catalog(&ui, query.as_str());
            }
        }
    });
    ui.global::<PluginActions>().on_refresh_extensions({
        let weak = ui.as_weak();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let count = refresh_extension_catalog(&ui, &core);
                ui.set_bot_view(16);
                ui.set_mx_open(true);
                ui.set_last_action(format!("EXTENSIONS: {} COMMANDS REGISTERED", count).into());
            }
        }
    });
    ui.global::<PluginActions>().on_install_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                install_extension_from_ui(&core, &ui);
            }
        }
    });
    ui.global::<PluginActions>().on_run_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move |extension_id, command_id, payload| {
            if let Some(ui) = weak.upgrade() {
                run_extension_from_ui(
                    extension_id.as_str(),
                    command_id.as_str(),
                    payload.as_str(),
                    &core,
                    &ui,
                );
            }
        }
    });
    ui.global::<PluginActions>().on_toggle_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move |extension_id, enabled| {
            if let Some(ui) = weak.upgrade() {
                toggle_extension_from_ui(extension_id.as_str(), enabled, &core, &ui);
            }
        }
    });
    ui.global::<PluginActions>().on_plugin_param_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, parameter_id, value| {
            let applied =
                apply_plugin_parameter(&core, track_id, plugin_index, parameter_id, value);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!(
                        "PLUGIN PARAM: TRACK {} FX {} P{}",
                        track_id, plugin_index, parameter_id
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "parameter rejected by host").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_apply_dynamics_suggestion({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index| {
            if track_id < 0 || plugin_index < 0 {
                return;
            }
            let result = core.apply_dynamics_suggestion_diagnostic_json(
                track_id as u32,
                plugin_index as u32,
                Vec::new(),
            );
            if let Some(ui) = weak.upgrade() {
                let ok = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false);
                ui.set_last_action(if ok {
                    "DYNAMICS ASSISTANT · DEFAULT GLUE APPLIED · UNDO READY".into()
                } else {
                    format!("DYNAMICS ASSISTANT FAILED · {}", result).into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_filter({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |row, name, value| {
            if row < 0 || !value.is_finite() {
                return;
            }
            let Some(track) = tracks.row_data(row as usize) else {
                return;
            };
            let Some(parameter_text) = name.strip_prefix("PARAM_") else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("FILTER PARAMETER REJECTED".into());
                }
                return;
            };
            let Ok(parameter) = parameter_text.parse::<u32>() else {
                return;
            };
            if parameter == 0 {
                return;
            }
            let applied = core.set_plugin_parameter(
                track.id.max(0) as u32,
                0,
                parameter - 1,
                value.clamp(0.0, 1.0),
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!("FILTER PARAM {}: APPLIED", parameter).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "filter parameter rejected by host")
                        .into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_macro_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |macro_index, value| {
            if macro_index < 0 || !value.is_finite() {
                return;
            }
            let normalized = value.clamp(0.0, 1.0);
            core.set_macro_value(macro_index as u32, normalized);
            if let Some(ui) = weak.upgrade() {
                let mut values: Vec<f32> = ui.get_track_macros().iter().collect();
                let index = macro_index as usize;
                if index >= values.len() {
                    return;
                }
                values[index] = normalized;
                ui.set_track_macros(slint::ModelRc::new(VecModel::from(values)));
                ui.set_last_action(format!("MACRO {}: {:.3}", macro_index, normalized).into());
            }
        }
    });
    ui.global::<PluginActions>().on_set_synth_engine({
        let weak = weak.clone();
        let core = core.clone();
        move |engine| {
            if !(0..=2).contains(&engine) {
                return;
            }
            core.set_preview_synth_engine(engine as u32);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!(
                        "SYNTH ENGINE: {}",
                        ["SYNTH", "WAVETABLE", "SAMPLER"][engine as usize]
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_set_route({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, enabled| {
            if source_id < 0 || dest_id < 0 || source_id == dest_id {
                return;
            }
            let ok = core.set_route(source_id as u32, dest_id as u32, enabled);
            if let Some(ui) = weak.upgrade() {
                ui.set_sidechain_result(ok);
                ui.set_last_action(if ok {
                    format!(
                        "ROUTE: {} -> {} {}",
                        source_id,
                        dest_id,
                        if enabled { "CONNECTED" } else { "DISCONNECTED" }
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "route rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_route_gain({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, gain, enabled| {
            if source_id < 0 || dest_id < 0 || source_id == dest_id || !gain.is_finite() {
                return;
            }
            let ok = core.set_route_gain(
                source_id as u32,
                dest_id as u32,
                gain.clamp(0.0, 2.0),
                enabled,
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("ROUTE GAIN: {} -> {} = {:.2}", source_id, dest_id, gain).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "route gain rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_save_preset({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, path| {
            let ok = track_id >= 0
                && plugin_index >= 0
                && !path.trim().is_empty()
                && core.save_plugin_preset(track_id as u32, plugin_index as u32, path.as_str());
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("PLUGIN PRESET SAVED: {}", path).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "preset save rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_load_preset({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, path| {
            let ok = track_id >= 0
                && plugin_index >= 0
                && !path.trim().is_empty()
                && core.load_plugin_preset(track_id as u32, plugin_index as u32, path.as_str());
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("PLUGIN PRESET LOADED: {}", path).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "preset load rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_move_plugin({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |plugin_index, delta| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let selected =
                crate::slint_ui::clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "move failed: no track selected").into(),
                );
                return;
            };
            if plugin_index < 0 || !(-1..=1).contains(&delta) {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "move failed: invalid plugin index")
                        .into(),
                );
                return;
            }
            let destination = plugin_index + delta;
            let ok = destination >= 0
                && core.move_plugin(
                    track.id.max(0) as u32,
                    plugin_index as u32,
                    destination as u32,
                );
            if ok {
                sync_tracks_from_engine(&tracks, &core);
                ui.set_fx_active_id(destination);
                ui.set_last_action(
                    format!("PLUGIN MOVED {} → {}", plugin_index, destination).into(),
                );
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "plugin reorder rejected by host").into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_browse_preset({
        let weak = weak.clone();
        move |for_save| {
            let dialog = rfd::FileDialog::new()
                .add_filter("Aura Plugin Preset", &["aupreset"])
                .set_title(if for_save {
                    "Save Plugin Preset"
                } else {
                    "Load Plugin Preset"
                });
            let path = if for_save {
                dialog.set_file_name("PluginPreset.aupreset").save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(path) = path {
                if let Some(ui) = weak.upgrade() {
                    ui.set_plugin_preset_path(path.to_string_lossy().into_owned().into());
                    ui.set_last_action(format!("PLUGIN PRESET PATH: {}", path.display()).into());
                }
            }
        }
    });
    ui.global::<PluginActions>().on_rescan_plugins({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let (discovered, rejected) = scan_installed_plugin_count();
            if let Some(ui) = weak.upgrade() {
                let catalog = crate::ui::plugin_commands::refresh_plugin_catalog_for_ui(&core, &ui);
                ui.set_last_action(
                    format!(
                        "PLUGIN SCAN COMPLETE: {} AVAILABLE · {} DISCOVERED · {} REJECTED",
                        catalog, discovered, rejected
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_insert_plugin({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |plugin_id| {
            if let Some(ui) = weak.upgrade() {
                crate::ui::plugin_commands::insert_catalog_plugin(&plugin_id, &core, &ui, &tracks);
            }
        }
    });
    ui.global::<PluginActions>().on_set_sidechain({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, plugin_index, tap_point, enabled| {
            let ok = source_id >= 0
                && dest_id >= 0
                && plugin_index >= 0
                && tap_point >= 0
                && core.set_sidechain_link(
                    source_id as u32,
                    dest_id as u32,
                    plugin_index as u32,
                    tap_point as u32,
                    enabled,
                );
            if let Some(ui) = weak.upgrade() {
                if ok {
                    ui.set_sidechain_result(enabled);
                }
                ui.set_last_action(if ok {
                    format!(
                        "SIDECHAIN {}: {} -> {} FX {}",
                        if enabled { "CONNECTED" } else { "DISCONNECTED" },
                        source_id,
                        dest_id,
                        plugin_index
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "sidechain rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_sidechain_tap_point({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, plugin_index, tap_point| {
            let valid = source_id >= 0
                && dest_id >= 0
                && plugin_index >= 0
                && tap_point >= 0
                && tap_point <= 2;
            let ok = valid
                && core.set_sidechain_link(
                    source_id as u32,
                    dest_id as u32,
                    plugin_index as u32,
                    tap_point as u32,
                    true,
                );
            if let Some(ui) = weak.upgrade() {
                if ok {
                    ui.set_sidechain_tap_point(tap_point);
                    ui.set_sidechain_result(true);
                    ui.set_last_action(format!("SIDECHAIN TAP UPDATED: {}", tap_point).into());
                } else if valid {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Plugin,
                            "sidechain tap update rejected by Core",
                        )
                        .into(),
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::apply_plugin_parameter;
    use aura_core_bridge::AuraCore;

    #[test]
    fn plugin_parameter_binding_validates_and_normalizes_before_core_apply() {
        let core = AuraCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.add_plugin(track_id, 0));

        assert!(!apply_plugin_parameter(&core, -1, 0, 0, 50.0));
        assert!(!apply_plugin_parameter(
            &core,
            track_id as i32,
            0,
            0,
            f32::NAN
        ));
        assert!(apply_plugin_parameter(&core, track_id as i32, 0, 0, 150.0));
        assert!(apply_plugin_parameter(&core, track_id as i32, 0, 0, -20.0));
    }
}
