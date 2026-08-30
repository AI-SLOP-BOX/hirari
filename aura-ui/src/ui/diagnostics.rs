use aura_core_bridge::AuraCore;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::slint_ui::{ui_error_message, AppWindow, UiErrorKind};

/// Handles diagnostic transport commands from the command palette.
/// Returns true when the command was consumed.
pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    started_ms: &AtomicU64,
    baseline_callbacks: &AtomicU64,
) -> bool {
    match command {
        "RECONNECT_AUDIO" => {
            core.try_reconnect_audio_device();
            ui.set_last_action("AUDIO RECONNECT REQUESTED".into());
            true
        }
        "TEST TONE" => {
            if !core.is_audio_device_ready() {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::AudioDevice,
                        "テストトーンを開始できません。オーディオデバイスが準備できていません",
                    )
                    .into(),
                );
                return true;
            }
            core.set_test_tone(true);
            if !core.is_playing() && !core.try_set_playing(true) {
                core.set_test_tone(false);
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::AudioDevice,
                        "テストトーンを開始できません。オーディオデバイスが準備できていません",
                    )
                    .into(),
                );
                return true;
            }
            baseline_callbacks.store(core.get_audio_callback_count(), Ordering::Release);
            let started = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as u64);
            started_ms.store(started, Ordering::Release);
            ui.set_last_action("TEST TONE: 440Hz".into());
            true
        }
        "STOP TEST TONE" => {
            core.set_test_tone(false);
            started_ms.store(0, Ordering::Release);
            ui.set_last_action("TEST TONE: STOPPED".into());
            true
        }
        _ => false,
    }
}
