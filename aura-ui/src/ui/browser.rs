use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, VecModel};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use crate::slint_ui::{
    choose_audio_library_directory, format_bytes, load_audio_library_path, resolve_audio_path,
    store_audio_library_path, ui_error_message, AppWindow, BrowserActions, UiErrorKind,
    Z_Sample_Entry,
};

pub fn install(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    samples: Rc<VecModel<Z_Sample_Entry>>,
    catalog: Rc<RefCell<Vec<Z_Sample_Entry>>>,
) {
    let weak = ui.as_weak();
    if let Some(path) = load_audio_library_path() {
        if core.scan_preview_audio(&path).is_ok() {
            if let Ok(value) =
                serde_json::from_str::<serde_json::Value>(&core.preview_audio_catalog_json())
            {
                let entries = catalog_entries_from_json(&value);
                *catalog.borrow_mut() = entries.clone();
                samples.set_vec(entries);
            }
        }
    }
    ui.global::<BrowserActions>().on_browser_search({
        let samples = samples.clone();
        let catalog = catalog.clone();
        move |query| {
            let query = query.trim().to_lowercase();
            let filtered: Vec<Z_Sample_Entry> = catalog
                .borrow()
                .iter()
                .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
                .cloned()
                .collect();
            samples.set_vec(filtered);
        }
    });
    ui.global::<BrowserActions>().on_browser_scan({
        let weak = weak.clone();
        let core = core.clone();
        let samples = samples.clone();
        let catalog = catalog.clone();
        move |requested| {
            let path = if requested.trim().is_empty() {
                choose_audio_library_directory()
            } else {
                Some(requested.to_string())
            };
            let result = path
                .as_deref()
                .ok_or_else(|| "audio library selection cancelled".to_owned())
                .and_then(|path| {
                    core.scan_preview_audio(path)
                        .map_err(|error| error.to_string())
                });
            if result.is_ok() {
                if let Some(path) = path.as_deref() {
                    store_audio_library_path(path);
                }
                if let Ok(value) =
                    serde_json::from_str::<serde_json::Value>(&core.preview_audio_catalog_json())
                {
                    let entries = catalog_entries_from_json(&value);
                    *catalog.borrow_mut() = entries.clone();
                    samples.set_vec(entries);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(match result {
                    Ok(count) => format!(
                        "LIBRARY SCANNED: {} assets · {}",
                        count,
                        path.unwrap_or_default()
                    )
                    .into(),
                    Err(error) => {
                        ui_error_message(UiErrorKind::Project, &format!("scan failed: {error}"))
                            .into()
                    }
                });
            }
        }
    });
    ui.global::<BrowserActions>().on_browser_preview({
        let weak = weak.clone();
        let core = core.clone();
        let samples = samples.clone();
        let catalog = catalog.clone();
        move |path| {
            let selected = resolve_audio_path(path.as_str());
            let selected_for_catalog = selected.clone();
            let result = selected
                .as_deref()
                .ok_or_else(|| "audio file selection cancelled".to_owned())
                .and_then(|path| {
                    Ok(core.preview_audio_file_async(path))
                });
            if result.is_ok() {
                if let Some(path) = selected_for_catalog {
                    let path_ref = std::path::Path::new(&path);
                    let name = path_ref
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("Imported Audio")
                        .to_owned();
                    let size = fs::metadata(path_ref)
                        .map(|meta| format_bytes(meta.len()))
                        .unwrap_or_else(|_| "--".to_owned());
                    let format = path_ref
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("AUDIO")
                        .to_ascii_uppercase();
                    let entry = Z_Sample_Entry {
                        name: name.into(),
                        path: path.clone().into(),
                        bpm: 0.0,
                        key: "-".into(),
                        fav: false,
                        sr: core.get_sample_rate().max(0.0) as i32,
                        size: size.into(),
                        fmt: format.into(),
                        wave: slint::ModelRc::default(),
                    };
                    let mut catalog = catalog.borrow_mut();
                    if let Some(existing) = catalog.iter_mut().find(|item| item.path == entry.path)
                    {
                        *existing = entry;
                    } else {
                        catalog.push(entry);
                    }
                    samples.set_vec(catalog.clone());
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(match result {
                    Ok(generation) => format!("PREVIEW QUEUED · generation {} · {}", generation, selected.unwrap_or_default()).into(),
                    Err(error) => {
                        ui_error_message(UiErrorKind::Project, &format!("preview failed: {error}"))
                            .into()
                    }
                });
            }
        }
    });
}

fn catalog_entries_from_json(value: &serde_json::Value) -> Vec<Z_Sample_Entry> {
    value
        .get("assets")
        .and_then(|assets| assets.as_array())
        .map(|assets| {
            assets
                .iter()
                .map(|asset| {
                    let path = asset
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let name = asset
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Audio");
                    let sr = asset
                        .get("sample_rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let size = asset
                        .get("size")
                        .and_then(|v| v.as_u64())
                        .map(format_bytes)
                        .unwrap_or_else(|| "--".to_owned());
                    Z_Sample_Entry {
                        name: name.into(),
                        path: path.into(),
                        bpm: 0.0,
                        key: "-".into(),
                        fav: false,
                        sr: sr as i32,
                        size: size.into(),
                        fmt: "WAV".into(),
                        wave: slint::ModelRc::default(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
