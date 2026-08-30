use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use crate::slint_ui::{
    choose_audio_file, clamp_selection_index, display_path, midi_notes_path,
    sync_tracks_from_engine, ui_error_message, AppWindow, EditorActions, UiErrorKind, Z_Track,
};

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
) -> bool {
    match command {
        "SELECT ALL CLIPS" => {
            ui.global::<EditorActions>().invoke_select_all_clips();
            true
        }
        "CLEAR CLIP SELECTION" => {
            ui.global::<EditorActions>().invoke_clear_clip_selection();
            true
        }
        "COPY SELECTED CLIPS" => {
            ui.global::<EditorActions>().invoke_copy_selected_clips();
            true
        }
        "PASTE CLIPS" => {
            ui.global::<EditorActions>().invoke_paste_clips(ui.get_ph());
            true
        }
        "EXPORT STEMS" => {
            let Some(output_dir) = rfd::FileDialog::new()
                .set_title("Export stems")
                .pick_folder()
            else {
                ui.set_last_action("STEM EXPORT CANCELLED".into());
                return true;
            };
            ui.set_last_action("STEM EXPORTING…".into());
            let diagnostic = core.bounce_stems_diagnostic_json(
                &output_dir.to_string_lossy(),
                0,
                &[],
                2.0,
                false,
                true,
            );
            match serde_json::from_str::<serde_json::Value>(&diagnostic) {
                Ok(result)
                    if result.get("ok").and_then(serde_json::Value::as_bool) == Some(true) =>
                {
                    let count = result
                        .get("outputs")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len);
                    ui.set_last_action(
                        format!("STEMS EXPORTED: {count} · {}", display_path(&output_dir)).into(),
                    );
                }
                Ok(result) => {
                    let code = result
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("stem_export_failed");
                    let message = result
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(code);
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, &format!("{code}: {message}")).into(),
                    );
                }
                Err(_) => {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, "invalid stem export response")
                            .into(),
                    );
                }
            }
            true
        }
        "EXPORT SELECTED STEM" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "stem export failed: no track selected")
                        .into(),
                );
                return true;
            };
            let Some(output_dir) = rfd::FileDialog::new()
                .set_title("Export selected stem")
                .pick_folder()
            else {
                ui.set_last_action("SELECTED STEM EXPORT CANCELLED".into());
                return true;
            };
            ui.set_last_action("SELECTED STEM EXPORTING…".into());
            let diagnostic = core.bounce_stems_diagnostic_json(
                &output_dir.to_string_lossy(),
                0,
                &[track.id.max(0) as u32],
                2.0,
                false,
                true,
            );
            match serde_json::from_str::<serde_json::Value>(&diagnostic) {
                Ok(result)
                    if result.get("ok").and_then(serde_json::Value::as_bool) == Some(true) =>
                {
                    ui.set_last_action(
                        format!(
                            "STEM EXPORTED: {} · {}",
                            track.name,
                            display_path(&output_dir)
                        )
                        .into(),
                    );
                }
                Ok(result) => {
                    let code = result
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("stem_export_failed");
                    ui.set_last_action(ui_error_message(UiErrorKind::Render, code).into());
                }
                Err(_) => {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, "invalid stem export response")
                            .into(),
                    );
                }
            }
            true
        }
        "NEW PROJECT" => {
            if let Some(path) = last_saved_path.borrow_mut().take() {
                let _ = fs::remove_file(midi_notes_path(&path));
            }
            core.new_project();
            sync_tracks_from_engine(tracks, core);
            ui.set_is_ply(false);
            ui.set_auto_save_status("Auto-save: New Project".into());
            ui.set_last_action("NEW PROJECT".into());
            true
        }
        "DUPLICATE SELECTED TRACK" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "duplicate failed: no track selected")
                        .into(),
                );
                return true;
            };
            let new_id = core.duplicate_track(track.id.max(0) as u32);
            if new_id != 0 {
                sync_tracks_from_engine(tracks, core);
                ui.set_last_action(format!("DUPLICATED TRACK {} → {}", track.id, new_id).into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "track duplication rejected by Core")
                        .into(),
                );
            }
            true
        }
        "DELETE SELECTED TRACK" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "delete failed: no track selected")
                        .into(),
                );
                return true;
            };
            if core.remove_track(track.id.max(0) as u32) {
                sync_tracks_from_engine(tracks, core);
                ui.set_sel_cid(-1);
                ui.set_sel_idx(clamp_selection_index(selected as i32, tracks.row_count()) as i32);
                ui.set_last_action(format!("DELETED TRACK {}", track.id).into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "track deletion rejected by Core")
                        .into(),
                );
            }
            true
        }
        "RELINK MISSING" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let clip_id = ui.get_sel_cid();
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "relink failed: no track").into(),
                );
                return true;
            };
            if clip_id < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "relink failed: no clip").into(),
                );
                return true;
            }
            if let Some(path) = choose_audio_file() {
                let replaced =
                    core.replace_region_audio(track.id.max(0) as u32, clip_id as u32, &path);
                if replaced {
                    sync_tracks_from_engine(tracks, core);
                }
                ui.set_last_action(if replaced {
                    format!("RELINKED: {}", display_path(&path)).into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        &format!("relink failed: {}", display_path(&path)),
                    )
                    .into()
                });
            }
            true
        }
        "IMPORT AUDIO" => {
            if tracks.row_count() == 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "import failed: create a track first")
                        .into(),
                );
                return true;
            }
            if let Some(path) = choose_audio_file() {
                let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
                let Some(track) = tracks.row_data(selected) else {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "import failed: no track selected")
                            .into(),
                    );
                    return true;
                };
                let imported = core.add_region_at_beat(
                    track.id.max(0) as u32,
                    &path,
                    ui.get_ph().max(0.0) as f64,
                );
                if imported {
                    sync_tracks_from_engine(tracks, core);
                }
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
            true
        }
        "DUPLICATE SELECTED CLIP" => {
            let clip_id = ui.get_sel_cid();
            if clip_id < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "duplicate failed: no clip selected")
                        .into(),
                );
                return true;
            }
            ui.global::<EditorActions>().invoke_duplicate_clip(clip_id);
            true
        }
        "DELETE SELECTED CLIP" => {
            let clip_id = ui.get_sel_cid();
            if clip_id < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "delete failed: no clip selected")
                        .into(),
                );
                return true;
            }
            ui.global::<EditorActions>().invoke_remove_clip(clip_id);
            true
        }
        "SPLIT SELECTED CLIP" => {
            let clip_id = ui.get_sel_cid();
            if clip_id < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Project, "split failed: no clip selected").into(),
                );
                return true;
            }
            let split_at = (0..tracks.row_count())
                .filter_map(|row| tracks.row_data(row))
                .flat_map(|track| track.clips.iter().collect::<Vec<_>>())
                .find(|clip| clip.id == clip_id)
                .map(|clip| clip.start_beat + clip.length_beats * 0.5)
                .unwrap_or(0.0);
            ui.global::<EditorActions>()
                .invoke_split_clip(clip_id, split_at);
            true
        }
        _ => false,
    }
}
