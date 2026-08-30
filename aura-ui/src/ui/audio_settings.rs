use aura_core_bridge::AuraCore;
use slint::ComponentHandle;
use std::rc::Rc;

use crate::slint_ui::{ui_error_with_action, AppWindow, AudioSettingsActions, UiErrorKind};

pub fn install(ui: &AppWindow, core: Rc<AuraCore>) {
    let weak = ui.as_weak();
    ui.global::<AudioSettingsActions>().on_reconnect({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let diagnostic = core.try_reconnect_audio_device_diagnostic_json();
            let reconnect_ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(|ok| ok.as_bool()))
                .unwrap_or(false);
            if let Some(ui) = weak.upgrade() {
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
            let valid_values =
                (8_000..=384_000).contains(&sample_rate) && (1..=16_384).contains(&buffer_size);
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
}
