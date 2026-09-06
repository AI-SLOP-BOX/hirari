use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{
    sync_midi_notes_to_core, AppWindow, MidiActions, StepActions, ZNote, Z_Track,
};
use crate::ui::sync::replace_track;

fn sanitize_notes(notes: &mut Vec<ZNote>) {
    notes.retain(|note| note.start_beat.is_finite() && note.length_beats.is_finite());
    for note in notes.iter_mut() {
        note.pitch = note.pitch.clamp(0, 127);
        note.start_beat = note.start_beat.max(0.0);
        note.length_beats = note.length_beats.max(0.0625);
        note.velocity = note.velocity.clamp(1, 127);
    }
    notes.sort_by(|a, b| {
        a.start_beat
            .partial_cmp(&b.start_beat)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    ui.global::<StepActions>().on_step_toggled({
        let tracks = tracks.clone();
        let core = core.clone();
        move |track_id, row, step, active| {
            if track_id < 0 || !(0..6).contains(&row) || !(0..16).contains(&step) {
                return;
            }
            let Some(track) = (0..tracks.row_count())
                .find_map(|index| tracks.row_data(index).filter(|track| track.id == track_id))
            else {
                return;
            };
            let pitch = [36, 38, 42, 46, 40, 45]
                .get(row as usize)
                .copied()
                .unwrap_or(36);
            let beat = (step.max(0) as f32) * 0.25;
            let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
            if active {
                if !notes
                    .iter()
                    .any(|note| note.pitch == pitch && (note.start_beat - beat).abs() < 0.01)
                {
                    notes.push(ZNote {
                        pitch,
                        start_beat: beat,
                        length_beats: 0.2,
                        velocity: 100,
                        articulation: 0,
                        selected: false,
                        lyric: "".into(),
                        vibrato_amount: 0.0,
                        vibrato_rate_millihz: 5000,
                    });
                }
            } else {
                notes
                    .retain(|note| !(note.pitch == pitch && (note.start_beat - beat).abs() < 0.01));
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
                    sync_midi_notes_to_core("", &tracks, &core);
                    break;
                }
            }
        }
    });
    ui.global::<MidiActions>().on_select_note({
        let tracks = tracks.clone();
        move |tid, pitch, beat| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    for note in &mut notes {
                        note.selected = note.pitch == pitch && (note.start_beat - beat).abs() < 0.1;
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
        let core = core.clone();
        move |tid, pitch, beat, dt, dp| {
            if !dt.is_finite() || !beat.is_finite() {
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
                            note.start_beat = (note.start_beat + dt).max(0.0);
                            note.pitch = (note.pitch + dp).clamp(0, 127);
                        }
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
    ui.global::<MidiActions>().on_resize_note({
        let tracks = tracks.clone();
        let core = core.clone();
        move |tid, pitch, beat, length| {
            if !beat.is_finite() || !length.is_finite() {
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
                            note.length_beats = length.max(0.0625);
                        }
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
                            pitch: pitch.clamp(0, 127),
                            start_beat: beat.max(0.0),
                            length_beats: length.max(0.0625),
                            velocity: velocity.clamp(1, 127),
                            articulation: 0,
                            selected: false,
                            lyric: "".into(),
                            vibrato_amount: 0.0,
                            vibrato_rate_millihz: 5000,
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
                            pitch,
                            start_beat: beat.max(0.0),
                            length_beats: length,
                            velocity: 100,
                            articulation: 0,
                            selected: false,
                            lyric: "".into(),
                            vibrato_amount: 0.0,
                            vibrato_rate_millihz: 5000,
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
                                pitch,
                                start_beat: chord_beat,
                                length_beats: length,
                                velocity: 100,
                                articulation: 0,
                                selected: false,
                                lyric: "".into(),
                                vibrato_amount: 0.0,
                                vibrato_rate_millihz: 5000,
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
        let core = core.clone();
        move |tid, index, velocity| {
            if !velocity.is_finite() {
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == tid {
                    let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                    if let Some(note) = notes.get_mut(index.max(0) as usize) {
                        note.velocity = velocity.round().clamp(1.0, 127.0) as i32;
                    } else {
                        continue;
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

#[cfg(test)]
mod tests {
    use super::sanitize_notes;
    use crate::slint_ui::ZNote;

    #[test]
    fn sanitize_notes_removes_invalid_positions_and_orders_notes() {
        let mut notes = vec![
            ZNote {
                pitch: 140,
                start_beat: 4.0,
                length_beats: 0.0,
                velocity: 200,
                articulation: 0,
                selected: false,
                lyric: "".into(),
            },
            ZNote {
                pitch: 60,
                start_beat: -2.0,
                length_beats: 1.0,
                velocity: 100,
                articulation: 0,
                selected: false,
                lyric: "".into(),
            },
            ZNote {
                pitch: 64,
                start_beat: f32::NAN,
                length_beats: 1.0,
                velocity: 100,
                articulation: 0,
                selected: false,
                lyric: "".into(),
            },
        ];

        sanitize_notes(&mut notes);

        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].start_beat, 0.0);
        assert_eq!(notes[0].pitch, 60);
        assert_eq!(notes[0].length_beats, 1.0);
        assert_eq!(notes[1].pitch, 127);
        assert_eq!(notes[1].length_beats, 0.0625);
        assert_eq!(notes[1].velocity, 127);
    }

    #[test]
    fn sanitize_notes_preserves_valid_selection_and_articulation() {
        let mut notes = vec![ZNote {
            pitch: 36,
            start_beat: 2.0,
            length_beats: 0.5,
            velocity: 80,
            articulation: 3,
            selected: true,
            lyric: "ka".into(),
        }];

        sanitize_notes(&mut notes);

        assert_eq!(notes[0].articulation, 3);
        assert!(notes[0].selected);
    }
}
