use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{
    choose_project_file, choose_project_save_file, display_path, hydrate_project_models,
    load_saved_project_path, save_ui_midi_notes, store_project_path, ui_error_message, AppWindow,
    UiErrorKind, Z_Track,
};
use crate::ui::track_model::sync_tracks_from_engine;

fn reset_project_scoped_ui(ui: &AppWindow) {
    ui.set_sidechain_source_id(-1);
    ui.set_sidechain_source_name("".into());
    ui.set_sidechain_result(false);
    ui.set_sidechain_tap_point(0);
    ui.set_plugin_pdc_status("CALCULATING".into());
    ui.set_plugin_pdc_compensation_ms(0.0);
}

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
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
        "OPEN PROJECT" => {
            let Some(path) = choose_project_file() else {
                ui.set_last_action("OPEN CANCELLED".into());
                return true;
            };
            let loaded = core.load_project(&path);
            let ok = loaded && hydrate_project_models(&path, tracks, core);
            if ok {
                reset_project_scoped_ui(ui);
                *last_saved_path.borrow_mut() = Some(path.clone());
                store_project_path(&path);
                ui.set_project_path(path.clone().into());
                ui.set_auto_save_status("Auto-save: Ready".into());
            }
            ui.set_last_action(if ok {
                format!("OPENED: {}", display_path(&path)).into()
            } else {
                ui_error_message(
                    UiErrorKind::Project,
                    &format!("open failed: {}", display_path(&path)),
                )
                .into()
            });
            true
        }
        "SAVE PROJECT" => {
            let path = last_saved_path
                .borrow()
                .clone()
                .or_else(choose_project_save_file);
            save_to_path(
                core,
                ui,
                tracks,
                last_saved_path,
                path,
                "SAVED",
                ProjectSaveKind::Project,
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
            let Some(path) = load_saved_project_path() else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "no recent project session found")
                        .into(),
                );
                return true;
            };
            let mut loaded = core.load_project(&path);
            let mut recovered_backup = false;
            // A just-written native snapshot can be unreadable after a crash
            // during plugin-state serialization. Prefer the newest validated
            // recovery generation over presenting an empty session.
            if !loaded && core.restore_project_backup(&path, 1) {
                loaded = true;
                recovered_backup = true;
            }
            let ok = loaded && hydrate_project_models(&path, tracks, core);
            if ok {
                reset_project_scoped_ui(ui);
                *last_saved_path.borrow_mut() = Some(path.clone());
                store_project_path(&path);
                ui.set_project_path(path.clone().into());
                ui.set_auto_save_status(if recovered_backup {
                    "Recovered backup · Auto-save Ready".into()
                } else {
                    "Restored · Auto-save Ready".into()
                });
            }
            ui.set_last_action(if ok {
                if recovered_backup {
                    format!("RECOVERED BACKUP: {}", display_path(&path)).into()
                } else {
                    format!("RESTORED: {}", display_path(&path)).into()
                }
            } else {
                ui_error_message(
                    UiErrorKind::Project,
                    &format!("restore failed: {}", display_path(&path)),
                )
                .into()
            });
            true
        }
        "SAVE DEMO" => {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("Aura_Demo.aura")
                .to_string_lossy()
                .into_owned();
            save_to_path(
                core,
                ui,
                tracks,
                last_saved_path,
                Some(path),
                "DEMO SAVED",
                ProjectSaveKind::Demo,
            );
            true
        }
        "SAVE PROJECT AS" => {
            let path = choose_project_save_file();
            save_to_path(
                core,
                ui,
                tracks,
                last_saved_path,
                path,
                "SAVED AS",
                ProjectSaveKind::SaveAs,
            );
            true
        }
        "RECOVER LAST BACKUP" => {
            let Some(path) = last_saved_path.borrow().clone() else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "no saved project to recover").into(),
                );
                return true;
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
            let recovered =
                generation.is_some_and(|generation| core.restore_project_backup(&path, generation));
            let ok = recovered && hydrate_project_models(&path, tracks, core);
            if recovered && !ok {
                let _ = core.load_project(&path);
            }
            if ok {
                reset_project_scoped_ui(ui);
                ui.set_auto_save_status("Recovered · Auto-save Ready".into());
            }
            ui.set_last_action(if ok {
                format!("RECOVERED BACKUP: {}", display_path(&path)).into()
            } else {
                ui_error_message(UiErrorKind::Project, "no valid recovery backup found").into()
            });
            true
        }
        _ => false,
    }
}

fn save_to_path(
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
    path: Option<String>,
    success_prefix: &str,
    failure_kind: ProjectSaveKind,
) -> bool {
    let Some(path) = path else {
        ui.set_last_action("SAVE CANCELLED".into());
        return false;
    };
    let saved = core.save_project(&path);
    let notes_saved = saved && save_ui_midi_notes(&path, tracks);
    if notes_saved {
        *last_saved_path.borrow_mut() = Some(path.clone());
        store_project_path(&path);
        ui.set_project_path(path.clone().into());
        ui.set_auto_save_status("Saved · Auto-save Ready".into());
        ui.set_last_action(format!("{}: {}", success_prefix, display_path(&path)).into());
    } else {
        ui.set_last_action(
            ui_error_message(
                UiErrorKind::Project,
                &format!("{}: {}", failure_kind.failure_label(), display_path(&path)),
            )
            .into(),
        );
    }
    notes_saved
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
