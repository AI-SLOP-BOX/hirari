use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{
    resolve_audio_path, ui_error_message, AppWindow, BrowserActions, DiagnosticsActions,
    MixerActions, UiErrorKind, Z_Track,
};
use crate::ui::sync::replace_track;

pub fn install(ui: &AppWindow, core: Rc<HirariCore>, tracks: Rc<VecModel<Z_Track>>) {
    // Reflect persisted Control Room state before the first frame is shown.
    ui.set_is_dim(core.control_room_dimmed());
    ui.set_is_talkback(core.control_room_talkback_enabled());
    ui.set_talkback_input_channel(core.control_room_talkback_input_channel() as i32);
    let monitor_enabled =
        serde_json::from_str::<serde_json::Value>(&core.control_room_monitor_snapshot_json())
            .ok()
            .and_then(|snapshot| snapshot["active_output_enabled"].as_bool())
            .unwrap_or(true);
    ui.set_is_monitor_enabled(monitor_enabled);
    ui.global::<DiagnosticsActions>().on_execute_advice({
        let core = core.clone();
        move |title| core.execute_mixing_advice(title.into())
    });
    ui.global::<DiagnosticsActions>().on_toggle_dim({
        let core = core.clone();
        let weak = ui.as_weak();
        move || {
            let enabled = core.control_room_dimmed();
            core.set_control_room_dim(!enabled);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("DIM MONITOR TOGGLED".into());
            }
        }
    });
    ui.global::<DiagnosticsActions>().on_toggle_talkback({
        let core = core.clone();
        let weak = ui.as_weak();
        move || {
            let enabled = core.control_room_talkback_enabled();
            core.set_control_room_talkback(!enabled, 1.0);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("TALKBACK TOGGLED".into());
            }
        }
    });
    ui.global::<DiagnosticsActions>().on_step_talkback_input({
        let core = core.clone();
        let weak = ui.as_weak();
        move |direction| {
            if !matches!(direction, -1 | 1) {
                return;
            }
            let available = core.get_audio_input_channel_count().min(32) as u32;
            let channel_count = if available == 0 { 32 } else { available };
            let current = core
                .control_room_talkback_input_channel()
                .min(channel_count - 1);
            let next = if direction > 0 {
                (current + 1) % channel_count
            } else {
                (current + channel_count - 1) % channel_count
            };
            if core.set_control_room_talkback_input_channel(next) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_talkback_input_channel(next as i32);
                    ui.set_last_action(format!("TALKBACK INPUT: {}", next + 1).into());
                }
            }
        }
    });
    ui.global::<DiagnosticsActions>().on_toggle_monitor_output({
        let core = core.clone();
        let weak = ui.as_weak();
        move || {
            let snapshot = serde_json::from_str::<serde_json::Value>(
                &core.control_room_monitor_snapshot_json(),
            )
            .ok();
            let enabled = snapshot
                .as_ref()
                .and_then(|value| value["active_output_enabled"].as_bool())
                .unwrap_or(true);
            let index = snapshot
                .as_ref()
                .and_then(|value| value["active_output_index"].as_u64())
                .unwrap_or(0) as u32;
            let accepted = core.set_control_room_speaker_enabled(index, !enabled);
            if let Some(ui) = weak.upgrade() {
                ui.set_is_monitor_enabled(if accepted { !enabled } else { enabled });
                ui.set_last_action("MONITOR OUTPUT TOGGLED".into());
            }
        }
    });
    ui.global::<MixerActions>().on_eq_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, low_band, low_cut, high_band, high_cut| {
            let accepted =
                serde_json::from_str::<serde_json::Value>(&core.set_track_eq_diagnostic_json(
                    id as u32, low_band, low_cut, high_band, high_cut,
                ))
                .ok()
                .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
            if !accepted {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id == id {
                    track.eq_low_band = low_band;
                    track.eq_low_cut = low_cut;
                    track.eq_high_band = high_band;
                    track.eq_high_cut = high_cut;
                    replace_track(&tracks, row, track);
                    break;
                }
            }
        }
    });
    ui.global::<BrowserActions>().on_import_sample({
        let core = core.clone();
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        move |requested_path, track_id, beat| {
            if tracks.row_count() == 0 {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "import failed: create a track first",
                        )
                        .into(),
                    );
                }
                return;
            }
            let Some(path) = resolve_audio_path(requested_path.as_str()) else {
                return;
            };
            if track_id < 0 {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                crate::ui::audio_import::import_audio_async(
                    &ui,
                    core.clone(),
                    tracks.clone(),
                    track_id as u32,
                    path,
                    beat as f64,
                    true,
                );
            }
        }
    });
}
