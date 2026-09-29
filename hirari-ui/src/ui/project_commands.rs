use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::slint_ui::{
    choose_dawproject_export_file, choose_dawproject_import_file, choose_project_file,
    choose_project_save_file, display_path, load_project_transactionally, load_saved_project_path,
    restore_latest_project_backup_transactionally, store_project_path, ui_error_message, AppWindow,
    ProjectActions, UiErrorKind, Z_Track,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};
use crate::ui::track_model::sync_tracks_from_engine;

pub(crate) fn reset_project_scoped_ui(ui: &AppWindow) {
    // These controls refer to IDs in the active project graph. Reset them
    // after a successful graph change, before telemetry can display stale
    // routing or plugin state from the previous project.
    ui.set_sidechain_source_id(-1);
    ui.set_sidechain_source_name("".into());
    ui.set_sidechain_result(false);
    ui.set_sidechain_tap_point(0);
    ui.set_plugin_pdc_status("CALCULATING".into());
    ui.set_plugin_pdc_compensation_ms(0.0);
    ui.set_fx_active_id(-1);
    ui.set_sel_idx(0);
    ui.set_sel_cid(-1);
    ui.set_selected_clip_reversed(false);
    ui.set_selected_clip_trim_start(0.0);
    ui.set_selected_clip_trim_end(1.0);
    ui.set_selected_clip_warp_ratio(1.0);
    ui.set_selected_clip_pitch_semitones(0.0);
    ui.set_selected_clip_gain(1.0);
    ui.set_selected_clip_name("".into());
    ui.set_selected_clip_loop_count(1);
    ui.set_selected_clip_sync_group(0);
    ui.set_selected_clip_sync_group_text("0".into());
    ui.set_sequencer_region_track_id(-1);
    ui.set_sequencer_region_id(-1);
    ui.set_sequencer_region_name("".into());
    ui.set_sequencer_region_start_beat(0.0);
    ui.set_sequencer_region_length_beats(0.0);
    ui.set_sequencer_region_bars(0);
    ui.set_sequencer_step_states(slint::ModelRc::default());
    ui.set_sequencer_unmapped_note_count(0);
    ui.set_sequencer_step_velocities(slint::ModelRc::default());
    ui.set_sequencer_step_probabilities(slint::ModelRc::default());
    ui.set_sequencer_step_gates(slint::ModelRc::default());
    ui.set_project_len(128.0);
}

pub(crate) fn detach_save_target_after_rollback_failure(
    ui: &AppWindow,
    last_saved_path: &RefCell<Option<String>>,
    result: &Result<(), crate::slint_ui::ProjectLoadTransactionError>,
) {
    if matches!(
        result,
        Err(crate::slint_ui::ProjectLoadTransactionError::RollbackFailed)
    ) {
        // The live Core graph can no longer be proven to match the prior save
        // target. Force any subsequent save through Save As so the old project
        // on disk cannot be overwritten by a partially restored graph.
        last_saved_path.borrow_mut().take();
        ui.set_project_path("".into());
        ui.set_project_save_status("Recovery Required · Save As Only".into());
    }
}

fn paths_refer_to_same_file(left: &str, right: &str) -> bool {
    match (
        std::path::Path::new(left).canonicalize(),
        std::path::Path::new(right).canonicalize(),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => std::path::Path::new(left) == std::path::Path::new(right),
    }
}

pub(crate) fn handle_command(
    command: &str,
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
    persisted_snapshot: &Cell<Option<u64>>,
    operation_gate: &OperationGate,
    project_save_queue: &Rc<crate::ui::project_save_queue::ProjectSaveQueue>,
    dawproject_import_queue: &std::sync::Arc<
        crate::ui::dawproject_import_queue::DawProjectImportQueue,
    >,
) -> bool {
    // Explicit routing commands are useful for keyboard-driven and AI-assisted
    // workflows. They carry IDs in the command itself, so they do not depend
    // on whichever row happens to be selected in the UI.
    let route_parts: Vec<&str> = command.split_whitespace().collect();
    if route_parts.len() == 4 && route_parts[0] == "ROUTE" && matches!(route_parts[3], "ON" | "OFF")
    {
        let (Ok(source_id), Ok(dest_id)) =
            (route_parts[1].parse::<u32>(), route_parts[2].parse::<u32>())
        else {
            return false;
        };
        let enabled = route_parts[3] == "ON";
        let ok = core.set_route(source_id, dest_id, enabled);
        ui.set_last_action(if ok {
            format!("ROUTE {} -> {} {}", source_id, dest_id, route_parts[3]).into()
        } else {
            ui_error_message(UiErrorKind::Project, "route rejected by Core").into()
        });
        return true;
    }
    if route_parts.len() == 5 && route_parts[0] == "ROUTE" && matches!(route_parts[4], "ON" | "OFF")
    {
        let (Ok(source_id), Ok(dest_id), Ok(gain)) = (
            route_parts[1].parse::<u32>(),
            route_parts[2].parse::<u32>(),
            route_parts[3].parse::<f32>(),
        ) else {
            return false;
        };
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return false;
        }
        let enabled = route_parts[4] == "ON";
        let ok = core.set_route_gain(source_id, dest_id, gain, enabled);
        ui.set_last_action(if ok {
            format!(
                "ROUTE {} -> {} GAIN {:.2} {}",
                source_id, dest_id, gain, route_parts[4]
            )
            .into()
        } else {
            ui_error_message(UiErrorKind::Project, "route gain rejected by Core").into()
        });
        return true;
    }
    if route_parts.len() == 5
        && route_parts[0] == "FEEDBACK"
        && matches!(route_parts[4], "ON" | "OFF")
    {
        let (Ok(source_id), Ok(dest_id), Ok(gain)) = (
            route_parts[1].parse::<u32>(),
            route_parts[2].parse::<u32>(),
            route_parts[3].parse::<f32>(),
        ) else {
            return false;
        };
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return false;
        }
        let enabled = route_parts[4] == "ON";
        let ok = core.set_feedback_route(source_id, dest_id, gain, enabled);
        ui.set_last_action(if ok {
            format!(
                "FEEDBACK {} -> {} GAIN {:.2} {}",
                source_id, dest_id, gain, route_parts[4]
            )
            .into()
        } else {
            ui_error_message(UiErrorKind::Project, "feedback route rejected by Core").into()
        });
        return true;
    }
    // Header actions carry the track identity explicitly. This avoids a
    // selection-update race when a freeze button is clicked on a row that was
    // not selected before the click.
    if let Some(raw_id) = command
        .strip_prefix("FREEZE TRACK ")
        .or_else(|| command.strip_prefix("UNFREEZE TRACK "))
    {
        let Ok(track_id) = raw_id.trim().parse::<u32>() else {
            return false;
        };
        let freezing = command.starts_with("FREEZE TRACK ");
        let ok = if freezing {
            core.freeze_track_to_project_end(track_id)
        } else {
            core.unfreeze_track(track_id)
        };
        if ok {
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action(if freezing {
                "TRACK FROZEN · CPU LOAD REDUCED".into()
            } else {
                "TRACK UNFROZEN · LIVE PROCESSING RESTORED".into()
            });
        } else {
            ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Project,
                    if freezing {
                        "track freeze rejected"
                    } else {
                        "track unfreeze rejected"
                    },
                )
                .into(),
            );
        }
        return true;
    }
    match command {
        "IMPORT DAWPROJECT" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Load) else {
                ui.set_last_action("PROJECT BUSY: DAWPROJECT IMPORT IGNORED".into());
                return true;
            };
            let Some(source) = choose_dawproject_import_file() else {
                ui.set_last_action("DAWPROJECT IMPORT CANCELLED".into());
                return true;
            };
            let Some(destination) = choose_project_save_file() else {
                ui.set_last_action("DAWPROJECT IMPORT CANCELLED".into());
                return true;
            };
            if last_saved_path
                .borrow()
                .as_deref()
                .is_some_and(|current| paths_refer_to_same_file(current, &destination))
            {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "choose a new Hirari project path so the current project remains recoverable",
                    )
                    .into(),
                );
                return true;
            }
            match dawproject_import_queue.enqueue(
                &source,
                &destination,
                core.get_sample_rate(),
                lease,
            ) {
                Ok(()) => {
                    ui.set_project_save_status("Importing DAWproject…".into());
                    ui.set_last_action(format!("IMPORTING: {}", display_path(&source)).into());
                }
                Err(error) => ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("DAWproject import could not start: {error}"),
                    )
                    .into(),
                ),
            }
            true
        }
        "ARCHIVE PROJECT" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                ui.set_last_action("PROJECT BUSY: ARCHIVE IGNORED".into());
                return true;
            };
            let source_project = last_saved_path
                .borrow()
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::env::temp_dir())
                        .join("untitled.hirari")
                });
            let project_name = source_project
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Hirari Project")
                .to_owned();
            let Some(mut destination) = rfd::FileDialog::new()
                .set_title("Save a single-file Hirari project package")
                .set_file_name(format!("{project_name}.hirari-package"))
                .save_file()
            else {
                ui.set_last_action("PROJECT ARCHIVE CANCELLED".into());
                return true;
            };
            if destination.extension().and_then(|value| value.to_str()) != Some("hirari-package") {
                destination.set_extension("hirari-package");
            }
            if destination.exists() {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("archive already exists: {}", display_path(&destination)),
                    )
                    .into(),
                );
                return true;
            }
            let snapshot = crate::slint_ui::prepare_ui_project_save(
                &source_project.to_string_lossy(),
                tracks,
                core,
                &ui.get_sequencer_patterns_json(),
            );
            let Some(snapshot) = snapshot else {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "project snapshot could not be captured",
                    )
                    .into(),
                );
                return true;
            };
            let weak = ui.as_weak();
            let source_project_for_worker = source_project.clone();
            let destination_for_worker = destination.clone();
            let spawn = std::thread::Builder::new()
                .name("hirari-project-archive".to_owned())
                .spawn(move || {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let result = (|| -> anyhow::Result<_> {
                        let mut snapshot = snapshot;
                        if !crate::slint_ui::attach_step_sequencer_patterns(&mut snapshot) {
                            anyhow::bail!("step sequencer project state is malformed");
                        }
                        let mut document = snapshot.document;
                        HirariCore::finalize_project_document_snapshot_v2(
                            &source_project_for_worker.to_string_lossy(),
                            &mut document,
                        )?;
                        let project_json = serde_json::to_vec_pretty(&document)?;
                        hirari_core_bridge::project_archive::ProjectArchive::create_single_file(
                            &source_project_for_worker,
                            &project_json,
                            &document,
                            &destination_for_worker,
                        )
                    })();
                    drop(lease);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(manifest) => ui.set_last_action(
                                    format!(
                                        "PROJECT ARCHIVED: {} · {} MEDIA FILES · {:.1} MB · PLUG-INS REQUIRE INSTALLATION",
                                        display_path(&destination_for_worker),
                                        manifest.entries.len(),
                                        manifest.total_bytes as f64 / (1024.0 * 1024.0),
                                    )
                                    .into(),
                                ),
                                Err(error) => ui.set_last_action(
                                    ui_error_message(
                                        UiErrorKind::Project,
                                        &format!("project archive failed: {error:#}"),
                                    )
                                    .into(),
                                ),
                            }
                        }
                    });
                });
            match spawn {
                Ok(_) => ui.set_last_action("CREATING PROJECT PACKAGE…".into()),
                Err(error) => ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("could not start archive worker: {error}"),
                    )
                    .into(),
                ),
            }
            true
        }
        "RESTORE PROJECT PACKAGE" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Load) else {
                ui.set_last_action("PROJECT BUSY: PACKAGE RESTORE IGNORED".into());
                return true;
            };
            let Some(package) = rfd::FileDialog::new()
                .set_title("Choose a Hirari project package")
                .add_filter("Hirari Project Package", &["hirari-package"])
                .pick_file()
            else {
                ui.set_last_action("PACKAGE RESTORE CANCELLED".into());
                return true;
            };
            let Some(parent) = rfd::FileDialog::new()
                .set_title("Choose where to restore the project")
                .pick_folder()
            else {
                ui.set_last_action("PACKAGE RESTORE CANCELLED".into());
                return true;
            };
            let package_name = package
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Hirari Project");
            let project_name = package_name
                .strip_suffix(".hirari-package")
                .unwrap_or(package_name);
            let destination = parent.join(format!("{project_name} Restored"));
            if destination.starts_with(&package) {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "choose a restore location outside the package",
                    )
                    .into(),
                );
                return true;
            }
            if destination.exists() {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!(
                            "restore destination already exists: {}",
                            display_path(&destination)
                        ),
                    )
                    .into(),
                );
                return true;
            }
            let weak = ui.as_weak();
            let package_for_worker = package.clone();
            let destination_for_worker = destination.clone();
            let spawn = std::thread::Builder::new()
                .name("hirari-project-package-restore".to_owned())
                .spawn(move || {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let result =
                        hirari_core_bridge::project_archive::ProjectArchive::restore_single_file(
                            &package_for_worker,
                            &destination_for_worker,
                        );
                    drop(lease);
                    let project_path = destination_for_worker.join("project.json");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(restored) => {
                                    ui.set_last_action(
                                        format!(
                                            "PACKAGE RESTORED · {} MEDIA FILES · OPENING PROJECT…",
                                            restored.len().saturating_sub(1),
                                        )
                                        .into(),
                                    );
                                    ui.global::<ProjectActions>().invoke_load_project(
                                        project_path.to_string_lossy().into_owned().into(),
                                    );
                                }
                                Err(error) => ui.set_last_action(
                                    ui_error_message(
                                        UiErrorKind::Project,
                                        &format!("package restore failed: {error:#}"),
                                    )
                                    .into(),
                                ),
                            }
                        }
                    });
                });
            match spawn {
                Ok(_) => ui.set_last_action("VERIFYING AND RESTORING PROJECT PACKAGE…".into()),
                Err(error) => ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("could not start package restore: {error}"),
                    )
                    .into(),
                ),
            }
            true
        }
        "EXPORT DAWPROJECT" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                ui.set_last_action("PROJECT BUSY: EXPORT IGNORED".into());
                return true;
            };
            let current_path = last_saved_path.borrow().clone();
            let source_project = current_path
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::env::temp_dir())
                        .join("untitled.hirari")
                });
            let default_name = format!(
                "{}.dawproject",
                source_project
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("untitled")
            );
            let Some(destination) = choose_dawproject_export_file(&default_name) else {
                ui.set_last_action("DAWPROJECT EXPORT CANCELLED".into());
                return true;
            };
            let project_name = source_project
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Hirari Project");
            let document = match core.project_document_snapshot_v2(project_name, core.get_tempo()) {
                Ok(document) => document,
                Err(error) => {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            &format!("export snapshot failed: {error}"),
                        )
                        .into(),
                    );
                    return true;
                }
            };
            let weak = ui.as_weak();
            let destination_for_worker = destination.clone();
            let spawn = std::thread::Builder::new()
                .name("hirari-dawproject-export".to_owned())
                .spawn(move || {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let result = hirari_core_bridge::dawproject::export_dawproject_file(
                        &document,
                        &source_project,
                        std::path::Path::new(&destination_for_worker),
                    );
                    drop(lease);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(report) => {
                                    ui.set_last_action(
                                        format!(
                                            "DAWPROJECT EXPORTED: {} · {} tracks · {} clips · {} MIDI notes · {} embedded WAV files · {} plugins omitted",
                                            display_path(&destination_for_worker),
                                            report.track_count,
                                            report.audio_clip_count,
                                            report.midi_note_count,
                                            report.embedded_media_count,
                                            report.unsupported_plugin_count,
                                        )
                                        .into(),
                                    );
                                }
                                Err(error) => ui.set_last_action(
                                    ui_error_message(
                                        UiErrorKind::Project,
                                        &format!("DAWproject export failed: {error:#}"),
                                    )
                                    .into(),
                                ),
                            }
                        }
                    });
                });
            match spawn {
                Ok(_) => ui.set_last_action("EXPORTING DAWPROJECT SNAPSHOT…".into()),
                Err(error) => {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            &format!("could not start export worker: {error}"),
                        )
                        .into(),
                    );
                }
            }
            true
        }
        "OPEN PROJECT" => {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Load) else {
                ui.set_last_action("PROJECT BUSY: OPEN IGNORED".into());
                return true;
            };
            let Some(path) = choose_project_file() else {
                ui.set_last_action("OPEN CANCELLED".into());
                return true;
            };
            let previous_path = last_saved_path.borrow().clone();
            let result = load_project_transactionally(
                &path,
                previous_path.as_deref(),
                tracks,
                core,
                |core| core.load_project(&path),
            );
            detach_save_target_after_rollback_failure(ui, last_saved_path, &result);
            let ok = result.is_ok();
            if ok {
                reset_project_scoped_ui(ui);
                ui.set_master_output_gain(core.master_gain());
                *last_saved_path.borrow_mut() = Some(path.clone());
                persisted_snapshot.set(crate::ui::project_state::save_fingerprint(
                    core,
                    tracks,
                    ui.get_sequencer_patterns_json().as_str(),
                ));
                store_project_path(&path);
                ui.set_project_path(path.clone().into());
                ui.set_project_save_status("Saved".into());
            }
            ui.set_last_action(if ok {
                format!("OPENED: {}", display_path(&path)).into()
            } else {
                ui_error_message(
                    UiErrorKind::Project,
                    &format!(
                        "open failed ({:?}): {}",
                        result.unwrap_err(),
                        display_path(&path)
                    ),
                )
                .into()
            });
            true
        }
        "SAVE PROJECT" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                ui.set_last_action("PROJECT BUSY: SAVE IGNORED".into());
                return true;
            };
            let path = last_saved_path
                .borrow()
                .clone()
                .or_else(choose_project_save_file);
            save_to_path(
                core,
                ui,
                tracks,
                project_save_queue,
                path,
                "SAVED",
                ProjectSaveKind::Project,
                lease,
            );
            true
        }
        "FREEZE SELECTED TRACK" | "UNFREEZE SELECTED TRACK" => {
            let selected =
                crate::slint_ui::clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "freeze failed: no track selected")
                        .into(),
                );
                return true;
            };
            let track_id = track.id.max(0) as u32;
            let freezing = command == "FREEZE SELECTED TRACK";
            let ok = if freezing {
                core.freeze_track_to_project_end(track_id)
            } else {
                core.unfreeze_track(track_id)
            };
            if ok {
                sync_tracks_from_engine(tracks, core);
                ui.set_last_action(
                    if freezing {
                        "TRACK FROZEN · CPU LOAD REDUCED"
                    } else {
                        "TRACK UNFROZEN · LIVE PROCESSING RESTORED"
                    }
                    .into(),
                );
            } else {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        if freezing {
                            "track freeze rejected"
                        } else {
                            "track unfreeze rejected"
                        },
                    )
                    .into(),
                );
            }
            true
        }
        "RESTORE RECENT SESSION" => {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Recover) else {
                ui.set_last_action("PROJECT BUSY: RESTORE IGNORED".into());
                return true;
            };
            let Some(path) = load_saved_project_path() else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "no recent project session found")
                        .into(),
                );
                return true;
            };
            let recovered_backup = Cell::new(false);
            let previous_path = last_saved_path.borrow().clone();
            let mut result = load_project_transactionally(
                &path,
                previous_path.as_deref(),
                tracks,
                core,
                |core| core.load_project(&path),
            );
            if matches!(
                result,
                Err(crate::slint_ui::ProjectLoadTransactionError::LoadFailed)
                    | Err(crate::slint_ui::ProjectLoadTransactionError::HydrationFailed)
            ) {
                match restore_latest_project_backup_transactionally(
                    &path,
                    previous_path.as_deref(),
                    tracks,
                    core,
                ) {
                    Ok(_) => {
                        recovered_backup.set(true);
                        result = Ok(());
                    }
                    Err(error) => result = Err(error),
                }
            }
            detach_save_target_after_rollback_failure(ui, last_saved_path, &result);
            let ok = result.is_ok();
            if ok {
                reset_project_scoped_ui(ui);
                *last_saved_path.borrow_mut() = Some(path.clone());
                store_project_path(&path);
                ui.set_show_genesis(false);
                ui.set_project_path(path.clone().into());
                ui.set_project_save_status(if recovered_backup.get() {
                    "Recovered backup · Unsaved".into()
                } else {
                    "Restored · Save to Keep".into()
                });
                let weak = ui.as_weak();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_show_genesis(false);
                        ui.set_workspace_preset("arrange".into());
                    }
                });
            }
            ui.set_last_action(if ok {
                if recovered_backup.get() {
                    format!("RECOVERED BACKUP: {}", display_path(&path)).into()
                } else {
                    format!("RESTORED: {}", display_path(&path)).into()
                }
            } else {
                ui_error_message(
                    UiErrorKind::Project,
                    &format!(
                        "restore failed ({:?}): {}",
                        result.unwrap_err(),
                        display_path(&path)
                    ),
                )
                .into()
            });
            true
        }
        "SAVE DEMO" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                ui.set_last_action("PROJECT BUSY: SAVE IGNORED".into());
                return true;
            };
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("Hirari_Demo.hirari")
                .to_string_lossy()
                .into_owned();
            save_to_path(
                core,
                ui,
                tracks,
                project_save_queue,
                Some(path),
                "DEMO SAVED",
                ProjectSaveKind::Demo,
                lease,
            );
            true
        }
        "SAVE PROJECT AS" => {
            let Some(lease) = operation_gate.try_enter(OperationKind::Save) else {
                ui.set_last_action("PROJECT BUSY: SAVE IGNORED".into());
                return true;
            };
            let path = choose_project_save_file();
            save_to_path(
                core,
                ui,
                tracks,
                project_save_queue,
                path,
                "SAVED AS",
                ProjectSaveKind::SaveAs,
                lease,
            );
            true
        }
        "RECOVER LAST BACKUP" => {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Recover) else {
                ui.set_last_action("PROJECT BUSY: RECOVERY IGNORED".into());
                return true;
            };
            let Some(path) = last_saved_path.borrow().clone() else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "no saved project to recover").into(),
                );
                return true;
            };
            let previous_path = last_saved_path.borrow().clone();
            let result = restore_latest_project_backup_transactionally(
                &path,
                previous_path.as_deref(),
                tracks,
                core,
            )
            .map(|_| ());
            detach_save_target_after_rollback_failure(ui, last_saved_path, &result);
            let ok = result.is_ok();
            if ok {
                reset_project_scoped_ui(ui);
                ui.set_project_save_status("Recovered · Unsaved".into());
            }
            ui.set_last_action(if ok {
                format!("RECOVERED BACKUP: {}", display_path(&path)).into()
            } else {
                ui_error_message(
                    UiErrorKind::Project,
                    &format!("recovery failed ({:?})", result.unwrap_err()),
                )
                .into()
            });
            true
        }
        _ => false,
    }
}

fn save_to_path(
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    project_save_queue: &Rc<crate::ui::project_save_queue::ProjectSaveQueue>,
    path: Option<String>,
    success_prefix: &str,
    failure_kind: ProjectSaveKind,
    lease: crate::ui::operation_gate::OperationLease,
) -> bool {
    let Some(path) = path else {
        ui.set_last_action("SAVE CANCELLED".into());
        return false;
    };
    let Some(fingerprint) = crate::ui::project_state::save_fingerprint(
        core,
        tracks,
        ui.get_sequencer_patterns_json().as_str(),
    ) else {
        ui.set_last_action(ui_error_message(UiErrorKind::Project, "save state unavailable").into());
        return false;
    };
    match project_save_queue.enqueue(
        &path,
        tracks,
        core,
        ui.get_sequencer_patterns_json().as_str(),
        fingerprint,
        crate::ui::project_save_queue::SaveIntent::Manual {
            success_prefix: success_prefix.to_owned(),
            failure_label: failure_kind.failure_label().to_owned(),
        },
        lease,
    ) {
        Ok(()) => {
            ui.set_project_save_status("Saving project snapshot…".into());
            ui.set_last_action(format!("SAVING: {}", display_path(&path)).into());
            true
        }
        Err(error) => {
            ui.set_project_save_status("Save could not be queued".into());
            ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Project,
                    &format!(
                        "{}: {} ({error})",
                        failure_kind.failure_label(),
                        display_path(&path)
                    ),
                )
                .into(),
            );
            false
        }
    }
}

#[derive(Clone, Copy)]
enum ProjectSaveKind {
    Project,
    SaveAs,
    Demo,
}

impl ProjectSaveKind {
    fn failure_label(self) -> &'static str {
        match self {
            Self::Project => "save failed",
            Self::SaveAs => "save as failed",
            Self::Demo => "demo save failed",
        }
    }
}
