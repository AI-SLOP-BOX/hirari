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
