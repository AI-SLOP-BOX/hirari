use hirari_core_bridge::project::{ProjectDocument, StepSequencerPatternContract};
use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::Cell;
use std::rc::Rc;

use crate::slint_ui::{
    sync_midi_notes_to_core, AppWindow, MidiActions, StepActions, ZNote, Z_Track,
};
use crate::ui::sync::replace_track;

fn step_duration_beats(ui: &AppWindow) -> f32 {
    let resolution = ui.get_sequencer_step_resolution().clamp(4, 64) as f32;
    let feel = match ui.get_sequencer_step_feel() {
        1 => 2.0 / 3.0,
        2 => 1.5,
        _ => 1.0,
    };
    4.0 / resolution * feel
}

fn sanitize_notes(notes: &mut Vec<ZNote>) {
    notes.retain(|note| note.start_beat.is_finite() && note.length_beats.is_finite());
    for note in notes.iter_mut() {
        note.pitch = note.pitch.clamp(0, 127);
        note.start_beat = note.start_beat.max(0.0);
        note.length_beats = note.length_beats.max(0.015625);
        note.velocity = note.velocity.clamp(1, 127);
    }
    notes.sort_by(|a, b| {
        a.start_beat
            .partial_cmp(&b.start_beat)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn step_sequencer_patterns(ui: &AppWindow) -> Vec<StepSequencerPatternContract> {
    serde_json::from_str(ui.get_sequencer_patterns_json().as_str()).unwrap_or_default()
}

pub(crate) fn copy_step_sequencer_pattern(
    ui: &AppWindow,
    track_id: u32,
    source_region_id: u32,
    destination_region_id: u32,
) {
    let mut patterns = step_sequencer_patterns(ui);
    let Some(mut pattern) = patterns
        .iter()
        .find(|pattern| pattern.track_id == track_id && pattern.region_id == source_region_id)
        .cloned()
    else {
        return;
    };
    pattern.region_id = destination_region_id;
    patterns.retain(|existing| {
        existing.track_id != track_id || existing.region_id != destination_region_id
    });
    patterns.push(pattern);
    set_step_sequencer_patterns(ui, patterns);
}

fn set_step_sequencer_patterns(ui: &AppWindow, mut patterns: Vec<StepSequencerPatternContract>) {
    patterns.sort_by_key(|pattern| pattern.region_id);
    if let Ok(json) = serde_json::to_string(&patterns) {
        ui.set_sequencer_patterns_json(json.into());
        crate::ui::project_state::mark_step_sequencer_patterns_changed();
    }
}

fn ensure_step_sequencer_patterns_loaded(ui: &AppWindow) {
    let project_path = ui.get_project_path().to_string();
    if ui.get_sequencer_patterns_project_path().as_str() == project_path {
        return;
    }
    let patterns = if project_path.trim().is_empty() {
        Vec::new()
    } else {
        ProjectDocument::load(&project_path)
            .map(|document| document.step_sequencer_patterns)
            .unwrap_or_default()
    };
    set_step_sequencer_patterns(ui, patterns);
    ui.set_sequencer_patterns_project_path(project_path.into());
}

fn active_lane_pitches(ui: &AppWindow) -> Option<[u8; 6]> {
    let model = ui.get_sequencer_lane_pitches();
    if model.row_count() != 6 {
        return None;
    }
    let pitches: [i32; 6] = std::array::from_fn(|index| model.row_data(index).unwrap_or(-1));
    if pitches.iter().any(|pitch| !(0..=127).contains(pitch)) {
        return None;
    }
    let mut result = [0_u8; 6];
    for (index, pitch) in pitches.into_iter().enumerate() {
        result[index] = u8::try_from(pitch).ok()?;
    }
    Some(result)
}

fn set_active_lane_pitches(ui: &AppWindow, pitches: [u8; 6]) {
    let pitches = pitches.map(i32::from).to_vec();
    ui.set_sequencer_lane_pitches(slint::ModelRc::new(VecModel::from(pitches)));
}

fn persist_active_step_sequencer_pattern(ui: &AppWindow) {
    if ui.get_sequencer_patterns_project_path().as_str() != ui.get_project_path().as_str() {
        return;
    }
    let track_id = ui.get_sequencer_region_track_id();
    let region_id = ui.get_sequencer_region_id();
    let (Ok(track_id), Ok(region_id), Some(lane_pitches)) = (
        u32::try_from(track_id),
        u32::try_from(region_id),
        active_lane_pitches(ui),
    ) else {
        return;
    };
    if track_id == 0 || region_id == 0 {
        return;
    }
    let resolution = ui.get_sequencer_step_resolution();
    let feel = ui.get_sequencer_step_feel();
    if ![4, 8, 16, 32, 64].contains(&resolution) || !(0..=2).contains(&feel) {
        return;
    }
    let mut patterns = step_sequencer_patterns(ui);
    patterns.retain(|pattern| pattern.region_id != region_id);
    patterns.push(StepSequencerPatternContract {
        track_id,
        region_id,
        lane_pitches,
        resolution: resolution as u8,
        feel: feel as u8,
    });
    set_step_sequencer_patterns(ui, patterns);
}

fn apply_step_sequencer_pattern(ui: &AppWindow, track_id: i32, region_id: i32) {
    let Some(pattern) = step_sequencer_patterns(ui).into_iter().find(|pattern| {
        pattern.track_id == track_id as u32 && pattern.region_id == region_id as u32
    }) else {
        set_active_lane_pitches(ui, [36, 38, 42, 46, 40, 45]);
        ui.set_sequencer_step_resolution(16);
        ui.set_sequencer_step_feel(0);
        return;
    };
    set_active_lane_pitches(ui, pattern.lane_pitches);
    ui.set_sequencer_step_resolution(i32::from(pattern.resolution));
    ui.set_sequencer_step_feel(i32::from(pattern.feel));
}

fn step_sequencer_lane_pitch(ui: &AppWindow, lane: i32) -> Option<i32> {
    if !(0..6).contains(&lane) {
        return None;
    }
    ui.get_sequencer_lane_pitches().row_data(lane as usize)
}

pub fn install(ui: &AppWindow, core: Rc<HirariCore>, tracks: Rc<VecModel<Z_Track>>) {
    let pending_note_edit = Rc::new(Cell::new(None::<i32>));
    ui.global::<StepActions>().on_load_pattern({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        move |track_id| {
            let selected = (0..tracks.row_count())
                .find_map(|row| tracks.row_data(row).filter(|track| track.id == track_id));
            if let Some(ui) = weak.upgrade() {
                refresh_step_sequencer_region(&ui, selected.as_ref());
            }
        }
    });
    ui.global::<StepActions>().on_set_step_resolution({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        move |resolution| {
            if ![4, 8, 16, 32, 64].contains(&resolution) {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.set_sequencer_step_resolution(resolution);
            persist_active_step_sequencer_pattern(&ui);
            let track_id = ui.get_sequencer_region_track_id();
            let track = (0..tracks.row_count())
                .find_map(|row| tracks.row_data(row).filter(|track| track.id == track_id));
            refresh_step_sequencer_state(&ui, track.as_ref());
            refresh_step_sequencer_page_count(&ui);
        }
    });
    ui.global::<StepActions>().on_set_step_feel({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        move |feel| {
            if !(0..=2).contains(&feel) {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.set_sequencer_step_feel(feel);
            persist_active_step_sequencer_pattern(&ui);
            let track_id = ui.get_sequencer_region_track_id();
            let track = (0..tracks.row_count())
                .find_map(|row| tracks.row_data(row).filter(|track| track.id == track_id));
            refresh_step_sequencer_state(&ui, track.as_ref());
            refresh_step_sequencer_page_count(&ui);
        }
    });
    ui.global::<StepActions>().on_set_lane_pitch({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |track_id, lane, new_pitch| {
            if track_id < 0 || !(0..6).contains(&lane) || !(0..=127).contains(&new_pitch) {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_id = ui.get_sequencer_region_id();
            if region_id < 0
                || ui.get_sequencer_region_track_id() != track_id
                || ui.get_sequencer_lane_pitches().row_count() != 6
            {
                return;
            }
            let Some(mut pitches) = active_lane_pitches(&ui) else {
                return;
            };
            if pitches
                .iter()
                .enumerate()
                .any(|(index, pitch)| index != lane as usize && i32::from(*pitch) == new_pitch)
            {
                ui.set_last_action("STEP LANE PITCHES MUST BE UNIQUE".into());
                return;
            }
            let old_pitch = i32::from(pitches[lane as usize]);
            if old_pitch == new_pitch {
                return;
            }
            let Some(row) = (0..tracks.row_count()).find(|&row| {
                tracks
                    .row_data(row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(track) = tracks.row_data(row) else {
                return;
            };
            if !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
                || !step_region_exists(&track, region_id)
            {
                return;
            }
            let mut notes = track.piano_roll_notes.iter().collect::<Vec<_>>();
            let mut changed = 0usize;
            for note in &mut notes {
                if note.pitch == old_pitch && step_note_belongs_to_region(&track, note, region_id) {
                    note.pitch = new_pitch;
                    note.region_id = region_id;
                    changed += 1;
                }
            }
            if changed > 0 {
                sanitize_notes(&mut notes);
                let mut updated = track.clone();
                updated.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                replace_track(&tracks, row, updated);
                if !sync_midi_notes_to_core("", &tracks, &core) {
                    replace_track(&tracks, row, track);
                    ui.set_last_action("STEP LANE PITCH CHANGE REJECTED BY CORE".into());
                    return;
                }
            }
            pitches[lane as usize] = new_pitch as u8;
            set_active_lane_pitches(&ui, pitches);
            persist_active_step_sequencer_pattern(&ui);
            let refreshed = (0..tracks.row_count())
                .find_map(|row| tracks.row_data(row).filter(|track| track.id == track_id));
            refresh_step_sequencer_state(&ui, refreshed.as_ref());
            ui.set_last_action(
                format!(
                    "STEP LANE {} MIDI PITCH: {} → {} · {} NOTES",
                    lane + 1,
                    old_pitch,
                    new_pitch,
                    changed
                )
                .into(),
            );
        }
    });
    ui.global::<StepActions>().on_step_toggled({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |track_id, row, step, active| {
            if track_id < 0 || !(0..6).contains(&row) || !(0..128).contains(&step) {
                return;
            }
            let Some(track) = (0..tracks.row_count())
                .find_map(|index| tracks.row_data(index).filter(|track| track.id == track_id))
            else {
                return;
            };
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_start = ui.get_sequencer_region_start_beat();
            let region_length = ui.get_sequencer_region_length_beats();
            let step_beats = step_duration_beats(&ui);
            if ui.get_sequencer_region_id() < 0
                || ui.get_sequencer_region_track_id() != track_id
                || !step_region_exists(&track, ui.get_sequencer_region_id())
                || !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
                || !region_start.is_finite()
                || !region_length.is_finite()
                || !(region_start + region_length).is_finite()
            {
                return;
            }
            let beat = region_start + step as f32 * step_beats;
            let relative_beat = step as f32 * step_beats;
            let remaining_region_beats = region_length - relative_beat;
            if !beat.is_finite()
                || relative_beat >= region_length
                || (active && remaining_region_beats < 0.015625)
            {
                return;
            }
            let Some(pitch) = step_sequencer_lane_pitch(&ui, row) else {
                return;
            };
            let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
            if active {
                if !notes.iter().any(|note| {
                    note.pitch == pitch
                        && (note.start_beat - beat).abs() < 0.01
                        && step_note_belongs_to_region(&track, note, ui.get_sequencer_region_id())
                }) {
                    notes.push(ZNote {
                        region_id: ui.get_sequencer_region_id(),
                        midi_channel: 0,
                        pitch,
                        start_beat: beat,
                        length_beats: (step_beats * 0.8).max(0.015625).min(remaining_region_beats),
                        velocity: 100,
                        articulation: 0,
                        selected: false,
                        lyric: "".into(),
                        vibrato_amount: 0.0,
                        vibrato_rate_millihz: 5000,
                        phoneme: "".into(),
                        pitch_curve_cents: slint::ModelRc::default(),
                        portamento_samples: 0,
                        probability: 100,
                        repeat_count: 1,
                    });
                }
            } else {
                notes.retain(|note| {
                    !(note.pitch == pitch
                        && (note.start_beat - beat).abs() < 0.01
                        && step_note_belongs_to_region(&track, note, ui.get_sequencer_region_id()))
                });
            }
            sanitize_notes(&mut notes);
            for index in 0..tracks.row_count() {
                if tracks
                    .row_data(index)
                    .is_some_and(|candidate| candidate.id == track.id)
                {
                    let mut updated = track.clone();
                    updated.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                    replace_track(&tracks, index, updated);
                    if sync_midi_notes_to_core("", &tracks, &core) {
                        let current = (0..tracks.row_count()).find_map(|row| {
                            tracks.row_data(row).filter(|track| track.id == track_id)
                        });
                        if tracks
                            .row_data(ui.get_selected_row().max(0) as usize)
                            .is_some_and(|selected| selected.id == track_id)
                        {
                            refresh_step_sequencer_state(&ui, current.as_ref());
                        }
                        ui.set_last_action(
                            format!(
                                "STEP SEQUENCER: BAR {} STEP {} {}",
                                step / 16 + 1,
                                step % 16 + 1,
                                if active { "ON" } else { "OFF" }
                            )
                            .into(),
                        );
                    } else if let Some(ui) = weak.upgrade() {
                        ui.set_last_action("STEP SEQUENCER EDIT REJECTED BY CORE".into());
                    }
                    break;
                }
            }
        }
    });
    ui.global::<StepActions>().on_reverse_lane({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |track_id, lane| {
            if track_id < 0 || !(0..6).contains(&lane) {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_id = ui.get_sequencer_region_id();
            if region_id < 0 || ui.get_sequencer_region_track_id() != track_id {
                return;
            }
            let Some(row) = (0..tracks.row_count()).find(|&row| {
                tracks
                    .row_data(row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(track) = tracks.row_data(row) else {
                return;
            };
            if !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
                || !step_region_exists(&track, region_id)
            {
                return;
            }
            let region_start = ui.get_sequencer_region_start_beat();
            let region_length = ui.get_sequencer_region_length_beats();
            let step_beats = step_duration_beats(&ui);
            if !region_start.is_finite()
                || !region_length.is_finite()
                || region_length < 0.015625
                || !(region_start + region_length).is_finite()
            {
                return;
            }
            let pattern_step_count = (((region_length - 0.015625) / step_beats).floor() as usize)
                .saturating_add(1)
                .min(128);
            if pattern_step_count < 2 {
                return;
            }
            let Some(pitch) = step_sequencer_lane_pitch(&ui, lane) else {
                return;
            };
            let region_end = region_start + region_length;
            let mut notes = track.piano_roll_notes.iter().collect::<Vec<_>>();
            let mut changed = 0usize;
            for note in &mut notes {
                if note.pitch != pitch
                    || !step_note_belongs_to_region(&track, note, region_id)
                    || note.start_beat < region_start
                    || note.start_beat >= region_end
                {
                    continue;
                }
                let relative_beat = note.start_beat - region_start;
                let step = (relative_beat / step_beats).round() as usize;
                if step >= pattern_step_count
                    || (relative_beat - step as f32 * step_beats).abs() >= 0.01
                {
                    continue;
                }
                let reversed_step = pattern_step_count - 1 - step;
                let new_relative_beat = reversed_step as f32 * step_beats;
                let remaining_region_beats = region_length - new_relative_beat;
                if remaining_region_beats < 0.015625 {
                    continue;
                }
                let new_start_beat = region_start + new_relative_beat;
                let new_length_beats = note.length_beats.min(remaining_region_beats).max(0.015625);
                if (note.start_beat - new_start_beat).abs() < 0.00001
                    && note.region_id == region_id
                    && (note.length_beats - new_length_beats).abs() < 0.00001
                {
                    continue;
                }
                note.region_id = region_id;
                note.start_beat = new_start_beat;
                note.length_beats = new_length_beats;
                changed += 1;
            }
            if changed == 0 {
                ui.set_last_action(
                    format!("STEP LANE {} HAS NO REVERSIBLE GRID NOTES", lane + 1).into(),
                );
                return;
            }
            sanitize_notes(&mut notes);
            let previous = track.clone();
            let mut updated = track;
            updated.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
            replace_track(&tracks, row, updated.clone());
            if sync_midi_notes_to_core("", &tracks, &core) {
                refresh_step_sequencer_state(&ui, Some(&updated));
                ui.set_last_action(
                    format!("REVERSED STEP LANE {}: {changed} NOTES", lane + 1).into(),
                );
            } else {
                replace_track(&tracks, row, previous.clone());
                refresh_step_sequencer_state(&ui, Some(&previous));
                ui.set_last_action("STEP LANE REVERSE REJECTED BY CORE".into());
            }
        }
    });
    ui.global::<StepActions>().on_set_velocity({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |track_id, lane, step, velocity| {
            if track_id < 0
                || !(0..6).contains(&lane)
                || !(0..128).contains(&step)
                || !velocity.is_finite()
            {
                return;
            }
            let Some(row) = (0..tracks.row_count()).find(|&row| {
                tracks
                    .row_data(row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument") {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_start = ui.get_sequencer_region_start_beat();
            let region_length = ui.get_sequencer_region_length_beats();
            let step_beats = step_duration_beats(&ui);
            if ui.get_sequencer_region_id() < 0
                || ui.get_sequencer_region_track_id() != track_id
                || !region_start.is_finite()
                || !region_length.is_finite()
                || !(region_start + region_length).is_finite()
                || step as f32 * step_beats >= region_length
            {
                return;
            }
            let Some(pitch) = step_sequencer_lane_pitch(&ui, lane) else {
                return;
            };
            let beat = region_start + step as f32 * step_beats;
            let mut notes = track.piano_roll_notes.iter().collect::<Vec<_>>();
            let mut found = false;
            for note in &mut notes {
                if note.pitch == pitch
                    && (note.start_beat - beat).abs() < 0.01
                    && step_note_belongs_to_region(&track, note, ui.get_sequencer_region_id())
                {
                    note.region_id = ui.get_sequencer_region_id();
                    note.velocity = velocity.round().clamp(1.0, 127.0) as i32;
                    found = true;
                }
            }
            if !found {
                return;
            }
            track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
            replace_track(&tracks, row, track.clone());
            pending_note_edit.set(Some(track_id));
            if tracks
                .row_data(ui.get_selected_row().max(0) as usize)
                .is_some_and(|selected| selected.id == track_id)
            {
                refresh_step_sequencer_state(&ui, Some(&track));
            }
        }
    });
    ui.global::<StepActions>().on_set_probability({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |track_id, lane, step, probability| {
            if track_id < 0
                || !(0..6).contains(&lane)
                || !(0..128).contains(&step)
                || !probability.is_finite()
            {
                return;
            }
            let Some(row) = (0..tracks.row_count()).find(|&row| {
                tracks
                    .row_data(row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument") {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_start = ui.get_sequencer_region_start_beat();
            let region_length = ui.get_sequencer_region_length_beats();
            let step_beats = step_duration_beats(&ui);
            if ui.get_sequencer_region_id() < 0
                || ui.get_sequencer_region_track_id() != track_id
                || !region_start.is_finite()
                || !region_length.is_finite()
                || !(region_start + region_length).is_finite()
                || step as f32 * step_beats >= region_length
            {
                return;
            }
            let Some(pitch) = step_sequencer_lane_pitch(&ui, lane) else {
                return;
            };
            let beat = region_start + step as f32 * step_beats;
            let mut notes = track.piano_roll_notes.iter().collect::<Vec<_>>();
            let mut found = false;
            for note in &mut notes {
                if note.pitch == pitch
                    && (note.start_beat - beat).abs() < 0.01
                    && step_note_belongs_to_region(&track, note, ui.get_sequencer_region_id())
                {
                    note.region_id = ui.get_sequencer_region_id();
                    note.probability = probability.round().clamp(0.0, 100.0) as i32;
                    found = true;
                }
            }
            if !found {
                return;
            }
            track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
            replace_track(&tracks, row, track.clone());
            pending_note_edit.set(Some(track_id));
            if tracks
                .row_data(ui.get_selected_row().max(0) as usize)
                .is_some_and(|selected| selected.id == track_id)
            {
                refresh_step_sequencer_state(&ui, Some(&track));
            }
        }
    });
    ui.global::<StepActions>().on_set_gate_percent({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |track_id, lane, step, gate_percent| {
            if track_id < 0
                || !(0..6).contains(&lane)
                || !(0..128).contains(&step)
                || !gate_percent.is_finite()
            {
                return;
            }
            let Some(row) = (0..tracks.row_count()).find(|&row| {
                tracks
                    .row_data(row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if !matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument") {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let region_start = ui.get_sequencer_region_start_beat();
            let region_length = ui.get_sequencer_region_length_beats();
            let step_beats = step_duration_beats(&ui);
            let relative_beat = step as f32 * step_beats;
            let remaining_region_beats = region_length - relative_beat;
            if ui.get_sequencer_region_id() < 0
                || ui.get_sequencer_region_track_id() != track_id
                || !region_start.is_finite()
                || !region_length.is_finite()
                || !(region_start + region_length).is_finite()
                || !remaining_region_beats.is_finite()
                || remaining_region_beats < 0.015625
                || relative_beat >= region_length
            {
                return;
            }
            let Some(pitch) = step_sequencer_lane_pitch(&ui, lane) else {
                return;
            };
            let beat = region_start + relative_beat;
            if !beat.is_finite() {
                return;
            }
            let minimum_gate_percent = (1.5625 / step_beats).ceil().max(5.0);
            let length_beats = (step_beats * gate_percent.clamp(minimum_gate_percent, 400.0)
                / 100.0)
                .clamp(0.015625, remaining_region_beats);
            let mut notes = track.piano_roll_notes.iter().collect::<Vec<_>>();
            let mut found = false;
            for note in &mut notes {
                if note.pitch == pitch
                    && (note.start_beat - beat).abs() < 0.01
                    && step_note_belongs_to_region(&track, note, ui.get_sequencer_region_id())
                {
                    note.region_id = ui.get_sequencer_region_id();
                    note.length_beats = length_beats;
                    found = true;
                }
            }
            if !found {
                return;
            }
            track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
            replace_track(&tracks, row, track.clone());
            pending_note_edit.set(Some(track_id));
            if tracks
                .row_data(ui.get_selected_row().max(0) as usize)
                .is_some_and(|selected| selected.id == track_id)
            {
                refresh_step_sequencer_state(&ui, Some(&track));
            }
        }
    });
    ui.global::<MidiActions>().on_select_note({
        let tracks = tracks.clone();
        move |tid, pitch, beat, note_index| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    for (index, note) in notes.iter_mut().enumerate() {
                        note.selected = index == note_index.max(0) as usize
                            && note.pitch == pitch
                            && (note.start_beat - beat).abs() < 0.1;
                    }
                    track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_move_note({
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |tid, pitch, beat, dt, dp, note_index| {
            if note_index < 0 || !dt.is_finite() || !beat.is_finite() {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let Some(mut note) = track.piano_roll_notes.row_data(note_index as usize)
                    else {
                        return;
                    };
                    note.start_beat = (beat + dt).max(0.0);
                    note.pitch = (pitch + dp).clamp(0, 127);
                    track
                        .piano_roll_notes
                        .set_row_data(note_index as usize, note);
                    pending_note_edit.set(Some(tid));
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_resize_note({
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |tid, pitch, beat, length, note_index| {
            if note_index < 0 || !beat.is_finite() || !length.is_finite() {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let Some(mut note) = track.piano_roll_notes.row_data(note_index as usize)
                    else {
                        return;
                    };
                    if note.pitch != pitch || (note.start_beat - beat).abs() > 0.1 {
                        return;
                    }
                    note.length_beats = length.max(0.015625);
                    track
                        .piano_roll_notes
                        .set_row_data(note_index as usize, note);
                    pending_note_edit.set(Some(tid));
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_commit_note_edit({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |_tid| {
            let Some(tid) = pending_note_edit.take() else {
                return false;
            };
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                sanitize_notes(&mut notes);
                track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                replace_track(&tracks, row, track);
                let accepted = sync_midi_notes_to_core("", &tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    let authoritative = tracks.row_data(row);
                    refresh_step_sequencer_state(&ui, authoritative.as_ref());
                    if !accepted {
                        ui.set_last_action("MIDI NOTE EDIT REJECTED BY CORE".into());
                    }
                }
                return accepted;
            }
            false
        }
    });
    ui.global::<MidiActions>().on_add_note({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, pitch, beat, length, velocity| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    if !notes
                        .iter()
                        .any(|note| note.pitch == pitch && (note.start_beat - beat).abs() < 0.01)
                    {
                        notes.push(ZNote {
                            region_id: 0,
                            midi_channel: 0,
                            pitch: pitch.clamp(0, 127),
                            start_beat: beat.max(0.0),
                            length_beats: length.max(0.0625),
                            velocity: velocity.clamp(1, 127),
                            articulation: 0,
                            selected: false,
                            lyric: "".into(),
                            vibrato_amount: 0.0,
                            vibrato_rate_millihz: 5000,
                            phoneme: "".into(),
                            pitch_curve_cents: slint::ModelRc::default(),
                            portamento_samples: 0,
                            probability: 100,
                            repeat_count: 1,
                        });
                        sanitize_notes(&mut notes);
                        track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                        replace_track(&tracks, i, track);
                        sync_midi_notes_to_core("", &tracks, &core);
                    }
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_delete_note({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, pitch| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track
                        .piano_roll_notes
                        .iter()
                        .filter(|note| !(note.selected && note.pitch == pitch))
                        .collect();
                    sanitize_notes(&mut notes);
                    track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                    replace_track(&tracks, i, track);
                    sync_midi_notes_to_core("", &tracks, &core);
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_piano_roll_delete({
        let ui = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = ui.upgrade() {
                let selected = ui.get_sel_idx();
                if selected >= 0 && (selected as usize) < tracks.row_count() {
                    if let Some(mut track) = tracks.row_data(selected as usize) {
                        let notes: Vec<ZNote> = track
                            .piano_roll_notes
                            .iter()
                            .filter(|note| !note.selected)
                            .collect();
                        track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                        replace_track(&tracks, selected as usize, track);
                        sync_midi_notes_to_core("", &tracks, &core);
                    }
                }
            }
        }
    });
    ui.global::<MidiActions>().on_quantize_notes({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, grid| {
            let step = if grid.is_finite() {
                grid.max(0.0625)
            } else {
                return;
            };
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    for note in &mut notes {
                        note.start_beat = (note.start_beat / step).round() * step;
                    }
                    sanitize_notes(&mut notes);
                    track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                    replace_track(&tracks, i, track);
                    sync_midi_notes_to_core("", &tracks, &core);
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_ramp_selected_velocities({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                let mut selected: Vec<usize> = notes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, note)| note.selected.then_some(index))
                    .collect();
                selected.sort_by(|a, b| {
                    notes[*a]
                        .start_beat
                        .partial_cmp(&notes[*b].start_beat)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                if selected.len() < 2 {
                    break;
                }
                let first = notes[selected[0]].velocity as f32;
                let last = notes[*selected.last().unwrap()].velocity as f32;
                let denominator = (selected.len() - 1) as f32;
                for (position, index) in selected.into_iter().enumerate() {
                    let t = position as f32 / denominator;
                    notes[index].velocity =
                        (first + (last - first) * t).round().clamp(1.0, 127.0) as i32;
                }
                sanitize_notes(&mut notes);
                track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                replace_track(&tracks, i, track);
                sync_midi_notes_to_core("", &tracks, &core);
                break;
            }
        }
    });
    ui.global::<MidiActions>().on_insert_chord({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, root, beat, quality, length, inversion| {
            if !beat.is_finite()
                || !length.is_finite()
                || !(0.0625..=64.0).contains(&length)
                || !(0..=127).contains(&root)
                || !(0..=3).contains(&quality)
                || !(0..=2).contains(&inversion)
            {
                return;
            }
            let base_intervals: &[i32] = match quality {
                0 => &[0, 4, 7],     // major
                1 => &[0, 3, 7],     // minor
                2 => &[0, 4, 7, 10], // dominant seventh
                _ => &[0, 5, 7],     // suspended fourth
            };
            let mut intervals = base_intervals.to_vec();
            let inversion_count = (inversion as usize).min(intervals.len().saturating_sub(1));
            for _ in 0..inversion_count {
                let first = intervals.remove(0) + 12;
                intervals.push(first);
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                for interval in &intervals {
                    let pitch = root + *interval;
                    if pitch > 127 {
                        continue;
                    }
                    if !notes
                        .iter()
                        .any(|note| note.pitch == pitch && (note.start_beat - beat).abs() < 0.01)
                    {
                        notes.push(ZNote {
                            region_id: 0,
                            midi_channel: 0,
                            pitch,
                            start_beat: beat.max(0.0),
                            length_beats: length,
                            velocity: 100,
                            articulation: 0,
                            selected: false,
                            lyric: "".into(),
                            vibrato_amount: 0.0,
                            vibrato_rate_millihz: 5000,
                            phoneme: "".into(),
                            pitch_curve_cents: slint::ModelRc::default(),
                            portamento_samples: 0,
                            probability: 100,
                            repeat_count: 1,
                        });
                    }
                }
                sanitize_notes(&mut notes);
                track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                replace_track(&tracks, i, track);
                sync_midi_notes_to_core("", &tracks, &core);
                let chord_name = match quality {
                    0 => "major",
                    1 => "minor",
                    2 => "dominant7",
                    _ => "sus4",
                };
                let _ = core.add_chord_event(
                    (beat.max(0.0) * 480.0).round() as u64,
                    root as u8,
                    intervals
                        .iter()
                        .map(|interval| (*interval).clamp(0, 127) as u8)
                        .collect(),
                    chord_name,
                );
                break;
            }
        }
    });
    ui.global::<MidiActions>().on_insert_progression({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, root, beat, preset, length, inversion| {
            if !beat.is_finite()
                || !length.is_finite()
                || !(0.0625..=64.0).contains(&length)
                || !(0..=127).contains(&root)
                || !(0..=2).contains(&preset)
                || !(0..=3).contains(&inversion)
            {
                return;
            }
            let progressions: &[[i32; 4]] = &[
                [0, 7, 9, 5], // I-V-vi-IV
                [9, 5, 0, 7], // vi-IV-I-V
                [0, 9, 5, 7], // I-vi-IV-V
            ];
            let qualities: [[i32; 4]; 3] = [[0, 0, 1, 0], [1, 0, 0, 0], [0, 1, 0, 0]];
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                for chord_index in 0..4 {
                    let chord_root = root + progressions[preset as usize][chord_index];
                    let chord_quality = qualities[preset as usize][chord_index];
                    let base_intervals: &[i32] = if chord_quality == 0 {
                        &[0, 4, 7]
                    } else {
                        &[0, 3, 7]
                    };
                    let mut intervals = base_intervals.to_vec();
                    let inversion_count =
                        (inversion as usize).min(intervals.len().saturating_sub(1));
                    for _ in 0..inversion_count {
                        let first = intervals.remove(0) + 12;
                        intervals.push(first);
                    }
                    let chord_beat = beat.max(0.0) + chord_index as f32 * length;
                    for interval in &intervals {
                        let pitch = chord_root + *interval;
                        if pitch > 127 {
                            continue;
                        }
                        if !notes.iter().any(|note| {
                            note.pitch == pitch && (note.start_beat - chord_beat).abs() < 0.01
                        }) {
                            notes.push(ZNote {
                                region_id: 0,
                                midi_channel: 0,
                                pitch,
                                start_beat: chord_beat,
                                length_beats: length,
                                velocity: 100,
                                articulation: 0,
                                selected: false,
                                lyric: "".into(),
                                vibrato_amount: 0.0,
                                vibrato_rate_millihz: 5000,
                                phoneme: "".into(),
                                pitch_curve_cents: slint::ModelRc::default(),
                                portamento_samples: 0,
                                probability: 100,
                                repeat_count: 1,
                            });
                        }
                    }
                    if (0..=127).contains(&chord_root) {
                        let name = match chord_quality {
                            0 => "major",
                            _ => "minor",
                        };
                        let _ = core.add_chord_event(
                            (chord_beat.max(0.0) * 480.0).round() as u64,
                            chord_root as u8,
                            intervals
                                .iter()
                                .map(|interval| (*interval).clamp(0, 127) as u8)
                                .collect(),
                            name,
                        );
                    }
                }
                sanitize_notes(&mut notes);
                track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                replace_track(&tracks, i, track);
                sync_midi_notes_to_core("", &tracks, &core);
                break;
            }
        }
    });
    ui.global::<MidiActions>().on_set_note_velocity({
        let tracks = tracks.clone();
        let pending_note_edit = pending_note_edit.clone();
        move |tid, index, velocity| {
            if !velocity.is_finite() {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let note_index = index.max(0) as usize;
                    let Some(mut note) = track.piano_roll_notes.row_data(note_index) else {
                        continue;
                    };
                    note.velocity = velocity.round().clamp(1.0, 127.0) as i32;
                    track.piano_roll_notes.set_row_data(note_index, note);
                    pending_note_edit.set(Some(tid));
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_set_note_articulation({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, index, articulation| {
            if !(0..=255).contains(&articulation) || index < 0 {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let Some(mut note) = track.piano_roll_notes.row_data(index as usize) else {
                    return;
                };
                note.articulation = articulation;
                track.piano_roll_notes.set_row_data(index as usize, note);
                replace_track(&tracks, row, track);
                let accepted = sync_midi_notes_to_core("", &tracks, &core);
                if !accepted {
                    crate::ui::track_model::sync_midi_notes_from_core(&tracks, &core);
                }
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if accepted {
                        format!("MIDI ARTICULATION {}", articulation).into()
                    } else {
                        "MIDI ARTICULATION REJECTED".into()
                    });
                }
                return;
            }
        }
    });
    ui.global::<MidiActions>().on_set_articulation_map({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, articulation_id, pitch| {
            if !(1..=255).contains(&articulation_id) || !(-1..=127).contains(&pitch) {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let mut pitches = vec![-1_i32; 256];
                let mut channels = vec![0_u8; 256];
                for id in 0..256 {
                    if let Some(value) = track.articulation_switches.row_data(id) {
                        pitches[id] = value;
                    }
                    if let Some(value) = track.articulation_switch_channels.row_data(id) {
                        channels[id] = value.clamp(0, 15) as u8;
                    }
                }
                let Ok(mut entries) = serde_json::from_str::<
                    Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
                >(&track.expression_map_json) else {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action("EXPRESSION MAP DATA INVALID".into());
                    }
                    return;
                };
                let id = articulation_id as u8;
                if let Some(entry) = entries.iter_mut().find(|entry| entry.articulation_id == id) {
                    entry.outputs.retain(|output| !matches!(
                        output,
                        hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch { .. }
                    ));
                    if pitch >= 0 {
                        entry.outputs.insert(
                            0,
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch as u8,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        );
                        entry.keyswitch_pitch = Some(pitch as u8);
                    } else {
                        entry.keyswitch_pitch = None;
                    }
                } else if pitch >= 0 {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name: format!("Articulation {id}"),
                        keyswitch_pitch: Some(pitch as u8),
                        channel: channels[id as usize],
                        group: 0,
                        outputs: vec![
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch as u8,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        ],
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                let expression_map_json =
                    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_owned());
                if !core.set_midi_expression_map(tid as u32, entries, articulation_id as u8) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action("EXPRESSION MAP REJECTED".into());
                    }
                    return;
                }
                track.articulation_switches = slint::ModelRc::new(slint::VecModel::from(pitches));
                track.expression_map_json = expression_map_json.into();
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "ARTICULATION {} {}",
                            articulation_id,
                            if pitch < 0 {
                                "CLEARED".to_owned()
                            } else {
                                format!("KEYSWITCH {pitch}")
                            }
                        )
                        .into(),
                    );
                }
                return;
            }
        }
    });
    ui.global::<MidiActions>().on_set_articulation_map_output({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, articulation_id, kind, a, b, channel| {
            if !(1..=255).contains(&articulation_id)
                || !(0..=15).contains(&channel)
                || !matches!(kind, 0..=10)
                || (kind != 0 && kind != 5 && kind != 10 && !(0..=127).contains(&a))
                || (kind != 0
                    && kind != 3
                    && kind != 5
                    && kind != 8
                    && kind != 10
                    && !(0..=127).contains(&b)
                    && !((kind == 1 || kind == 6) && b == -1))
            {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let Ok(mut entries) = serde_json::from_str::<
                    Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
                >(&track.expression_map_json) else {
                    return;
                };
                let id = articulation_id as u8;
                let existing_index = entries.iter().position(|entry| entry.articulation_id == id);
                let off_phase = kind >= 6;
                let base_kind = if off_phase { kind - 5 } else { kind };
                if base_kind == 0 && existing_index.is_none() {
                    return;
                }
                let entry_index = if let Some(index) = existing_index {
                    index
                } else {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name: format!("Articulation {id}"),
                        keyswitch_pitch: None,
                        channel: channel as u8,
                        group: 0,
                        outputs: Vec::new(),
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                    entries.len() - 1
                };
                let entry = &mut entries[entry_index];
                entry.channel = channel as u8;
                use hirari_core_bridge::project_contracts::ExpressionMapOutput as Output;
                if base_kind == 5 {
                    entry
                        .outputs
                        .retain(|output| matches!(output, Output::KeySwitch { .. }));
                    entry.off_outputs.clear();
                }
                let outputs = if off_phase {
                    &mut entry.off_outputs
                } else {
                    &mut entry.outputs
                };
                outputs.retain(|output| match base_kind {
                    0 => true,
                    1 => !matches!(output, Output::ProgramChange { .. }),
                    2 => !matches!(output, Output::ControlChange { .. }),
                    3 => !matches!(output, Output::ChannelPressure { .. }),
                    4 => !matches!(output, Output::PitchBend { .. }),
                    5 => matches!(output, Output::KeySwitch { .. }),
                    _ => true,
                });
                let new_output = match base_kind {
                    0 => None,
                    1 => Some(Output::ProgramChange {
                        bank_msb: (b >= 0).then_some(b as u8),
                        bank_lsb: None,
                        program: a as u8,
                    }),
                    2 => Some(Output::ControlChange {
                        controller: a as u8,
                        value: b as u8,
                    }),
                    3 => Some(Output::ChannelPressure { value: a as u8 }),
                    4 => {
                        let bend = ((b * 128 + a) - 8192) as i16;
                        Some(Output::PitchBend { value: bend })
                    }
                    5 => None,
                    _ => return,
                };
                if let Some(output) = new_output {
                    outputs.push(output);
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                let expression_map_json =
                    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into());
                if !core.set_midi_expression_map(tid as u32, entries, articulation_id as u8) {
                    return;
                }

                let mut kinds = (0..256)
                    .map(|i| track.articulation_output_kinds.row_data(i).unwrap_or(0))
                    .collect::<Vec<_>>();
                let mut values_a = (0..256)
                    .map(|i| track.articulation_output_a.row_data(i).unwrap_or(0))
                    .collect::<Vec<_>>();
                let mut values_b = (0..256)
                    .map(|i| track.articulation_output_b.row_data(i).unwrap_or(0))
                    .collect::<Vec<_>>();
                let index = articulation_id as usize;
                kinds[index] = if base_kind == 0 || base_kind == 5 {
                    0
                } else {
                    kind
                };
                values_a[index] = if base_kind == 5 { 0 } else { a };
                values_b[index] = if base_kind == 5 { 0 } else { b };
                track.articulation_output_kinds = slint::ModelRc::new(slint::VecModel::from(kinds));
                track.articulation_output_a = slint::ModelRc::new(slint::VecModel::from(values_a));
                track.articulation_output_b = slint::ModelRc::new(slint::VecModel::from(values_b));
                let mut channels = (0..256)
                    .map(|i| track.articulation_switch_channels.row_data(i).unwrap_or(0))
                    .collect::<Vec<_>>();
                channels[articulation_id as usize] = channel;
                track.articulation_switch_channels =
                    slint::ModelRc::new(slint::VecModel::from(channels));
                track.expression_map_json = expression_map_json.into();
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!("ARTICULATION {} MIDI OUTPUT UPDATED", articulation_id).into(),
                    );
                }
                return;
            }
        }
    });
    ui.global::<MidiActions>().on_set_articulation_map_name({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, articulation_id, requested_name| {
            if !(1..=255).contains(&articulation_id)
                || requested_name.len() > 128
                || requested_name.contains('\0')
            {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != tid {
                    continue;
                }
                let Ok(mut entries) = serde_json::from_str::<
                    Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
                >(&track.expression_map_json) else {
                    return;
                };
                for entry in &mut entries {
                    if entry.name.trim().is_empty() {
                        entry.name = format!("Articulation {}", entry.articulation_id);
                    }
                }
                let id = articulation_id as u8;
                let index = if let Some(index) =
                    entries.iter().position(|entry| entry.articulation_id == id)
                {
                    index
                } else {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name: format!("Articulation {id}"),
                        keyswitch_pitch: None,
                        channel: 0,
                        group: 0,
                        outputs: Vec::new(),
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                    entries.len() - 1
                };
                let name = if requested_name.trim().is_empty() {
                    format!("Articulation {id}")
                } else {
                    requested_name.trim().to_owned()
                };
                entries[index].name = name.clone();
                entries.sort_by_key(|entry| entry.articulation_id);
                if !core.set_midi_expression_map(tid as u32, entries.clone(), id) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action("ARTICULATION NAME ALREADY USED OR REJECTED".into());
                    }
                    return;
                }
                let expression_map_json =
                    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_owned());
                let mut names = (0..256)
                    .map(|value| format!("Articulation {value}").into())
                    .collect::<Vec<slint::SharedString>>();
                names[0] = slint::SharedString::default();
                for entry in &entries {
                    names[entry.articulation_id as usize] = entry.name.clone().into();
                }
                track.articulation_names = slint::ModelRc::new(slint::VecModel::from(names));
                track.expression_map_json = expression_map_json.into();
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("ARTICULATION {id} NAMED {name}").into());
                }
                return;
            }
        }
    });
    ui.global::<MidiActions>().on_import_expression_map_pro({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid| {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Hirari Expression Map JSON", &["json"])
                .pick_file()
            else {
                return false;
            };
            let map_json = match std::fs::read_to_string(&path) {
                Ok(json) => json,
                Err(error) => {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(format!("EXPRESSION MAP READ FAILED: {error}").into());
                    }
                    return false;
                }
            };
            let map =
                match hirari_core_bridge::expression_map::ExpressionMapPro::from_json(&map_json) {
                    Ok(map) => map,
                    Err(error) => {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_last_action(format!("EXPRESSION MAP INVALID: {error}").into());
                        }
                        return false;
                    }
                };
            let map_name = map.name.clone();
            if !core.set_midi_expression_map_pro(tid as u32, map) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        "EXPRESSION MAP NOT SUPPORTED BY THE CURRENT ART-ID PLAYBACK PATH".into(),
                    );
                }
                return false;
            }
            crate::ui::track_model::sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!("EXPRESSION MAP {map_name} IMPORTED TO TRACK {tid}").into(),
                );
            }
            true
        }
    });
    ui.global::<MidiActions>().on_set_note_lyric({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, pitch, beat, lyric| {
            if !beat.is_finite() || lyric.len() > 1024 || lyric.contains('\0') {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    for note in &mut notes {
                        if note.pitch == pitch && (note.start_beat - beat).abs() < 0.1 {
                            note.lyric = lyric.clone();
                            break;
                        }
                    }
                    track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                    replace_track(&tracks, i, track);
                    sync_midi_notes_to_core("", &tracks, &core);
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_apply_swing({
        let ui = ui.as_weak();
        let core = core.clone();
        move |subdivision, amount| {
            if let Some(ui) = ui.upgrade() {
                let ok = core.apply_midi_swing(subdivision, amount);
                ui.set_last_action(if ok {
                    format!("MIDI SWING: {:.0}%", amount * 100.0).into()
                } else {
                    "MIDI SWING REJECTED".into()
                });
            }
        }
    });
    ui.global::<MidiActions>().on_humanize({
        let ui = ui.as_weak();
        let core = core.clone();
        move |timing, velocity, seed| {
            if let Some(ui) = ui.upgrade() {
                let ok = core.humanize_midi(timing, velocity as i16, seed.max(1) as u64);
                ui.set_last_action(if ok {
                    "MIDI HUMANIZE APPLIED".into()
                } else {
                    "MIDI HUMANIZE REJECTED".into()
                });
            }
        }
    });
    ui.global::<MidiActions>().on_set_expression_events({
        let ui = ui.as_weak();
        let core = core.clone();
        move |snapshot| {
            if let Some(ui) = ui.upgrade() {
                let ok = core.set_midi_events_json(snapshot.as_str());
                ui.set_last_action(if ok {
                    "MIDI EXPRESSION EVENTS UPDATED".into()
                } else {
                    "MIDI EXPRESSION EVENTS REJECTED".into()
                });
            }
        }
    });
}

pub(crate) fn refresh_step_sequencer_region(ui: &AppWindow, track: Option<&Z_Track>) {
    let same_project =
        ui.get_sequencer_patterns_project_path().as_str() == ui.get_project_path().as_str();
    if same_project {
        persist_active_step_sequencer_pattern(ui);
    }
    ensure_step_sequencer_patterns_loaded(ui);
    let selected_clip_id = ui.get_sel_cid();
    let selected_clip = track
        .filter(|track| matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument"))
        .and_then(|track| track.clips.iter().find(|clip| clip.id == selected_clip_id))
        .filter(|clip| {
            clip.id >= 0
                && clip.start_beat.is_finite()
                && clip.length_beats.is_finite()
                && clip.length_beats > 0.0
        });
    if let (Some(track), Some(clip)) = (track, selected_clip) {
        ui.set_sequencer_region_track_id(track.id);
        ui.set_sequencer_region_id(clip.id);
        ui.set_sequencer_region_name(clip.name.clone());
        ui.set_sequencer_region_start_beat(clip.start_beat.max(0.0));
        ui.set_sequencer_region_length_beats(clip.length_beats);
        apply_step_sequencer_pattern(ui, track.id, clip.id);
        let page_beats = step_duration_beats(ui) * 16.0;
        ui.set_sequencer_region_bars(((clip.length_beats / page_beats).ceil() as i32).clamp(1, 8));
    } else {
        ui.set_sequencer_region_track_id(-1);
        ui.set_sequencer_region_id(-1);
        ui.set_sequencer_region_name("".into());
        ui.set_sequencer_region_start_beat(0.0);
        ui.set_sequencer_region_length_beats(0.0);
        ui.set_sequencer_region_bars(0);
    }
    refresh_step_sequencer_state(ui, track);
}

fn refresh_step_sequencer_page_count(ui: &AppWindow) {
    let length = ui.get_sequencer_region_length_beats();
    if !length.is_finite() || length <= 0.0 {
        ui.set_sequencer_region_bars(0);
        return;
    }
    let page_beats = step_duration_beats(ui) * 16.0;
    ui.set_sequencer_region_bars(((length / page_beats).ceil() as i32).clamp(1, 8));
}

pub(crate) fn refresh_step_sequencer_state(ui: &AppWindow, track: Option<&Z_Track>) {
    let mut states = vec![false; 768];
    let mut velocities = vec![100; 768];
    let mut probabilities = vec![100; 768];
    let mut gates = vec![80; 768];
    let region_start = ui.get_sequencer_region_start_beat();
    let region_end = region_start + ui.get_sequencer_region_length_beats();
    let step_beats = step_duration_beats(ui);
    let mut unmapped_note_count = 0_i32;
    let lane_pitches = active_lane_pitches(ui).unwrap_or([36, 38, 42, 46, 40, 45]);
    if let Some(track) = track.filter(|track| {
        track.id == ui.get_sequencer_region_track_id()
            && ui.get_sequencer_region_id() >= 0
            && matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
            && region_start.is_finite()
            && region_end.is_finite()
    }) {
        for note in track.piano_roll_notes.iter() {
            if !step_note_belongs_to_region(track, &note, ui.get_sequencer_region_id()) {
                continue;
            }
            let Some(lane) = lane_pitches
                .iter()
                .position(|pitch| i32::from(*pitch) == note.pitch)
            else {
                unmapped_note_count = unmapped_note_count.saturating_add(1);
                continue;
            };
            if !note.start_beat.is_finite()
                || note.start_beat < region_start
                || note.start_beat >= region_end
            {
                unmapped_note_count = unmapped_note_count.saturating_add(1);
                continue;
            }
            let relative_beat = note.start_beat - region_start;
            let step = (relative_beat / step_beats).round() as usize;
            if step >= 128 || (relative_beat - step as f32 * step_beats).abs() >= 0.01 {
                unmapped_note_count = unmapped_note_count.saturating_add(1);
                continue;
            }
            let index = lane * 128 + step;
            if states[index] {
                unmapped_note_count = unmapped_note_count.saturating_add(1);
                continue;
            }
            velocities[index] = note.velocity.clamp(1, 127);
            probabilities[index] = note.probability.clamp(0, 100);
            gates[index] = (note.length_beats.max(0.015625) / step_beats * 100.0)
                .round()
                .clamp(5.0, 400.0) as i32;
            states[index] = true;
        }
    }
    ui.set_sequencer_step_states(slint::ModelRc::new(VecModel::from(states)));
    ui.set_sequencer_step_velocities(slint::ModelRc::new(VecModel::from(velocities)));
    ui.set_sequencer_step_probabilities(slint::ModelRc::new(VecModel::from(probabilities)));
    ui.set_sequencer_step_gates(slint::ModelRc::new(VecModel::from(gates)));
    ui.set_sequencer_unmapped_note_count(unmapped_note_count);
}

fn step_note_belongs_to_region(track: &Z_Track, note: &ZNote, region_id: i32) -> bool {
    if !step_region_exists(track, region_id)
        || !note.start_beat.is_finite()
        || !note.length_beats.is_finite()
    {
        return false;
    }
    if note.region_id == region_id {
        return true;
    }
    if note.region_id != 0 {
        return false;
    }

    // Old projects did not record note ownership. Associate a legacy note
    // only when exactly one clip fully contains it; overlapping clips remain
    // ambiguous and cannot be edited through the Step Sequencer.
    let note_end = note.start_beat + note.length_beats.max(0.015625);
    if !note_end.is_finite() {
        return false;
    }
    let mut owner = None;
    for clip in track.clips.iter() {
        let clip_end = clip.start_beat + clip.length_beats;
        if clip.id > 0
            && clip.start_beat.is_finite()
            && clip.length_beats.is_finite()
            && clip.length_beats > 0.0
            && note.start_beat >= clip.start_beat
            && note_end <= clip_end + 0.0001
        {
            if owner.replace(clip.id).is_some() {
                return false;
            }
        }
    }
    owner == Some(region_id)
}

fn step_region_exists(track: &Z_Track, region_id: i32) -> bool {
    region_id > 0
        && track.clips.iter().any(|clip| {
            clip.id == region_id
                && clip.start_beat.is_finite()
                && clip.length_beats.is_finite()
                && clip.length_beats > 0.0
        })
}

#[cfg(test)]
#[path = "midi_tests.rs"]
mod midi_tests;
