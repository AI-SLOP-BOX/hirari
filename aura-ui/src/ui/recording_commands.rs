use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};

use crate::slint_ui::{AppWindow, UiErrorKind, Z_Track};

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &VecModel<Z_Track>,
) -> bool {
    if command.trim().to_ascii_uppercase() != "RECOVER RECORDING" {
        return false;
    }
    let Some(track) = (0..tracks.row_count()).find_map(|row| tracks.row_data(row)) else {
        ui.set_last_action("RECORDING RECOVERY FAILED: NO TRACK".into());
        return true;
    };
    let candidates: serde_json::Value =
        serde_json::from_str(&core.recording_recovery_candidates_json())
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    let Some(path) = candidates
        .as_array()
        .and_then(|items| items.first())
        .and_then(serde_json::Value::as_str)
    else {
        ui.set_last_action("RECORDING RECOVERY: NO CANDIDATE".into());
        return true;
    };
    let result = core.recover_recording_spool_to_track(path, track.id.max(0) as u32);
    ui.set_last_action(match result {
        Ok(frames) => format!("RECOVERED RECORDING: {} FRAMES", frames).into(),
        Err(error) => {
            crate::slint_ui::ui_error_message(UiErrorKind::AudioDevice, &error.to_string()).into()
        }
    });
    true
}
