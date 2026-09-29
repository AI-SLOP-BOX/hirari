use hirari_core_bridge::piano_visualizer::{PianoNote, PianoVisualizer, PianoVisualizerConfig};
use hirari_core_bridge::HirariCore;
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
        "hirari-piano-mix-{}-{nonce}-{sequence}.wav",
        std::process::id(),
    ))
}

use crate::slint_ui::{
    choose_audio_file, clamp_selection_index, display_path, sync_tracks_from_engine,
    ui_error_message, AppWindow, EditorActions, RenderActions, UiErrorKind, Z_Track,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};

fn rollback_midi_import(
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &VecModel<Z_Track>,
    detail: &str,
) {
    let _ = core.abort_undo_transaction();
    sync_tracks_from_engine(tracks, core);
    crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
    ui.set_last_action(ui_error_message(UiErrorKind::Project, detail).into());
}

pub(crate) fn handle_command(
    command: &str,
    core: &Rc<HirariCore>,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
    operation_gate: &OperationGate,
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
        "ALIGN AUDIO TO REFERENCE" => {
            ui.global::<EditorActions>()
                .invoke_align_selected_audio(ui.get_sel_cid());
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
            let jobs = (0..tracks.row_count())
                .filter_map(|row| tracks.row_data(row))
                // Folder tracks only organize the project; unlike bus tracks,
                // they have no audio output to render as a stem.
                .filter(|track| track.id > 0 && track.r#type != "FOLD")
                .map(|track| (track.id as u32, track.name.to_string()))
                .collect::<Vec<_>>();
            let jobs = serde_json::to_string(&jobs).unwrap_or_default();
            ui.global::<RenderActions>()
                .invoke_start_stems(output_dir.to_string_lossy().as_ref().into(), jobs.into());
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
                .set_file_name("hirari-piano.mp4")
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
                    hirari_core_bridge::piano_visualizer::PianoVisualizerError::Encoder(
                        "Hirari project bounce failed".into(),
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
            if track.id <= 0 || track.r#type == "FOLD" {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Render,
                        if track.id <= 0 {
                            "selected track has no render ID"
                        } else {
                            "folder tracks do not produce audio stems"
                        },
                    )
                    .into(),
                );
                return true;
            }
            let jobs = serde_json::to_string(&[(track.id as u32, track.name.to_string())])
                .unwrap_or_default();
            ui.global::<RenderActions>()
                .invoke_start_stems(output_dir.to_string_lossy().as_ref().into(), jobs.into());
            true
        }
        "NEW PROJECT" => {
            let Some(_lease) = operation_gate.try_enter(OperationKind::Load) else {
                ui.set_last_action("PROJECT BUSY: NEW PROJECT IGNORED".into());
                return true;
            };
            // Keep the previous project's sidecar intact; it belongs to that
            // saved project even though this session no longer targets it.
            last_saved_path.borrow_mut().take();
            core.new_project();
            crate::ui::track_model::reset_project_scoped_track_overlays(tracks);
            sync_tracks_from_engine(tracks, core);
            crate::ui::project_commands::reset_project_scoped_ui(ui);
            ui.set_is_ply(false);
            ui.set_project_save_status("Unsaved project".into());
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
                crate::ui::audio_import::import_audio_async(
                    ui,
                    core.clone(),
                    tracks.clone(),
                    track.id.max(0) as u32,
                    path,
                    ui.get_ph().max(0.0) as f64,
                    false,
                );
            }
            true
        }
        "IMPORT MUSICXML" => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Import MusicXML Score")
                .add_filter("MusicXML", &["musicxml", "xml"])
                .pick_file()
            else {
                ui.set_last_action("MUSICXML IMPORT CANCELLED".into());
                return true;
            };
            let score = match hirari_core_bridge::musicxml::read_musicxml(&path) {
                Ok(score) => score,
                Err(error) => {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            &format!("MusicXML import failed: {error}"),
                        )
                        .into(),
                    );
                    return true;
                }
            };
            let tempo_events = score.tempo_events.clone();
            let source_parts = score
                .parts
                .into_iter()
                .enumerate()
                .filter(|(_, part)| !part.notes.is_empty())
                .collect::<Vec<_>>();
            if source_parts.is_empty() {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MusicXML import failed: score has no pitched notes",
                    )
                    .into(),
                );
                return true;
            }
            let part_count = source_parts.len();
            let mut notes: Vec<hirari_core_bridge::project_contracts::MidiNoteContract> =
                match serde_json::from_str(&core.midi_notes_json()) {
                    Ok(notes) => notes,
                    Err(error) => {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                &format!(
                                    "MusicXML import failed: current notes are invalid ({error})"
                                ),
                            )
                            .into(),
                        );
                        return true;
                    }
                };
            let original_notes = notes.clone();
            let current_tempos = core.get_tempo_events();
            let current_meters = core.get_time_signature_events();
            if !current_tempos.len().is_multiple_of(3) || !current_meters.len().is_multiple_of(3) {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MusicXML import failed: current tempo or meter map is invalid",
                    )
                    .into(),
                );
                return true;
            }
            core.begin_undo_transaction("Import MusicXML Score");
            if !tempo_events.is_empty() {
                for event in current_tempos.as_chunks::<3>().0.iter().rev() {
                    if event[0] > 0.0 && !core.remove_tempo_event(event[0]) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MusicXML import failed: could not replace the tempo map",
                        );
                        return true;
                    }
                }
                for event in &tempo_events {
                    if !core.set_tempo_event(event.beat, event.bpm, false) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MusicXML import failed: could not apply the tempo map",
                        );
                        return true;
                    }
                }
            }
            if !score.time_signatures.is_empty() {
                for event in current_meters.as_chunks::<3>().0.iter().rev() {
                    if event[0] > 0.0 && !core.remove_time_signature_event(event[0]) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MusicXML import failed: could not replace the meter map",
                        );
                        return true;
                    }
                }
                for event in &score.time_signatures {
                    if !core.set_time_signature_event(
                        event.beat,
                        event.numerator,
                        event.denominator,
                    ) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MusicXML import failed: could not apply the meter map",
                        );
                        return true;
                    }
                }
            }
            let mut imported_note_count = 0usize;
            let mut last_track_id = 0u32;
            for (source_index, source_part) in source_parts {
                let track_id = core.add_midi_track();
                if track_id == 0 {
                    rollback_midi_import(
                        core,
                        ui,
                        tracks,
                        "MusicXML import failed: could not create MIDI tracks",
                    );
                    return true;
                }
                last_track_id = track_id;
                let imported_name = source_part
                    .name
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(64)
                    .collect::<String>();
                let track_name = if imported_name.trim().is_empty() {
                    format!("Score {}", source_index + 1)
                } else {
                    imported_name.trim().to_owned()
                };
                if !core.set_track_name(track_id, &track_name) {
                    rollback_midi_import(
                        core,
                        ui,
                        tracks,
                        "MusicXML import failed: could not name MIDI tracks",
                    );
                    return true;
                }
                for note in source_part.notes {
                    let start_sample = core.beats_to_samples(note.start_beat);
                    let end_sample = core.beats_to_samples(note.start_beat + note.length_beats);
                    notes.push(hirari_core_bridge::project_contracts::MidiNoteContract {
                        track_id,
                        region_id: 0,
                        pitch: note.pitch,
                        midi_channel: note.voice,
                        articulation: 0,
                        velocity: note.velocity,
                        start_sample,
                        length_samples: end_sample.saturating_sub(start_sample).max(1),
                        lyric: note.lyric,
                        phoneme: String::new(),
                        pitch_curve_cents: Vec::new(),
                        vibrato_depth_cents: 0,
                        vibrato_rate_millihz: 5_000,
                        portamento_samples: 0,
                        probability: 100,
                        repeat_count: 1,
                    });
                    imported_note_count += 1;
                }
            }
            if !core.replace_midi_note_contracts(notes, true) {
                rollback_midi_import(
                    core,
                    ui,
                    tracks,
                    "MusicXML import failed: Core rejected notes",
                );
                return true;
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                let _ = core.replace_midi_note_contracts(original_notes, false);
                sync_tracks_from_engine(tracks, core);
                crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MusicXML import failed: could not commit the score import",
                    )
                    .into(),
                );
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
            if last_track_id != 0 {
                let selected = (0..tracks.row_count()).find(|row| {
                    tracks
                        .row_data(*row)
                        .is_some_and(|track| track.id == last_track_id as i32)
                });
                if let Some(row) = selected {
                    ui.set_sel_idx(row as i32);
                }
            }
            ui.set_last_action(
                format!(
                    "IMPORTED {imported_note_count} MUSICXML NOTES · {} PARTS · {} · {}",
                    part_count,
                    if tempo_events.is_empty() {
                        "PROJECT TEMPO KEPT"
                    } else {
                        "TEMPO MAP IMPORTED"
                    },
                    display_path(&path)
                )
                .into(),
            );
            true
        }
        "IMPORT MIDI" => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Import Standard MIDI File")
                .add_filter("Standard MIDI", &["mid", "midi"])
                .pick_file()
            else {
                ui.set_last_action("MIDI IMPORT CANCELLED".into());
                return true;
            };
            let imported = match hirari_core_bridge::midi_file::read_standard_midi(&path) {
                Ok(file) => file,
                Err(error) => {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            &format!("MIDI import failed: {error}"),
                        )
                        .into(),
                    );
                    return true;
                }
            };
            let import_tempo_map = imported.has_explicit_tempo_map;
            let tempo_map = imported.tempo_map.clone();
            let time_signature_map = imported.time_signature_map.clone();
            let import_time_signature_map = !time_signature_map.is_empty();
            let source_tracks = imported
                .tracks
                .into_iter()
                .enumerate()
                .filter(|(_, track)| !track.notes.is_empty())
                .collect::<Vec<_>>();
            if source_tracks.is_empty() && !import_tempo_map && !import_time_signature_map {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MIDI import failed: file has no notes",
                    )
                    .into(),
                );
                return true;
            }

            let mut notes: Vec<hirari_core_bridge::project_contracts::MidiNoteContract> =
                match serde_json::from_str(&core.midi_notes_json()) {
                    Ok(notes) => notes,
                    Err(error) => {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                &format!("MIDI import failed: current notes are invalid ({error})"),
                            )
                            .into(),
                        );
                        return true;
                    }
                };
            let original_notes = notes.clone();

            let current_tempo = core.get_tempo_events();
            let current_time_signatures = core.get_time_signature_events();
            if !current_tempo.len().is_multiple_of(3)
                || !current_time_signatures.len().is_multiple_of(3)
            {
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MIDI import failed: current tempo or meter map is invalid",
                    )
                    .into(),
                );
                return true;
            }

            core.begin_undo_transaction("Import MIDI");
            if import_tempo_map {
                for event in current_tempo.as_chunks::<3>().0.iter().rev() {
                    if event[0] > 0.0 && !core.remove_tempo_event(event[0]) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MIDI import failed: could not replace the tempo map",
                        );
                        return true;
                    }
                }
                for event in &tempo_map {
                    if !core.set_tempo_event(event.beat, event.bpm, event.ramp) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MIDI import failed: could not apply the MIDI tempo map",
                        );
                        return true;
                    }
                }
            }
            if import_time_signature_map {
                for event in current_time_signatures.as_chunks::<3>().0.iter().rev() {
                    if event[0] > 0.0 && !core.remove_time_signature_event(event[0]) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MIDI import failed: could not replace the meter map",
                        );
                        return true;
                    }
                }
                for event in &time_signature_map {
                    let numerator = event.numerator;
                    let denominator = event.denominator;
                    if !core.set_time_signature_event(event.beat, numerator, denominator) {
                        rollback_midi_import(
                            core,
                            ui,
                            tracks,
                            "MIDI import failed: could not apply the MIDI meter map",
                        );
                        return true;
                    }
                }
            }

            let mut imported_note_count = 0usize;
            for (source_index, source_track) in source_tracks {
                let track_id = core.add_midi_track();
                if track_id == 0 {
                    rollback_midi_import(
                        core,
                        ui,
                        tracks,
                        "MIDI import failed: could not create tracks",
                    );
                    return true;
                }
                let imported_name = source_track
                    .name
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(64)
                    .collect::<String>();
                let track_name = if imported_name.trim().is_empty() {
                    format!("MIDI {}", source_index + 1)
                } else {
                    imported_name.trim().to_owned()
                };
                if !core.set_track_name(track_id, &track_name) {
                    rollback_midi_import(
                        core,
                        ui,
                        tracks,
                        "MIDI import failed: could not name tracks",
                    );
                    return true;
                }
                for note in source_track.notes {
                    let start_beat = note.start_tick as f64 / f64::from(imported.ppq);
                    let end_beat = note.end_tick as f64 / f64::from(imported.ppq);
                    let start_sample = core.beats_to_samples(start_beat);
                    let end_sample = core.beats_to_samples(end_beat.max(start_beat));
                    let length_samples = end_sample.saturating_sub(start_sample).max(1);
                    notes.push(hirari_core_bridge::project_contracts::MidiNoteContract {
                        // Standard MIDI files identify notes by track/channel,
                        // not by an arrangement-region ID. Keep imported notes
                        // track-scoped until they are explicitly placed in a
                        // region in the editor.
                        region_id: 0,
                        track_id,
                        pitch: note.pitch,
                        midi_channel: note.midi_channel,
                        articulation: 0,
                        velocity: note.velocity,
                        start_sample,
                        length_samples,
                        lyric: note.lyric,
                        phoneme: String::new(),
                        pitch_curve_cents: Vec::new(),
                        vibrato_depth_cents: 0,
                        vibrato_rate_millihz: 5_000,
                        portamento_samples: 0,
                        probability: 100,
                        repeat_count: 1,
                    });
                    imported_note_count += 1;
                }
            }

            if imported_note_count > 0 && !core.replace_midi_note_contracts(notes, true) {
                rollback_midi_import(core, ui, tracks, "MIDI import failed: Core rejected notes");
                return true;
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                let _ = core.replace_midi_note_contracts(original_notes, false);
                sync_tracks_from_engine(tracks, core);
                crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
                ui.set_last_action(
                    ui_error_message(
                        UiErrorKind::Project,
                        "MIDI import failed: could not commit the import operation",
                    )
                    .into(),
                );
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
            if imported_note_count > 0 {
                ui.set_sel_idx((tracks.row_count().saturating_sub(1)) as i32);
            }
            let tempo_status = if import_tempo_map {
                "TEMPO MAP IMPORTED"
            } else {
                "PROJECT TEMPO KEPT"
            };
            let meter_status = if import_time_signature_map {
                "METER MAP IMPORTED"
            } else {
                "PROJECT METER KEPT"
            };
            ui.set_last_action(
                format!(
                    "IMPORTED {imported_note_count} MIDI NOTES · {tempo_status} · {meter_status} · {}",
                    display_path(&path)
                )
                .into(),
            );
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
