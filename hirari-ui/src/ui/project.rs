use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::orchestrator::{CoreOrchestrator, UIAction};
use crate::slint_ui::{
    begin_recovery_as_transactionally, choose_project_save_file, choose_video_file, display_path,
    fallback_template_tracks, hydrate_project_backup_models, hydrate_project_models,
    load_project_transactionally, load_project_transactionally_with_hydrator,
    restore_latest_project_backup_transactionally, store_project_path, sync_tracks_from_engine,
    ui_error_message, AppWindow, ProjectActions, UiErrorKind, Z_Marker, Z_Track,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};
use crate::ui::sync::replace_track;
use crate::ui::track_model::sync_midi_notes_from_core;

pub(crate) fn sync_markers_from_core(markers: &Rc<VecModel<Z_Marker>>, core: &HirariCore) {
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
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    markers: Rc<VecModel<Z_Marker>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    persisted_snapshot: Rc<Cell<Option<u64>>>,
    orchestrator: Rc<CoreOrchestrator>,
    operation_gate: OperationGate,
    project_save_queue: Rc<crate::ui::project_save_queue::ProjectSaveQueue>,
    moufu_publisher: Option<crate::moufu::MoufuPublisher>,
    session_recovery_path: Rc<RefCell<Option<std::path::PathBuf>>>,
) {
    let weak = ui.as_weak();
    let markers_for_load = markers.clone();
    ui.global::<ProjectActions>().on_load_project({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let last_saved_path = last_saved_path.clone();
        let persisted_snapshot = persisted_snapshot.clone();
        let operation_gate = operation_gate.clone();
        let moufu_publisher = moufu_publisher.clone();
        move |path| {
            let Some(lease) = operation_gate.try_enter(OperationKind::Load) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: LOAD IGNORED".into());
                }
                return;
            };
            let previous_path = last_saved_path.borrow().clone();
            let result = if path.trim().is_empty() {
                Err(crate::slint_ui::ProjectLoadTransactionError::LoadFailed)
            } else {
                load_project_transactionally(
                    path.as_str(),
                    previous_path.as_deref(),
                    &tracks,
                    &core,
                    |core| {
                        core.load_project(path.as_str())
                            && operation_gate.is_current(lease.generation)
                    },
                )
            };
            if let Some(ui) = weak.upgrade() {
                crate::ui::project_commands::detach_save_target_after_rollback_failure(
                    &ui,
                    &last_saved_path,
                    &result,
                );
            }
            let ok = result.is_ok();
            if ok {
                if let Some(ui) = weak.upgrade() {
                    crate::ui::project_commands::reset_project_scoped_ui(&ui);
                }
                sync_markers_from_core(&markers_for_load, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                }
                *last_saved_path.borrow_mut() = Some(path.to_string());
                let patterns_json = weak
                    .upgrade()
                    .map(|ui| ui.get_sequencer_patterns_json().to_string())
                    .unwrap_or_else(|| "[]".to_owned());
                persisted_snapshot.set(crate::ui::project_state::save_fingerprint(
                    &core,
                    &tracks,
                    &patterns_json,
                ));
                store_project_path(path.as_str());
                if let Some(ui) = weak.upgrade() {
                    ui.set_show_genesis(false);
                }
                if let Some(publisher) = &moufu_publisher {
                    crate::moufu::publish_layout(publisher, &core, &tracks, false);
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
                } else if let Err(error) = result {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("open failed ({error:?}): {}", display_path(&path)),
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Project, "project load request was superseded")
                        .into()
                });
            }
        }
    });
    ui.global::<ProjectActions>().on_save_project({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let operation_gate = operation_gate.clone();
        let project_save_queue = project_save_queue.clone();
        move |path| {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: SAVE IGNORED".into());
                }
                return;
            };
            let path = path.to_string();
            let patterns_json = weak
                .upgrade()
                .map(|ui| ui.get_sequencer_patterns_json().to_string())
                .unwrap_or_else(|| "[]".to_owned());
            let saved = !path.trim().is_empty()
                && crate::ui::project_state::save_fingerprint(&core, &tracks, &patterns_json)
                    .is_some_and(|fingerprint| {
                        project_save_queue
                            .enqueue(
                                &path,
                                &tracks,
                                &core,
                                &patterns_json,
                                fingerprint,
                                crate::ui::project_save_queue::SaveIntent::Manual {
                                    success_prefix: "SAVED".to_owned(),
                                    failure_label: "save failed".to_owned(),
                                },
                                lease,
                            )
                            .is_ok()
                    });
            if let Some(ui) = weak.upgrade() {
                if saved {
                    ui.set_project_save_status("Saving project snapshot…".into());
                    ui.set_last_action(format!("SAVING: {}", display_path(&path)).into());
                } else if path.trim().is_empty() {
                    ui.set_last_action("SAVE CANCELLED".into());
                } else {
                    ui.set_project_save_status("Save could not be queued".into());
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "project save could not be started")
                            .into(),
                    );
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_recover_unsaved_session({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let markers = markers.clone();
        let last_saved_path = last_saved_path.clone();
        let persisted_snapshot = persisted_snapshot.clone();
        let session_recovery_path = session_recovery_path.clone();
        let operation_gate = operation_gate.clone();
        move || {
            let Some(path) = session_recovery_path.borrow().clone() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("UNSAVED RECOVERY FILE IS UNAVAILABLE".into());
                }
                return;
            };
            let Some(_lease) = operation_gate.try_enter(OperationKind::Load) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: RECOVERY IGNORED".into());
                }
                return;
            };
            let path_text = path.to_string_lossy().into_owned();
            let previous_path = last_saved_path.borrow().clone();
            let result = load_project_transactionally(
                &path_text,
                previous_path.as_deref(),
                &tracks,
                &core,
                |core| core.load_project(&path_text),
            );
            if let Some(ui) = weak.upgrade() {
                crate::ui::project_commands::detach_save_target_after_rollback_failure(
                    &ui,
                    &last_saved_path,
                    &result,
                );
            }
            match result {
                Ok(()) => {
                    sync_markers_from_core(&markers, &core);
                    let patterns_json = weak
                        .upgrade()
                        .map(|ui| ui.get_sequencer_patterns_json().to_string())
                        .unwrap_or_else(|| "[]".to_owned());
                    persisted_snapshot.set(crate::ui::project_state::save_fingerprint(
                        &core,
                        &tracks,
                        &patterns_json,
                    ));
                    if let Some(ui) = weak.upgrade() {
                        crate::ui::project_commands::reset_project_scoped_ui(&ui);
                        ui.set_master_output_gain(core.master_gain());
                        ui.set_project_path("Recovered session · Save As to keep".into());
                        ui.set_project_save_status("Recovered · Unsaved".into());
                        ui.set_unsaved_recovery_available(false);
                        ui.set_show_genesis(false);
                        ui.set_last_action("UNSAVED SESSION RECOVERED · SAVE AS TO KEEP IT".into());
                    }
                }
                Err(error) => {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                &format!("unsaved session recovery failed ({error:?})"),
                            )
                            .into(),
                        );
                    }
                }
            }
        }
    });
    ui.global::<ProjectActions>().on_discard_unsaved_recovery({
        let weak = weak.clone();
        let session_recovery_path = session_recovery_path.clone();
        move || {
            let Some(path) = session_recovery_path.borrow().clone() else {
                return;
            };
            if crate::slint_ui::discard_session_recovery(&path) {
                *session_recovery_path.borrow_mut() = crate::slint_ui::new_session_recovery_path();
                if let Some(ui) = weak.upgrade() {
                    ui.set_unsaved_recovery_available(false);
                    ui.set_project_save_status("Unsaved project · Auto-recovery ready".into());
                    ui.set_last_action("UNSAVED RECOVERY DISCARDED".into());
                }
            } else if let Some(ui) = weak.upgrade() {
                ui.set_last_action("UNSAVED RECOVERY COULD NOT BE DISCARDED".into());
            }
        }
    });
    ui.global::<ProjectActions>().on_dismiss_unsaved_recovery({
        let weak = weak.clone();
        let session_recovery_path = session_recovery_path.clone();
        move || {
            *session_recovery_path.borrow_mut() = crate::slint_ui::new_session_recovery_path();
            if let Some(ui) = weak.upgrade() {
                ui.set_unsaved_recovery_available(false);
                ui.set_last_action("UNSAVED RECOVERY KEPT FOR LATER".into());
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
            let previous_path = last_saved_path.borrow().clone();
            let result = restore_latest_project_backup_transactionally(
                &path,
                previous_path.as_deref(),
                &tracks,
                &core,
            )
            .map(|_| ());
            if let Some(ui) = weak.upgrade() {
                crate::ui::project_commands::detach_save_target_after_rollback_failure(
                    &ui,
                    &last_saved_path,
                    &result,
                );
            }
            let models_ready = result.is_ok();
            if models_ready {
                sync_markers_from_core(&markers_for_latest, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                    crate::ui::project_commands::reset_project_scoped_ui(&ui);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if models_ready {
                    "RECOVERED LAST BACKUP".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("recovery failed ({:?})", result.unwrap_err()),
                    )
                    .into()
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
            let previous_path = last_saved_path.borrow().clone();
            let result = if generation >= 0 {
                load_project_transactionally_with_hydrator(
                    &path,
                    previous_path.as_deref(),
                    &tracks,
                    &core,
                    |core| core.restore_project_backup(&path, generation as u32),
                    |hydration_path, tracks, core| {
                        if hydration_path == path {
                            hydrate_project_backup_models(&path, generation as u32, tracks, core)
                        } else {
                            hydrate_project_models(hydration_path, tracks, core)
                        }
                    },
                )
            } else {
                Err(crate::slint_ui::ProjectLoadTransactionError::LoadFailed)
            };
            if let Some(ui) = weak.upgrade() {
                crate::ui::project_commands::detach_save_target_after_rollback_failure(
                    &ui,
                    &last_saved_path,
                    &result,
                );
            }
            let models_ready = result.is_ok();
            if models_ready {
                sync_markers_from_core(&markers_for_generation, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_master_output_gain(core.master_gain());
                    crate::ui::project_commands::reset_project_scoped_ui(&ui);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if models_ready {
                    format!("RECOVERED BACKUP .bak.{}", generation).into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("recovery failed ({:?})", result.unwrap_err()),
                    )
                    .into()
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
        let persisted_snapshot = persisted_snapshot.clone();
        let operation_gate = operation_gate.clone();
        let project_save_queue = project_save_queue.clone();
        move |generation| {
            let Some(lease) = operation_gate.try_enter(OperationKind::Recover) else {
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
            let Ok(generation) = u32::try_from(generation) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "invalid recovery generation")
                            .into(),
                    );
                }
                return;
            };
            let rollback = match begin_recovery_as_transactionally(
                &source, generation, &tracks, &core,
            ) {
                Ok(rollback) => rollback,
                Err(error) => {
                    if error == crate::slint_ui::ProjectLoadTransactionError::RollbackFailed {
                        if let Some(ui) = weak.upgrade() {
                            crate::ui::project_commands::detach_save_target_after_rollback_failure(
                                &ui,
                                &last_saved_path,
                                &Err(error),
                            );
                        }
                    }
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                &format!("recovery as failed ({error:?})"),
                            )
                            .into(),
                        );
                    }
                    return;
                }
            };
            sync_markers_from_core(&markers_for_generation_as, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_master_output_gain(core.master_gain());
                crate::ui::project_commands::reset_project_scoped_ui(&ui);
            }
            let patterns_json = weak
                .upgrade()
                .map(|ui| ui.get_sequencer_patterns_json().to_string())
                .unwrap_or_else(|| "[]".to_owned());
            let Some(fingerprint) =
                crate::ui::project_state::save_fingerprint(&core, &tracks, &patterns_json)
            else {
                // The recovered candidate is valid but cannot be fingerprinted;
                // retain it for Save As instead of risking a destructive rollback.
                drop(rollback);
                last_saved_path.borrow_mut().take();
                if let Some(ui) = weak.upgrade() {
                    ui.set_project_path("Recovered project · Save As to keep".into());
                    ui.set_project_save_status("Recovered project · Save manually".into());
                    ui.set_last_action("RECOVERED PROJECT NEEDS SAVE AS".into());
                }
                return;
            };
            let ui_weak = weak.clone();
            let core_after_save = core.clone();
            let tracks_after_save = tracks.clone();
            let last_saved_after_failure = last_saved_path.clone();
            let persisted_after_failure = persisted_snapshot.clone();
            let markers_after_failure = markers_for_generation_as.clone();
            let failure_rollback = Some(rollback);
            let after_completion: Box<dyn FnOnce(bool)> = Box::new(move |success| {
                let Some(rollback) = failure_rollback else {
                    return;
                };
                if success {
                    drop(rollback);
                    return;
                }
                let unchanged = crate::ui::project_state::save_fingerprint(
                    &core_after_save,
                    &tracks_after_save,
                    &ui_weak
                        .upgrade()
                        .map(|ui| ui.get_sequencer_patterns_json().to_string())
                        .unwrap_or_else(|| "[]".to_owned()),
                ) == Some(fingerprint);
                if unchanged {
                    let result = rollback.restore(&tracks_after_save, &core_after_save);
                    if let Some(ui) = ui_weak.upgrade() {
                        crate::ui::project_commands::detach_save_target_after_rollback_failure(
                            &ui,
                            &last_saved_after_failure,
                            &result,
                        );
                    }
                    if result.is_ok() {
                        sync_markers_from_core(&markers_after_failure, &core_after_save);
                        let current = crate::ui::project_state::save_fingerprint(
                            &core_after_save,
                            &tracks_after_save,
                            &ui_weak
                                .upgrade()
                                .map(|ui| ui.get_sequencer_patterns_json().to_string())
                                .unwrap_or_else(|| "[]".to_owned()),
                        );
                        persisted_after_failure.set(current);
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_master_output_gain(core_after_save.master_gain());
                            crate::ui::project_commands::reset_project_scoped_ui(&ui);
                            ui.set_project_save_status(
                                "Recovered As failed · Previous project restored".into(),
                            );
                            ui.set_last_action(
                                "RECOVER AS FAILED · PREVIOUS PROJECT RESTORED".into(),
                            );
                        }
                    } else if let Some(ui) = ui_weak.upgrade() {
                        ui.set_last_action("RECOVER AS FAILED · RECOVERY ROLLBACK REQUIRED".into());
                    }
                } else {
                    // Preserve edits made while the recovered snapshot was
                    // being published. Detach from the old project path so a
                    // later Save cannot overwrite the project we recovered from.
                    drop(rollback);
                    last_saved_after_failure.borrow_mut().take();
                    persisted_after_failure.set(Some(fingerprint));
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_project_path("Recovered project · Save As to keep".into());
                        ui.set_project_save_status("Recovered changes unsaved · Save As".into());
                        ui.set_last_action(
                            "RECOVER AS FAILED · EDITS KEPT · SAVE AS REQUIRED".into(),
                        );
                    }
                }
            });
            let queued = project_save_queue.enqueue_with_completion(
                &destination,
                &tracks,
                &core,
                &patterns_json,
                fingerprint,
                crate::ui::project_save_queue::SaveIntent::RecoveryAs {
                    success_prefix: format!("RECOVERED .bak.{generation} AS"),
                },
                lease,
                Some(after_completion),
            );
            if queued.is_ok() {
                persisted_snapshot.set(Some(fingerprint));
                if let Some(ui) = weak.upgrade() {
                    ui.set_project_save_status("Saving recovered project snapshot…".into());
                    ui.set_last_action("RECOVERING BACKUP · SAVING AS NEW PROJECT".into());
                }
            } else {
                log::warn!(
                    "Hirari Recover As save could not be queued: {}",
                    queued.unwrap_err()
                );
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
                    crate::ui::project_commands::reset_project_scoped_ui(&ui);
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
                    crate::ui::project_commands::reset_project_scoped_ui(&ui);
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
                if ui.get_bot_view() == 25 {
                    let selected = tracks.row_data(row);
                    crate::ui::midi::refresh_step_sequencer_region(&ui, selected.as_ref());
                }
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
        let last_saved_path = last_saved_path.clone();
        let operation_gate = operation_gate.clone();
        move || {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Load) else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PROJECT BUSY: RESET IGNORED".into());
                }
                return;
            };
            // Genesis reset starts a new unsaved session. Keep the previous
            // project's MIDI sidecar on disk for when that project is opened.
            last_saved_path.borrow_mut().take();
            core.new_project();
            crate::ui::track_model::reset_project_scoped_track_overlays(&tracks);
            sync_markers_from_core(&markers_for_genesis, &core);
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                crate::ui::project_commands::reset_project_scoped_ui(&ui);
                ui.set_master_output_gain(core.master_gain());
                ui.set_sel_idx(0);
                ui.set_last_action("PROJECT RESET: NEW SESSION READY".into());
            }
        }
    });
}
