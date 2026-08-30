use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{
    display_path, resolve_audio_path, sync_tracks_from_engine, ui_error_message, AppWindow,
    BrowserActions, DiagnosticsActions, MixerActions, UiErrorKind, Z_Track,
};
use crate::ui::sync::replace_track;

pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    ui.global::<DiagnosticsActions>().on_toggle_cloner({
        let tracks = tracks.clone();
        move |id| {
            for index in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(index) else {
                    continue;
                };
                if track.id == id {
                    track.saturate_active = !track.saturate_active;
                    replace_track(&tracks, index, track);
                    break;
                }
            }
        }
    });
    ui.global::<DiagnosticsActions>().on_execute_advice({
        let core = core.clone();
        move |title| core.execute_mixing_advice(title.into())
    });
    ui.global::<DiagnosticsActions>().on_toggle_dim({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("DIM MONITOR TOGGLED".into());
            }
        }
    });
    ui.global::<MixerActions>().on_eq_changed({
        let core = core.clone();
        move |id, low_band, low_cut, high_band, high_cut| {
            core.set_track_eq(id as u32, low_band, low_cut, high_band, high_cut);
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
            let imported = core.add_region_at_beat(track_id as u32, path.as_str(), beat as f64);
            if imported {
                let _ = core.register_preview_audio(path.as_str());
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if imported {
                    format!("IMPORTED: {}", display_path(&path)).into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("import failed: {}", display_path(&path)),
                    )
                    .into()
                });
            }
        }
    });
}
