use aura_core_bridge::piano_visualizer::{PianoNote, PianoVisualizer, PianoVisualizerConfig};
use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PIANO_BOUNCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn beat_to_seconds(beat: f64, packed_events: &[f64], fallback_bpm: f32) -> f64 {
    if !beat.is_finite() || beat <= 0.0 {
        return 0.0;
    }
    let mut cursor_beat = 0.0;
    let mut cursor_seconds = 0.0;
    let mut bpm = f64::from(fallback_bpm.max(1.0));
    let mut ramp = false;
    for event in packed_events.as_chunks::<3>().0 {
        let event_beat = event[0];
        let event_bpm = event[1];
        if !event_beat.is_finite() || !event_bpm.is_finite() || event_beat < cursor_beat {
            continue;
        }
        if event_beat > beat {
            break;
        }
        let segment_beats = event_beat - cursor_beat;
        if segment_beats > 0.0 {
            let effective_bpm = if ramp {
                (bpm + event_bpm.max(1.0)) * 0.5
            } else {
                bpm
            };
            cursor_seconds += segment_beats * 60.0 / effective_bpm.max(1.0);
        }
        cursor_beat = event_beat;
        bpm = event_bpm.max(1.0);
        ramp = event[2].is_finite() && event[2] > 0.5;
    }
    cursor_seconds + (beat - cursor_beat).max(0.0) * 60.0 / bpm.max(1.0)
}

fn piano_bounce_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = PIANO_BOUNCE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "aura-piano-mix-{}-{nonce}-{sequence}.wav",
        std::process::id(),
    ))
}

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
        "PIANO VISUALIZER" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action("PIANO VISUALIZER: SELECT A MIDI TRACK".into());
                return true;
            };
            // MIDI note positions are stored in beats.  Convert using the
            // project's active tempo instead of the old 120-BPM shortcut so
            // the rendered piano video stays sample-accurate for user tempo
            // changes (the tempo map's first event is the Core fallback).
            let fallback_bpm = core.get_tempo();
            let tempo_events = core.get_tempo_events();
            let notes: Vec<PianoNote> = track
                .piano_roll_notes
                .iter()
                .filter_map(|note| {
                    let start_beat = f64::from(note.start_beat).max(0.0);
                    let end_beat = start_beat + f64::from(note.length_beats.max(0.0));
                    let start = beat_to_seconds(start_beat, &tempo_events, fallback_bpm);
                    let duration =
                        (beat_to_seconds(end_beat, &tempo_events, fallback_bpm) - start).max(0.0);
                    (duration > 0.0).then_some(PianoNote {
                        start_seconds: start.max(0.0),
                        duration_seconds: duration,
                        pitch: note.pitch.clamp(0, 127) as u8,
                        velocity: note.velocity.clamp(1, 127) as u8,
                        channel: 0,
                    })
                })
                .collect();
            let Some(output) = rfd::FileDialog::new()
                .set_title("Export Piano Visualizer MP4")
                .add_filter("MP4 video", &["mp4"])
                .set_file_name("aura-piano.mp4")
                .save_file()
            else {
                ui.set_last_action("PIANO VISUALIZER CANCELLED".into());
                return true;
            };
            ui.set_last_action("PIANO VISUALIZER RENDERING…".into());
            let bounced_audio = piano_bounce_path();
            let result = if core.bounce_project(bounced_audio.to_string_lossy().as_ref(), 0) {
                PianoVisualizer::render_to_mp4_from_wav(
                    &notes,
                    &bounced_audio,
                    &PianoVisualizerConfig::default(),
                    &output,
                )
            } else {
                Err(
                    aura_core_bridge::piano_visualizer::PianoVisualizerError::Encoder(
                        "Aura project bounce failed".into(),
                    ),
                )
            };
            let _ = fs::remove_file(&bounced_audio);
            match result {
                Ok(()) => ui.set_last_action(
                    format!("PIANO VIDEO EXPORTED · {}", display_path(&output)).into(),
                ),
                Err(error) => ui.set_last_action(format!("PIANO VIDEO FAILED · {error}").into()),
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

#[cfg(test)]
mod tests {
    #[test]
    fn piano_visualizer_uses_project_tempo_for_beat_conversion() {
        assert!((super::beat_to_seconds(1.0, &[], 120.0) - 0.5).abs() < f64::EPSILON);
        assert!((super::beat_to_seconds(1.0, &[], 60.0) - 1.0).abs() < f64::EPSILON);
        assert!((super::beat_to_seconds(1.0, &[], 240.0) - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn piano_visualizer_clamps_invalid_tempo_safely() {
        assert!((super::beat_to_seconds(1.0, &[], 0.0) - 60.0).abs() < f64::EPSILON);
        assert!((super::beat_to_seconds(1.0, &[], -10.0) - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn piano_visualizer_integrates_tempo_events() {
        let events = [0.0, 120.0, 0.0, 4.0, 60.0, 0.0];
        assert!((super::beat_to_seconds(4.0, &events, 120.0) - 2.0).abs() < 1e-9);
        assert!((super::beat_to_seconds(6.0, &events, 120.0) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn piano_visualizer_bounce_paths_are_unique() {
        let first = super::piano_bounce_path();
        let second = super::piano_bounce_path();
        assert_ne!(first, second);
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("wav"));
    }
}
