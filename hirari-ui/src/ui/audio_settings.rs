use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model};
use std::rc::Rc;

use crate::slint_ui::{ui_error_with_action, AppWindow, AudioSettingsActions, UiErrorKind};

fn matching_device_indices(catalog: &str, query: &str) -> Vec<i32> {
    let query = query.trim().to_lowercase();
    let Ok(devices) = serde_json::from_str::<serde_json::Value>(catalog) else {
        return Vec::new();
    };
    let Some(devices) = devices.as_array() else {
        return Vec::new();
    };
    devices
        .iter()
        .enumerate()
        .filter_map(|(index, device)| {
            let name = device.get("name")?.as_str()?;
            (query.is_empty() || name.to_lowercase().contains(&query))
                .then_some(index.min(i32::MAX as usize) as i32)
        })
        .collect()
}

fn update_track_input_endpoint(
    tracks: &slint::VecModel<crate::slint_ui::Z_Track>,
    device_uid: &str,
    device_name: &str,
) {
    for row in 0..tracks.row_count() {
        let Some(mut track) = tracks.row_data(row) else {
            continue;
        };
        if track.r#type == "Audio" || track.r#type == "Vocal" {
            if track.input_endpoint_uid.as_str() != device_uid
                || track.input_endpoint_name.as_str() != device_name
            {
                track.input_endpoint_uid = device_uid.into();
                track.input_endpoint_name = device_name.into();
                tracks.set_row_data(row, track);
                crate::ui::project_state::mark_ui_routing_changed();
            }
        }
    }
}

pub fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<slint::VecModel<crate::slint_ui::Z_Track>>,
) {
    let weak = ui.as_weak();
    ui.global::<AudioSettingsActions>().on_filter_devices({
        let weak = weak.clone();
        let core = core.clone();
        move |query| {
            let catalog = core.list_audio_devices_json();
            if let Some(ui) = weak.upgrade() {
                let indices = matching_device_indices(&catalog, query.as_str());
                ui.set_audio_device_query(query);
                ui.set_audio_device_catalog(catalog.into());
                ui.set_audio_device_indices(slint::ModelRc::new(slint::VecModel::from(indices)));
            }
        }
    });
    ui.global::<AudioSettingsActions>().on_reconnect({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move || {
            let diagnostic = core.try_reconnect_audio_device_diagnostic_json();
            let reconnect_ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(|ok| ok.as_bool()))
                .unwrap_or(false);
            if let Some(ui) = weak.upgrade() {
                if reconnect_ok {
                    update_track_input_endpoint(
                        &tracks,
                        &core.audio_input_device_uid(),
                        &core.audio_input_device_name(),
                    );
                }
                ui.set_audio_device_ready(reconnect_ok);
                ui.set_last_action(if reconnect_ok {
                    "AUDIO DEVICE RECONNECTED".into()
                } else {
                    ui_error_with_action(
                        UiErrorKind::AudioDevice,
                        "reconnect failed",
                        "Open Audio Settings and select an available device",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<AudioSettingsActions>().on_apply_config({
        let weak = weak.clone();
        let core = core.clone();
        move |sample_rate, buffer_size| {
            let busy = core.is_playing() || core.recording_preview_active();
            // Keep the UI preflight aligned with the public Core contract so
            // unsupported values are rejected before touching the device.
            let valid_values = matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000 | 192_000)
                && matches!(buffer_size, 32 | 64 | 128 | 256 | 512 | 1024 | 2048);
            let diagnostic = if !busy && valid_values {
                core.apply_audio_config_diagnostic_json(sample_rate as u32, buffer_size as u32)
            } else {
                "{}".to_owned()
            };
            let parsed = serde_json::from_str::<serde_json::Value>(&diagnostic).ok();
            let applied = parsed
                .as_ref()
                .and_then(|value| value.get("ok").and_then(|ok| ok.as_bool()))
                .unwrap_or(false);
            let error_code = parsed
                .as_ref()
                .and_then(|value| {
                    value
                        .get("driver")
                        .and_then(|driver| driver.get("error_code"))
                        .or_else(|| value.get("error_code"))
                })
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!("AUDIO CONFIG: {} Hz · {} samples", sample_rate, buffer_size).into()
                } else if busy {
                    "AUDIO CONFIG BLOCKED: STOP PLAYBACK OR RECORDING FIRST".into()
                } else {
                    let detail = if error_code != 0 {
                        format!("audio configuration rejected (OSStatus {})", error_code)
                    } else {
                        "invalid audio configuration".to_owned()
                    };
                    ui_error_with_action(
                        UiErrorKind::AudioDevice,
                        &detail,
                        "Stop playback and choose a supported sample rate or buffer size",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<AudioSettingsActions>()
        .on_select_default_device({
            let weak = weak.clone();
            let core = core.clone();
            let tracks = tracks.clone();
            move || {
                let catalog =
                    serde_json::from_str::<serde_json::Value>(&core.list_audio_devices_json()).ok();
                let device_id = catalog
                    .as_ref()
                    .and_then(|value| value.as_array())
                    .and_then(|devices| devices.first())
                    .and_then(|device| device.get("id"))
                    .and_then(|id| id.as_u64())
                    .map(|id| id.min(u32::MAX as u64) as u32);
                let sample_rate = match core.get_sample_rate().round() as u32 {
                    44_100 | 48_000 | 88_200 | 96_000 | 192_000 => {
                        core.get_sample_rate().round() as u32
                    }
                    _ => 48_000,
                };
                let buffer_size = match core.get_buffer_size() {
                    32 | 64 | 128 | 256 | 512 | 1024 | 2048 => core.get_buffer_size(),
                    _ => 256,
                };
                let selected = device_id
                    .is_some_and(|id| core.select_audio_device(id, sample_rate, buffer_size));
                if let Some(ui) = weak.upgrade() {
                    if selected {
                        update_track_input_endpoint(
                            &tracks,
                            &core.audio_input_device_uid(),
                            &core.audio_input_device_name(),
                        );
                    }
                    ui.set_last_action(if selected {
                        "AUDIO DEVICE SELECTED".into()
                    } else {
                        ui_error_with_action(
                            UiErrorKind::AudioDevice,
                            "device selection failed",
                            "Reconnect Audio and verify an input/output device is available",
                        )
                        .into()
                    });
                }
            }
        });
    ui.global::<AudioSettingsActions>().on_select_device({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |index| {
            // Resolve the click against the exact catalog currently shown in
            // the UI. Re-querying Core here could shift indexes if a device
            // was unplugged or reordered after the row was rendered.
            let catalog = weak.upgrade().and_then(|ui| {
                serde_json::from_str::<serde_json::Value>(ui.get_audio_device_catalog().as_str())
                    .ok()
            });
            let device_id = catalog
                .as_ref()
                .and_then(|value| value.as_array())
                .and_then(|devices| devices.get(index.max(0) as usize))
                .and_then(|device| device.get("id"))
                .and_then(|id| id.as_u64())
                .map(|id| id.min(u32::MAX as u64) as u32);
            let sample_rate = match core.get_sample_rate().round() as u32 {
                44_100 | 48_000 | 88_200 | 96_000 | 192_000 => {
                    core.get_sample_rate().round() as u32
                }
                _ => 48_000,
            };
            let buffer_size = match core.get_buffer_size() {
                32 | 64 | 128 | 256 | 512 | 1024 | 2048 => core.get_buffer_size(),
                _ => 256,
            };
            let selected =
                device_id.is_some_and(|id| core.select_audio_device(id, sample_rate, buffer_size));
            if let Some(ui) = weak.upgrade() {
                if selected {
                    update_track_input_endpoint(
                        &tracks,
                        &core.audio_input_device_uid(),
                        &core.audio_input_device_name(),
                    );
                }
                ui.set_last_action(if selected {
                    format!("AUDIO DEVICE SELECTED: {index}").into()
                } else {
                    ui_error_with_action(
                        UiErrorKind::AudioDevice,
                        "device selection failed",
                        "Reconnect Audio and verify the selected device supports input or output",
                    )
                    .into()
                });
            }
        }
    });
}
