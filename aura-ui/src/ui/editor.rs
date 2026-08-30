use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{
    sync_tracks_from_engine, ui_error_message, AppWindow, EditorActions, UiErrorKind, Z_Track,
};

fn region_targets(tracks: &VecModel<Z_Track>, requested_id: i32) -> Vec<(u32, u32)> {
    let requested_is_selected = (0..tracks.row_count()).any(|row| {
        tracks.row_data(row).is_some_and(|track| {
            track
                .clips
                .iter()
                .any(|clip| clip.id == requested_id && clip.selected)
        })
    });
    let mut targets = Vec::new();
    for row in 0..tracks.row_count() {
        let Some(track) = tracks.row_data(row) else {
            continue;
        };
        for clip in track.clips.iter() {
            if (requested_is_selected && clip.selected)
                || (!requested_is_selected && clip.id == requested_id)
            {
                targets.push((track.id as u32, clip.id as u32));
            }
        }
    }
    targets
}

/// Non-destructive region editing callbacks. UI models are updated only after
/// the corresponding Core mutation is accepted.
pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    let clipboard: Rc<RefCell<Vec<(u32, u32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    ui.global::<EditorActions>().on_select_all_clips({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move || {
            let mut first = None;
            let mut count = 0u32;
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                let mut clips: Vec<_> = track.clips.iter().collect();
                for clip in &mut clips {
                    if first.is_none() {
                        first = Some((row as i32, clip.id));
                    }
                    clip.selected = true;
                    count = count.saturating_add(1);
                }
                track.clips = slint::ModelRc::new(VecModel::from(clips));
                tracks.set_row_data(row, track);
            }
            if let Some(ui) = weak.upgrade() {
                if let Some((row, clip_id)) = first {
                    ui.set_sel_idx(row);
                    ui.set_sel_cid(clip_id);
                }
                ui.set_last_action(
                    format!(
                        "SELECTED {} CLIP{}",
                        count,
                        if count == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_clear_clip_selection({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move || {
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                let mut clips: Vec<_> = track.clips.iter().collect();
                for clip in &mut clips {
                    clip.selected = false;
                }
                track.clips = slint::ModelRc::new(VecModel::from(clips));
                tracks.set_row_data(row, track);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_cid(-1);
                ui.set_last_action("SELECTION CLEARED".into());
            }
        }
    });
    ui.global::<EditorActions>().on_copy_selected_clips({
        let weak = weak.clone();
        let tracks = tracks.clone();
        let clipboard = clipboard.clone();
        move || {
            let mut selected = Vec::new();
            for row in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(row) else {
                    continue;
                };
                for clip in track.clips.iter().filter(|clip| clip.selected) {
                    selected.push((track.id as u32, clip.id as u32, clip.start_beat));
                }
            }
            if selected.is_empty() {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("COPY IGNORED: NO CLIPS SELECTED".into());
                }
                return;
            }
            let origin = selected
                .iter()
                .map(|(_, _, start)| *start)
                .fold(f32::INFINITY, f32::min);
            *clipboard.borrow_mut() = selected
                .into_iter()
                .map(|(track, clip, start)| (track, clip, start - origin))
                .collect();
            if let Some(ui) = weak.upgrade() {
                let count = clipboard.borrow().len();
                ui.set_last_action(
                    format!("COPIED {} CLIP{}", count, if count == 1 { "" } else { "S" }).into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_paste_clips({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let clipboard = clipboard.clone();
        move |beat| {
            if !beat.is_finite() || beat < 0.0 {
                return;
            }
            let items = clipboard.borrow().clone();
            if items.is_empty() {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PASTE IGNORED: CLIPBOARD EMPTY".into());
                }
                return;
            }
            core.begin_undo_transaction("Paste Clips");
            let mut created = Vec::new();
            for (track_id, source_id, offset) in items {
                let Some(destination) = (beat + offset).is_finite().then_some(beat + offset) else {
                    let _ = core.abort_undo_transaction();
                    return;
                };
                let new_id = core.duplicate_region(track_id, source_id, destination as f64);
                if new_id == 0 {
                    let _ = core.abort_undo_transaction();
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                "paste rejected: source clip unavailable",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                created.push(new_id);
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                return;
            }
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_cid(created.last().copied().unwrap_or_default() as i32);
                ui.set_last_action(
                    format!(
                        "PASTED {} CLIP{}",
                        created.len(),
                        if created.len() == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_clip_moved({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |cid, delta| {
            let Some(ui) = weak.upgrade() else { return };
            let snap = match ui.get_snap_val().as_str() {
                "1/1" => 1.0,
                "1/2" => 2.0,
                "1/4" => 4.0,
                "1/8" => 8.0,
                "1/16" => 16.0,
                "1/32" => 32.0,
                _ => 16.0,
            };
            if !delta.is_finite() {
                return;
            }
            let targets = region_targets(&tracks, cid);
            if targets.is_empty() {
                return;
            }
            let step = 4.0 / snap;
            let mut moves = Vec::with_capacity(targets.len());
            for (track_id, target_id) in &targets {
                let Some(track) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id as u32 == *track_id)
                else {
                    return;
                };
                let Some(clip) = track.clips.iter().find(|clip| clip.id as u32 == *target_id)
                else {
                    return;
                };
                let next = ((clip.start_beat + delta).max(0.0) / step).round() * step;
                if !next.is_finite() {
                    return;
                }
                moves.push((*track_id, *target_id, next as f64));
            }
            core.begin_undo_transaction("Move Selected Clips");
            for (track_id, target_id, next) in &moves {
                if !core.move_region(*track_id, *target_id, *next) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                ui.set_sel_cid(cid);
                ui.set_last_action(
                    format!(
                        "MOVED {} CLIP{}",
                        moves.len(),
                        if moves.len() == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_split_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, beat| {
            if !beat.is_finite() || beat <= 0.0 {
                return;
            }
            let selected = region_targets(&tracks, id);
            if selected.is_empty() {
                return;
            }
            let mut targets = Vec::new();
            for (track_id, clip_id) in selected {
                let Some(track) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id as u32 == track_id)
                else {
                    continue;
                };
                let Some(clip) = track.clips.iter().find(|clip| clip.id as u32 == clip_id) else {
                    continue;
                };
                let right = clip.start_beat + clip.length_beats.max(0.0);
                if beat > clip.start_beat && beat < right {
                    targets.push((track_id, clip_id));
                }
            }
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Split Selected Clips");
            for (track_id, clip_id) in &targets {
                if !core.split_region(*track_id, *clip_id, beat as f64) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "SPLIT {} CLIP{} @ {:.2} BEAT",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" },
                            beat
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_split_clip_with_crossfade({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, beat, ratio| {
            if !beat.is_finite() || beat <= 0.0 || !ratio.is_finite() {
                return;
            }
            let selected = region_targets(&tracks, id);
            let targets = selected
                .into_iter()
                .filter(|(track_id, clip_id)| {
                    (0..tracks.row_count())
                        .filter_map(|row| tracks.row_data(row))
                        .any(|track| {
                            track.id as u32 == *track_id
                                && track.clips.iter().any(|clip| clip.id as u32 == *clip_id)
                        })
                })
                .collect::<Vec<_>>();
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Split Clips With Crossfade");
            for (track_id, clip_id) in &targets {
                if !core.split_region_with_auto_crossfade(
                    *track_id,
                    *clip_id,
                    beat as f64,
                    ratio.clamp(0.0, 1.0),
                ) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "SPLIT + CROSSFADE {} CLIP{}",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" }
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_split_clip_at_silence({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, threshold, min_length| {
            if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) || min_length < 1 {
                return;
            }
            let targets = region_targets(&tracks, id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Split Regions At Silence");
            let mut total = 0u32;
            for (track_id, clip_id) in targets {
                let waveform = core.get_region_waveform(track_id, clip_id);
                total = total.saturating_add(core.split_region_at_silence(
                    track_id,
                    clip_id,
                    &waveform,
                    threshold,
                    min_length as usize,
                ));
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("SPLIT AT SILENCE: {} BOUNDARIES", total).into());
                }
            }
        }
    });
    ui.global::<EditorActions>().on_clip_gain_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, gain| {
            let bounded = gain.clamp(0.0, 2.0);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Set Selected Clip Gain");
            for (track_id, target_id) in &targets {
                if !core.set_region_gain(*track_id, *target_id, bounded) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
            }
        }
    });
    ui.global::<EditorActions>().on_apply_gain_staging({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, gain_db| {
            if track_id < 0 || !gain_db.is_finite() {
                return;
            }
            let track_id = track_id as u32;
            let mut peak = 0.0_f32;
            if (gain_db + 6.0).abs() < f32::EPSILON {
                for row in 0..tracks.row_count() {
                    let Some(track) = tracks.row_data(row) else {
                        continue;
                    };
                    if track.id as u32 != track_id {
                        continue;
                    }
                    for clip in track.clips.iter() {
                        peak = core
                            .get_region_waveform(track_id, clip.id as u32)
                            .into_iter()
                            .filter(|sample| sample.is_finite())
                            .map(|sample| sample.abs())
                            .fold(peak, f32::max);
                    }
                }
            }
            let applied_gain_db = if peak > 1.0e-9 {
                (-6.0 - 20.0 * peak.log10()).clamp(-24.0, 24.0)
            } else {
                gain_db
            };
            core.begin_undo_transaction("Automatic Gain Staging");
            let result = core.apply_gain_staging_diagnostic_json(track_id, applied_gain_db);
            let accepted = serde_json::from_str::<serde_json::Value>(&result)
                .ok()
                .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if accepted {
                    format!("AUTO GAIN STAGING: {applied_gain_db:.1} dB").into()
                } else {
                    "AUTO GAIN STAGING REJECTED".into()
                });
                if accepted {
                    sync_tracks_from_engine(&tracks, &core);
                }
            }
        }
    });
    ui.global::<EditorActions>().on_fade_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, fade_in_delta, fade_out_delta| {
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            let mut values = Vec::with_capacity(targets.len());
            for (track_id, target_id) in &targets {
                let Some(track) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id as u32 == *track_id)
                else {
                    return;
                };
                let Some(clip) = track.clips.iter().find(|clip| clip.id as u32 == *target_id)
                else {
                    return;
                };
                values.push((
                    (clip.fade_in + fade_in_delta).clamp(0.0, 1.0),
                    (clip.fade_out + fade_out_delta).clamp(0.0, 1.0),
                ));
            }
            core.begin_undo_transaction("Set Selected Clip Fades");
            for ((track_id, target_id), (next_in, next_out)) in targets.iter().zip(values) {
                if !core.set_region_fades(*track_id, *target_id, next_in, next_out) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
            }
        }
    });
    ui.global::<EditorActions>().on_reverse_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, reverse| {
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Reverse Selected Clips");
            for (track_id, target_id) in &targets {
                if !core.set_region_reverse(*track_id, *target_id, reverse) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_reversed(reverse);
                    ui.set_last_action(
                        format!(
                            "REVERSED {} CLIP{}",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" }
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_trim_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, start, end| {
            let start = start.clamp(0.0, 1.0);
            let end = end.clamp(0.0, 1.0);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() || start >= end {
                return;
            }
            core.begin_undo_transaction("Trim Selected Clips");
            for (track_id, target_id) in &targets {
                if !core.set_region_trim(*track_id, *target_id, start, end) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_trim_start(start);
                    ui.set_selected_clip_trim_end(end);
                    ui.set_last_action(
                        format!(
                            "TRIMMED {} CLIP{}",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" }
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_warp_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, ratio| {
            if !ratio.is_finite() {
                return;
            }
            let ratio = ratio.clamp(0.25, 4.0);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Warp Selected Clips");
            for (track_id, target_id) in &targets {
                if !core.set_region_warp_ratio(*track_id, *target_id, ratio as f64) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_warp_ratio(ratio);
                    ui.set_last_action(
                        format!(
                            "WARPED {} CLIP{}: {:.2}x",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" },
                            ratio
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_pitch_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, semitones| {
            if !semitones.is_finite() {
                return;
            }
            let value = semitones.clamp(-24.0, 24.0);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Pitch Selected Clips");
            for (track_id, target_id) in &targets {
                if !core.set_region_pitch_semitones(*track_id, *target_id, value) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_pitch_semitones(value);
                    ui.set_last_action(
                        format!(
                            "PITCHED {} CLIP{}: {:+.1} st",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" },
                            value
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_loop_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, count| {
            let count = count.clamp(1, 1024);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Loop Selected Clips");
            for (track_id, target_id) in &targets {
                if !core.set_region_loop_count(*track_id, *target_id, count as u32) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_loop_count(count);
                    ui.set_last_action(
                        format!(
                            "LOOPED {} CLIP{}: {}x",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" },
                            count
                        )
                        .into(),
                    );
                }
            }
        }
    });
    ui.global::<EditorActions>().on_duplicate_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id| {
            let clicked_is_selected = (0..tracks.row_count()).any(|row| {
                tracks.row_data(row).is_some_and(|track| {
                    track
                        .clips
                        .iter()
                        .any(|clip| clip.id == clip_id && clip.selected)
                })
            });
            let mut targets = Vec::new();
            for row in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(row) else {
                    continue;
                };
                for clip in track.clips.iter() {
                    if (clicked_is_selected && clip.selected)
                        || (!clicked_is_selected && clip.id == clip_id)
                    {
                        let destination = clip.start_beat + clip.length_beats;
                        if destination.is_finite() && destination >= 0.0 {
                            targets.push((track.id as u32, clip.id as u32, destination as f64));
                        }
                    }
                }
            }
            if targets.is_empty() {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "no valid clip selected").into(),
                    );
                }
                return;
            }
            core.begin_undo_transaction("Duplicate Selected Clips");
            let mut created = Vec::new();
            for (track_id, source_id, destination) in targets {
                let new_id = core.duplicate_region(track_id, source_id, destination);
                if new_id == 0 {
                    let _ = core.abort_undo_transaction();
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                "clip duplicate rejected by Core",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                created.push(new_id);
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "clip duplicate transaction could not commit",
                        )
                        .into(),
                    );
                }
                return;
            }
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_cid(created.last().copied().unwrap_or_default() as i32);
                ui.set_last_action(
                    format!(
                        "DUPLICATED {} CLIP{}",
                        created.len(),
                        if created.len() == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_remove_clip({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id| {
            let clicked_is_selected = (0..tracks.row_count()).any(|row| {
                tracks.row_data(row).is_some_and(|track| {
                    track
                        .clips
                        .iter()
                        .any(|clip| clip.id == clip_id && clip.selected)
                })
            });
            let mut targets = Vec::new();
            for row in 0..tracks.row_count() {
                let Some(track) = tracks.row_data(row) else {
                    continue;
                };
                for clip in track.clips.iter() {
                    if (clicked_is_selected && clip.selected)
                        || (!clicked_is_selected && clip.id == clip_id)
                    {
                        targets.push((track.id as u32, clip.id as u32));
                    }
                }
            }
            if targets.is_empty() {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Project, "no clip selected").into(),
                    );
                }
                return;
            }
            core.begin_undo_transaction("Remove Selected Clips");
            for (track_id, selected_id) in &targets {
                if !core.remove_region(*track_id, *selected_id) {
                    let _ = core.abort_undo_transaction();
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(UiErrorKind::Project, "clip removal rejected by Core")
                                .into(),
                        );
                    }
                    return;
                }
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "clip removal transaction could not commit",
                        )
                        .into(),
                    );
                }
                return;
            }
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_cid(-1);
                ui.set_last_action(
                    format!(
                        "REMOVED {} CLIP{}",
                        targets.len(),
                        if targets.len() == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
}
