use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::orchestrator::{CoreOrchestrator, UIAction};
use crate::slint_ui::{
    choose_project_save_file, choose_video_file, display_path, fallback_template_tracks,
    hydrate_project_models, save_ui_midi_notes, store_project_path, sync_tracks_from_engine,
    ui_error_message, AppWindow, ProjectActions, UiErrorKind, Z_Marker, Z_Track,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};
use crate::ui::sync::replace_track;
use crate::ui::track_model::sync_midi_notes_from_core;

fn reset_project_scoped_ui(ui: &AppWindow) {
    // Plugin and routing widgets are keyed by the previous project graph.
    // Clear them before the next polling tick so a successful load cannot
    // briefly display stale sidechain/PDC state from the old project.
    ui.set_sidechain_source_id(-1);
    ui.set_sidechain_source_name("".into());
    ui.set_sidechain_result(false);
    ui.set_sidechain_tap_point(0);
    ui.set_plugin_pdc_status("CALCULATING".into());
    ui.set_plugin_pdc_compensation_ms(0.0);
    ui.set_sel_idx(0);
    ui.set_sel_cid(-1);
    ui.set_selected_clip_reversed(false);
    ui.set_selected_clip_trim_start(0.0);
    ui.set_selected_clip_trim_end(1.0);
    ui.set_selected_clip_warp_ratio(1.0);
    ui.set_selected_clip_pitch_semitones(0.0);
    ui.set_selected_clip_loop_count(1);
}

fn sync_markers_from_core(markers: &Rc<VecModel<Z_Marker>>, core: &AuraCore) {
    #[derive(serde::Deserialize)]
    struct Marker {
        id: u32,
        label: String,
        beat: f64,
        color: String,
    }
    let parsed = serde_json::from_str::<Vec<Marker>>(&core.markers_json()).unwrap_or_default();
    let rows = parsed
        .into_iter()
        .map(|marker| {
            let rgb =
                u32::from_str_radix(marker.color.trim_start_matches('#'), 16).unwrap_or(0x6472a8);
            Z_Marker {
                id: marker.id as i32,
                label: marker.label.into(),
                beat: marker.beat as f32,
                color: slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
            }
        })
        .collect::<Vec<_>>();
    markers.set_vec(rows);
}

pub(crate) fn install(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    tracks: Rc<VecModel<Z_Track>>,
    markers: Rc<VecModel<Z_Marker>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    orchestrator: Rc<CoreOrchestrator>,
    operation_gate: OperationGate,
) {
    let weak = ui.as_weak();
    let markers_for_load = markers.clone();
    ui.global::<ProjectActions>().on_load_project({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move |path| {
            let Some(lease) = operation_gate.try_enter(OperationKind::Load) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: LOAD IGNORED".into());
                }
                return;
            };
            let mut ok = !path.trim().is_empty() && core.load_project(path.as_str());
            if ok && operation_gate.is_current(lease.generation) {
                if let Some(ui) = weak.upgrade() {
                    reset_project_scoped_ui(&ui);
                }
                ok = hydrate_project_models(path.as_str(), &tracks, &core);
                if ok {
                    sync_markers_from_core(&markers_for_load, &core);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_master_output_gain(core.master_gain());
                    }
                    *last_saved_path.borrow_mut() = Some(path.to_string());
                    store_project_path(path.as_str());
                }
            }
            let _request_generation = lease.generation;
            if let Some(ui) = weak.upgrade() {
                if ok {
                    ui.set_project_path(path.clone());
                }
                ui.set_last_action(if ok {
                    format!("OPENED: {}", display_path(&path)).into()
                } else if path.trim().is_empty() {
                    "OPEN CANCELLED".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("open failed: {}", display_path(&path)),
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_save_project({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move |path| {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: SAVE IGNORED".into());
                }
                return;
            };
            let saved = !path.trim().is_empty()
                && core.save_project(path.as_str())
                && save_ui_midi_notes(path.as_str(), &tracks);
            if saved && operation_gate.is_current(lease.generation) {
                *last_saved_path.borrow_mut() = Some(path.to_string());
                store_project_path(path.as_str());
            }
            let _request_generation = lease.generation;
            if let Some(ui) = weak.upgrade() {
                if saved {
                    ui.set_project_path(path.clone());
                    ui.set_auto_save_status("Saved · Auto-save Ready".into());
                }
                ui.set_last_action(if saved {
                    format!("SAVED: {}", display_path(&path)).into()
                } else if path.trim().is_empty() {
                    "SAVE CANCELLED".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("save failed: {}", display_path(&path)),
                    )
                    .into()
                });
            }
        }
    });
    let markers_for_latest = markers.clone();
    ui.global::<ProjectActions>().on_recover_latest({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move || {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Recover) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: RECOVERY IGNORED".into());
                }
                return;
            };
            let Some(path) = last_saved_path.borrow().clone() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "no saved project to recover")
                            .into(),
                    );
                }
                return;
            };
            let candidates: serde_json::Value =
                serde_json::from_str(&core.recovery_candidates_json(&path))
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
            let generation = candidates
                .as_array()
                .and_then(|items| items.first())
                .and_then(|item| item.get("generation"))
                .and_then(serde_json::Value::as_u64)
                .map(|value| value as u32);
            let ok =
                generation.is_some_and(|generation| core.restore_project_backup(&path, generation));
            let models_ready = ok && hydrate_project_models(&path, &tracks, &core);
            if ok && !models_ready {
                let _ = core.load_project(&path);
            }
            if models_ready {
                sync_markers_from_core(&markers_for_latest, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                    reset_project_scoped_ui(&ui);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if models_ready {
                    "RECOVERED LAST BACKUP".into()
                } else {
                    ui_error_message(UiErrorKind::Project, "no valid recovery backup found").into()
                });
            }
        }
    });
    let markers_for_generation = markers.clone();
    ui.global::<ProjectActions>().on_recover_generation({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move |generation| {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Recover) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: RECOVERY IGNORED".into());
                }
                return;
            };
            let Some(path) = last_saved_path.borrow().clone() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "no saved project to recover")
                            .into(),
                    );
                }
                return;
            };
            let ok = generation >= 0 && core.restore_project_backup(&path, generation as u32);
            let models_ready = ok && hydrate_project_models(&path, &tracks, &core);
            if ok && !models_ready {
                let _ = core.load_project(&path);
            }
            if models_ready {
                sync_markers_from_core(&markers_for_generation, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                    reset_project_scoped_ui(&ui);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if models_ready {
                    format!("RECOVERED BACKUP .bak.{}", generation).into()
                } else {
                    ui_error_message(UiErrorKind::Project, "selected backup is invalid").into()
                });
            }
        }
    });
    let markers_for_generation_as = markers.clone();
    ui.global::<ProjectActions>().on_recover_generation_as({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move |generation| {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Recover) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: RECOVERY IGNORED".into());
                }
                return;
            };
            let Some(source) = last_saved_path.borrow().clone() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "no saved project to recover")
                            .into(),
                    );
                }
                return;
            };
            let Some(destination) = choose_project_save_file() else {
                return;
            };
            let recovered =
                generation >= 0 && core.restore_project_backup(&source, generation as u32);
            let saved_as = recovered && core.save_project(&destination);
            // Restoring the source mutates the live engine before the new
            // destination is published. If the destination cannot be saved,
            // roll the engine back to the original project so the UI and the
            // current project path never describe different projects.
            if recovered && !saved_as {
                let _ = core.load_project(&source);
            }
            let mut restored = saved_as;
            if restored && hydrate_project_models(&destination, &tracks, &core) {
                sync_markers_from_core(&markers_for_generation_as, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                    reset_project_scoped_ui(&ui);
                }
                *last_saved_path.borrow_mut() = Some(destination.clone());
                store_project_path(&destination);
            } else if restored {
                restored = false;
                let _ = core.load_project(&source);
            }
            if let Some(ui) = weak.upgrade() {
                if restored {
                    ui.set_project_path(destination.clone().into());
                    ui.set_auto_save_status("Recovered As · Auto-save Ready".into());
                }
                ui.set_last_action(if restored {
                    format!(
                        "RECOVERED .bak.{} AS {}",
                        generation,
                        display_path(&destination)
                    )
                    .into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "selected backup could not be saved as a new project",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_load_video({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let Some(path) = choose_video_file() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("VIDEO LOAD CANCELLED".into());
                }
                return;
            };
            let ok = core.load_video(&path);
            if let Some(ui) = weak.upgrade() {
                ui.set_show_video(ok);
                ui.set_last_action(if ok {
                    format!("VIDEO LOADED: {}", display_path(&path)).into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("video load failed: {}", display_path(&path)),
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_undo({
        let weak = weak.clone();
        let orchestrator = orchestrator.clone();
        let tracks = tracks.clone();
        let markers = markers.clone();
        move || {
            let result = orchestrator.dispatch_result(UIAction::Undo);
            if result.is_ok() {
                sync_tracks_from_engine(&tracks, &orchestrator.core());
                sync_midi_notes_from_core(&tracks, &orchestrator.core());
                sync_markers_from_core(&markers, &orchestrator.core());
            }
            if let Some(ui) = weak.upgrade() {
                if result.is_ok() {
                    ui.set_master_output_gain(orchestrator.core().master_gain());
                    reset_project_scoped_ui(&ui);
                }
                ui.set_last_action(match result {
                    Ok(_) => "UNDO APPLIED".into(),
                    Err(error) => ui_error_message(UiErrorKind::Project, &error.to_string()).into(),
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_redo({
        let weak = weak.clone();
        let orchestrator = orchestrator.clone();
        let tracks = tracks.clone();
        let markers = markers.clone();
        move || {
            let result = orchestrator.dispatch_result(UIAction::Redo);
            if result.is_ok() {
                sync_tracks_from_engine(&tracks, &orchestrator.core());
                sync_midi_notes_from_core(&tracks, &orchestrator.core());
                sync_markers_from_core(&markers, &orchestrator.core());
            }
            if let Some(ui) = weak.upgrade() {
                if result.is_ok() {
                    ui.set_master_output_gain(orchestrator.core().master_gain());
                    reset_project_scoped_ui(&ui);
                }
                ui.set_last_action(match result {
                    Ok(_) => "REDO APPLIED".into(),
                    Err(error) => ui_error_message(UiErrorKind::Project, &error.to_string()).into(),
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_add_track({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |kind| {
            let type_id = if kind == "AUDIO" {
                0
            } else if kind == "FOLD" {
                1
            } else {
                2
            };
            core.add_track(type_id);
            sync_tracks_from_engine(&tracks, &core);
            if tracks.row_count() == 0 {
                tracks.set_vec(fallback_template_tracks(&[("Audio 1", 0)]));
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(format!("ADD TRACK: {}", kind).into());
            }
        }
    });
    ui.global::<ProjectActions>().on_delete_track({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            if let Some(pos) = (0..tracks.row_count())
                .find(|&i| tracks.row_data(i).is_some_and(|track| track.id == id))
            {
                let name = tracks
                    .row_data(pos)
                    .map(|track| track.name)
                    .unwrap_or_default();
                let ok = core.remove_track(id as u32);
                if ok {
                    sync_tracks_from_engine(&tracks, &core);
                }
                if let Some(ui) = weak.upgrade() {
                    if ok && ui.get_sidechain_source_id() == id {
                        ui.set_sidechain_source_id(-1);
                        ui.set_sidechain_source_name("".into());
                        ui.set_sidechain_result(false);
                    }
                    ui.set_last_action(if ok {
                        format!("DEL TRACK: {}", name).into()
                    } else {
                        "DELETE TRACK REJECTED".into()
                    });
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_select_track({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move |id| {
            let Some(row) = (0..tracks.row_count())
                .find(|&i| tracks.row_data(i).is_some_and(|track| track.id == id))
            else {
                return;
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_idx(row as i32);
                ui.set_last_action(format!("SELECT TRACK: {}", id).into());
            }
        }
    });
    ui.global::<ProjectActions>().on_toggle_arm({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id == id {
                    let armed = !track.armed;
                    if !core.set_track_armed(id as u32, armed) {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_last_action(
                                ui_error_message(
                                    UiErrorKind::AudioDevice,
                                    "record arm rejected by Core",
                                )
                                .into(),
                            );
                        }
                        return;
                    }
                    track.armed = armed;
                    replace_track(&tracks, row, track);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            format!("TRACK {} ARM: {}", id, if armed { "ON" } else { "OFF" })
                                .into(),
                        );
                    }
                    break;
                }
            }
        }
    });
    let markers_for_genesis = markers.clone();
    ui.global::<ProjectActions>().on_genesis_reset({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move || {
            core.new_project();
            sync_markers_from_core(&markers_for_genesis, &core);
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_master_output_gain(core.master_gain());
                ui.set_sel_idx(0);
                ui.set_last_action("PROJECT RESET: NEW SESSION READY".into());
            }
        }
    });
}
