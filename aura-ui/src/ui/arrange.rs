use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{
    sync_tracks_from_engine, AppWindow, ProjectActions, SpatialActions, Z_Track,
};
use crate::ui::sync::replace_track;

/// Track-level arrangement state. Region editing callbacks remain separate;
/// this module owns folder state, duplication, naming and spatial placement.
pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    ui.global::<ProjectActions>().on_toggle_expand({
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.expanded = !track.expanded;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_toggle_folder({
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id && track.is_folder {
                    track.folded = !track.folded;
                    track.expanded = !track.folded;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_duplicate_track({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |id| {
            let new_id = core.duplicate_track(id as u32);
            if new_id != 0 || id == 0 {
                sync_tracks_from_engine(&tracks, &core);
                if new_id != 0 {
                    if let Some(ui) = weak.upgrade() {
                        for index in 0..tracks.row_count() {
                            if tracks
                                .row_data(index)
                                .is_some_and(|track| track.id == new_id as i32)
                            {
                                ui.set_sel_idx(index as i32);
                                break;
                            }
                        }
                        ui.set_last_action(format!("DUPLICATED TRACK {} → {}", id, new_id).into());
                    }
                }
            }
        }
    });
    ui.global::<SpatialActions>().on_pan_3d_moved({
        let tracks = tracks.clone();
        let core = core.clone();
        move |id, x, y, z| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    if core.set_spatial_position(id as u32, x, y, z) {
                        track.pan3d_x = x;
                        track.pan3d_y = y;
                        track.pan3d_z = z;
                        replace_track(&tracks, i, track);
                    }
                    break;
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_rename_track({
        let tracks = tracks.clone();
        let core = core.clone();
        move |id, name| {
            if name.trim().is_empty() {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    if !core.set_track_name(id as u32, name.as_str()) {
                        return;
                    }
                    track.name = name;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
}
