use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::slint_ui::{
    sync_midi_notes_to_core, sync_tracks_from_engine, ui_error_message, AppWindow, EditorActions,
    UiErrorKind, ZNote, Z_Track,
};
use crate::ui::midi::copy_step_sequencer_pattern;

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

fn poll_audio_pitch_analysis(
    weak: slint::Weak<AppWindow>,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    track_id: u32,
    region_id: u32,
) {
    slint::Timer::single_shot(std::time::Duration::from_millis(80), move || {
        let Some(ui) = weak.upgrade() else { return };
        match core.region_audio_note_analysis_status(track_id, region_id) {
            1 => poll_audio_pitch_analysis(weak, core, tracks, track_id, region_id),
            2 => {
                if core.finalize_region_audio_note_analysis(track_id, region_id) {
                    sync_tracks_from_engine(&tracks, &core);
                    let segment_count = (0..tracks.row_count())
                        .filter_map(|row| tracks.row_data(row))
                        .find(|track| track.id == track_id as i32)
                        .and_then(|track| {
                            track
                                .clips
                                .iter()
                                .find(|clip| clip.id == region_id as i32)
                                .map(|clip| clip.audio_pitch_segments.row_count())
                        })
                        .unwrap_or(0);
                    ui.set_last_action(
                        format!("PITCH ANALYSIS COMPLETE · {segment_count} SEGMENTS").into(),
                    );
                } else {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "pitch analysis result could not be applied to the current project",
                        )
                        .into(),
                    );
                }
            }
            3 => {
                sync_tracks_from_engine(&tracks, &core);
                ui.set_last_action("PITCH ANALYSIS COMPLETE".into());
            }
            _ => ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Project,
                    "pitch analysis failed or the region changed before completion",
                )
                .into(),
            ),
        }
    });
}

fn poll_group_quantize(
    weak: slint::Weak<AppWindow>,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    track_id: u32,
    region_id: u32,
) {
    slint::Timer::single_shot(std::time::Duration::from_millis(60), move || {
        let Some(ui) = weak.upgrade() else { return };
        match core.region_quantize_status(track_id, region_id) {
            1 => poll_group_quantize(weak, core, tracks, track_id, region_id),
            2 => {
                if core.finalize_region_quantize(track_id, region_id) {
                    sync_tracks_from_engine(&tracks, &core);
                    ui.set_last_action(
                        "AUDIO GROUP QUANTIZED · SHARED WARP MAP · UNDO AVAILABLE".into(),
                    );
                } else {
                    if core.region_quantize_status(track_id, region_id) == 2 {
                        poll_group_quantize(weak, core, tracks, track_id, region_id);
                    } else {
                        ui.set_last_action(ui_error_message(
                            UiErrorKind::Project,
                            "quantize result expired because the project changed during analysis",
                        ).into());
                    }
                }
            }
            _ => ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Project,
                    "quantize failed: no transient map could be built for these regions",
                )
                .into(),
            ),
        }
    });
}

fn poll_region_alignment(
    weak: slint::Weak<AppWindow>,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    reference_track: u32,
    reference_region: u32,
    target_count: usize,
) {
    slint::Timer::single_shot(std::time::Duration::from_millis(60), move || {
        let Some(ui) = weak.upgrade() else { return };
        match core.region_alignment_status(reference_track, reference_region) {
            1 => poll_region_alignment(
                weak,
                core,
                tracks,
                reference_track,
                reference_region,
                target_count,
            ),
            2 => {
                if core.finalize_region_alignment(reference_track, reference_region) {
                    sync_tracks_from_engine(&tracks, &core);
                    ui.set_last_action(
                        format!("{target_count} AUDIO REGIONS ALIGNED TO REFERENCE · ONE UNDO")
                            .into(),
                    );
                } else if core.region_alignment_status(reference_track, reference_region) == 2 {
                    // A user edit transaction is still open. Core leaves the
                    // ready result intact and this poll retries afterward.
                    poll_region_alignment(
                        weak,
                        core,
                        tracks,
                        reference_track,
                        reference_region,
                        target_count,
                    );
                } else {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "alignment result expired because the project changed during analysis",
                        )
                        .into(),
                    );
                }
            }
            _ => ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Project,
                    "audio alignment failed: select two unlocked, matching-length audio regions",
                )
                .into(),
            ),
        }
    });
}

/// Non-destructive region editing callbacks. UI models are updated only after
/// the corresponding Core mutation is accepted.
pub fn install(ui: &AppWindow, core: Rc<HirariCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    let clipboard: Rc<RefCell<Vec<(u32, u32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    ui.global::<EditorActions>().on_select_clip({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move |track_id, clip_id| {
            let Some(selected_row) = (0..tracks.row_count()).find(|row| {
                tracks.row_data(*row).is_some_and(|track| {
                    track.id == track_id && track.clips.iter().any(|clip| clip.id == clip_id)
                })
            }) else {
                return;
            };
            let mut selected_clip = None;
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                let is_target_track = track.id == track_id;
                let mut clips = track.clips.iter().collect::<Vec<_>>();
                for clip in &mut clips {
                    clip.selected = is_target_track && clip.id == clip_id;
                    if clip.selected {
                        selected_clip = Some(clip.clone());
                    }
                }
                track.clips = slint::ModelRc::new(VecModel::from(clips));
                tracks.set_row_data(row, track);
            }
            if let (Some(ui), Some(clip)) = (weak.upgrade(), selected_clip) {
                ui.set_sel_idx(selected_row as i32);
                ui.set_sel_cid(clip_id);
                ui.set_selected_clip_reversed(clip.reverse);
                ui.set_selected_clip_trim_start(clip.trim_start);
                ui.set_selected_clip_trim_end(clip.trim_end);
                ui.set_selected_clip_warp_ratio(clip.warp_ratio);
                ui.set_selected_clip_pitch_preserve_warp(clip.pitch_preserve_warp);
                ui.set_selected_clip_pitch_semitones(clip.pitch_semitones);
                ui.set_selected_clip_gain(clip.gain);
                ui.set_selected_clip_name(clip.name.clone());
                ui.set_selected_clip_loop_count(clip.loop_count);
                ui.set_selected_clip_sync_group(clip.sync_group);
                ui.set_selected_clip_sync_group_text(clip.sync_group.to_string().into());
                ui.set_last_action(
                    format!("SELECT REGION: {} · TRACK {}", clip.name, track_id).into(),
                );
            }
        }
    });
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
            let mut pattern_copies = Vec::new();
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
                pattern_copies.push((track_id, source_id, new_id));
            }
            if !core.end_undo_transaction() {
                let _ = core.abort_undo_transaction();
                return;
            }
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                for (track_id, source_id, new_id) in pattern_copies {
                    copy_step_sequencer_pattern(&ui, track_id, source_id, new_id);
                }
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
            let mut moved_sync_groups = HashSet::new();
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
                if clip.sync_group > 0 {
                    if moved_sync_groups.insert(clip.sync_group) {
                        let mut member_count = 0usize;
                        for row in 0..tracks.row_count() {
                            if let Some(member_track) = tracks.row_data(row) {
                                member_count += member_track
                                    .clips
                                    .iter()
                                    .filter(|member| member.sync_group == clip.sync_group)
                                    .count();
                            }
                        }
                        member_count = member_count.max(1);
                        moves.push((*track_id, *target_id, next as f64, true, member_count));
                    }
                } else {
                    moves.push((*track_id, *target_id, next as f64, false, 1));
                }
            }
            core.begin_undo_transaction("Move Selected Clips");
            for (track_id, target_id, next, is_sync_group, _) in &moves {
                let moved = if *is_sync_group {
                    core.move_region_sync_group(*track_id, *target_id, *next)
                } else {
                    core.move_region(*track_id, *target_id, *next)
                };
                if !moved {
                    let _ = core.abort_undo_transaction();
                    if *is_sync_group {
                        ui.set_last_action(
                            "SYNC GROUP MOVE FAILED: UNLOCK MEMBERS AND KEEP ALL CLIPS IN RANGE"
                                .into(),
                        );
                    }
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                ui.set_sel_cid(cid);
                let moved_clip_count = moves.iter().map(|entry| entry.4).sum::<usize>();
                ui.set_last_action(
                    format!(
                        "MOVED {} CLIP{}",
                        moved_clip_count,
                        if moved_clip_count == 1 { "" } else { "S" }
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<EditorActions>().on_sync_group_changed({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, group_text| {
            let raw = group_text.trim();
            let Ok(group) = raw.parse::<u32>() else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("SYNC GROUP MUST BE A NON-NEGATIVE WHOLE NUMBER".into());
                }
                return;
            };
            if group > i32::MAX as u32 {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("SYNC GROUP MUST BE 0–2147483647".into());
                }
                return;
            }
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Set Region Sync Group");
            for (track_id, region_id) in &targets {
                if !core.set_region_sync_group(*track_id, *region_id, group) {
                    let _ = core.abort_undo_transaction();
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            "SYNC GROUP CHANGE FAILED: UNLOCK ALL SELECTED REGIONS FIRST".into(),
                        );
                    }
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    ui.set_selected_clip_sync_group(group as i32);
                    ui.set_selected_clip_sync_group_text(group.to_string().into());
                    ui.set_last_action(
                        format!(
                            "{} SYNC GROUP {} ON {} CLIP{}",
                            if group == 0 { "CLEARED" } else { "SET" },
                            group,
                            targets.len(),
                            if targets.len() == 1 { "" } else { "S" }
                        )
                        .into(),
                    );
                }
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
            let existing_ids = targets.iter().fold(
                std::collections::HashMap::<u32, HashSet<u32>>::new(),
                |mut ids, (track_id, _)| {
                    let row = (0..tracks.row_count())
                        .filter_map(|row| tracks.row_data(row))
                        .find(|track| track.id as u32 == *track_id);
                    if let Some(track) = row {
                        ids.entry(*track_id)
                            .or_default()
                            .extend(track.clips.iter().map(|clip| clip.id as u32));
                    }
                    ids
                },
            );
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
                    let mut new_right_regions = Vec::new();
                    for (track_id, old_ids) in &existing_ids {
                        if let Some(track) = (0..tracks.row_count())
                            .filter_map(|row| tracks.row_data(row))
                            .find(|track| track.id as u32 == *track_id)
                        {
                            new_right_regions.extend(
                                track
                                    .clips
                                    .iter()
                                    .filter(|clip| {
                                        !old_ids.contains(&(clip.id as u32))
                                            && (clip.start_beat - beat).abs() < 0.0001
                                    })
                                    .map(|clip| (*track_id, clip.id as u32)),
                            );
                        }
                    }
                    new_right_regions.sort_by_key(|(track_id, region_id)| (*track_id, *region_id));
                    let mut split_sources = targets.clone();
                    split_sources.sort();
                    for ((track_id, source_id), (right_track_id, right_id)) in
                        split_sources.into_iter().zip(new_right_regions)
                    {
                        if track_id == right_track_id {
                            copy_step_sequencer_pattern(&ui, track_id, source_id, right_id);
                        }
                    }
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
    ui.global::<EditorActions>().on_quantize_sync_group({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, strength, grid_index, swing| {
            if !strength.is_finite() || !(0.0..=1.0).contains(&strength)
                || !(0..=12).contains(&grid_index) || !swing.is_finite() {
                return;
            }
            let Some((track_id, region_id)) = region_targets(&tracks, clip_id).into_iter().next() else {
                return;
            };
            let started = core.quantize_region_sync_group(
                track_id, region_id, strength, grid_index as u8, swing,
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if started {
                    "ANALYZING SYNCED AUDIO TRANSIENTS…".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "quantize could not start: unlock matching, non-reversed 1.0x audio regions first",
                    ).into()
                });
            }
            if started {
                poll_group_quantize(weak.clone(), core.clone(), tracks.clone(), track_id, region_id);
            }
        }
    });
    ui.global::<EditorActions>().on_align_selected_audio({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |reference_region_id| {
            let selected = region_targets(&tracks, reference_region_id);
            let selected: Vec<_> = selected.into_iter().filter(|(track_id, _)| {
                (0..tracks.row_count()).filter_map(|row| tracks.row_data(row)).any(|track| {
                    track.id == *track_id as i32 && matches!(track.r#type.as_str(), "Audio" | "Vocal")
                })
            }).collect();
            if selected.len() < 2 {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(ui_error_message(
                        UiErrorKind::Project,
                        "audio alignment needs a reference and at least one selected Audio/Vocal region",
                    ).into());
                }
                return;
            }
            let Some(reference) = selected.iter()
                .copied()
                .find(|(_, region_id)| *region_id as i32 == reference_region_id)
            else {
                return;
            };
            let targets: Vec<_> = selected.into_iter().filter(|pair| *pair != reference).collect();
            let target_count = targets.len();
            let started = core.align_regions_to_reference(reference.0, reference.1, &targets);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if started {
                    "ANALYZING AUDIO ALIGNMENT…".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "alignment could not start: select up to 32 unlocked, unreversed Audio/Vocal regions with matching lengths",
                    ).into()
                });
            }
            if started {
                poll_region_alignment(
                    weak.clone(), core.clone(), tracks.clone(), reference.0, reference.1, target_count,
                );
            }
        }
    });
    ui.global::<EditorActions>().on_clip_gain_changed({
        let weak = weak.clone();
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
                if let Some(ui) = weak.upgrade() {
                    if ui.get_sel_cid() == clip_id {
                        ui.set_selected_clip_gain(bounded);
                    }
                }
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
    ui.global::<EditorActions>().on_trim_clip_by_beats({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |clip_id, trim_left, delta_beats| {
            if !delta_beats.is_finite() {
                return;
            }
            let requested = region_targets(&tracks, clip_id);
            if requested.is_empty() {
                return;
            }
            let Ok(layout) =
                serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            else {
                return;
            };
            let Some(project_tracks) = layout.as_array() else {
                return;
            };
            let mut edits = Vec::new();
            for (track_id, region_id) in requested {
                let Some(track_model) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id as u32 == track_id)
                else {
                    return;
                };
                if track_model.r#type != "Audio" && track_model.r#type != "Vocal" {
                    continue;
                }
                let Some(clip) = track_model
                    .clips
                    .iter()
                    .find(|clip| clip.id as u32 == region_id)
                else {
                    return;
                };
                if !clip.start_beat.is_finite()
                    || !clip.length_beats.is_finite()
                    || clip.length_beats <= 0.0
                {
                    return;
                }
                let start_beat = (if trim_left {
                    (clip.start_beat + delta_beats).max(0.0)
                } else {
                    clip.start_beat
                }) as f64;
                let end_beat = (if trim_left {
                    clip.start_beat + clip.length_beats
                } else {
                    clip.start_beat + clip.length_beats + delta_beats
                }) as f64;
                if !start_beat.is_finite() || !end_beat.is_finite() || end_beat <= start_beat {
                    return;
                }

                let Some(region) = project_tracks
                    .iter()
                    .find(|track| {
                        track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                    })
                    .and_then(|track| track.get("regions"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|regions| {
                        regions.iter().find(|region| {
                            region.get("id").and_then(serde_json::Value::as_u64)
                                == Some(region_id as u64)
                        })
                    })
                else {
                    return;
                };
                let Some(current_start) = region.get("start").and_then(serde_json::Value::as_u64)
                else {
                    return;
                };
                let Some(current_length) = region.get("len").and_then(serde_json::Value::as_u64)
                else {
                    return;
                };
                let Some(current_end) = current_start.checked_add(current_length) else {
                    return;
                };
                let Some(source_offset) = region
                    .get("source_offset")
                    .and_then(serde_json::Value::as_u64)
                else {
                    return;
                };
                let Some(base_source_offset) = region
                    .get("base_source_offset")
                    .and_then(serde_json::Value::as_u64)
                else {
                    return;
                };
                let Some(base_length) = region
                    .get("base_length")
                    .and_then(serde_json::Value::as_u64)
                else {
                    return;
                };
                let Some(trimmed_offset) = source_offset.checked_sub(base_source_offset) else {
                    return;
                };
                let Some(base_start) = current_start.checked_sub(trimmed_offset) else {
                    return;
                };
                if base_length == 0 || base_start.checked_add(base_length).is_none() {
                    return;
                }
                let base_end = base_start + base_length;
                let requested_start = if trim_left {
                    core.beats_to_samples(start_beat)
                } else {
                    current_start
                };
                let requested_end = if trim_left {
                    current_end
                } else {
                    core.beats_to_samples(end_beat)
                };
                let start_norm =
                    requested_start.saturating_sub(base_start) as f64 / base_length as f64;
                let end_norm = requested_end.saturating_sub(base_start) as f64 / base_length as f64;
                let start_norm = if requested_start <= base_start {
                    0.0
                } else {
                    start_norm.min(1.0)
                };
                let end_norm = if requested_end >= base_end {
                    1.0
                } else {
                    end_norm.min(1.0)
                };
                if !start_norm.is_finite() || !end_norm.is_finite() || start_norm >= end_norm {
                    return;
                }
                edits.push((track_id, region_id, start_norm as f32, end_norm as f32));
            }
            if edits.is_empty() {
                return;
            }

            core.begin_undo_transaction("Trim Selected Clips");
            for (track_id, region_id, start, end) in &edits {
                if !core.set_region_trim(*track_id, *region_id, *start, *end) {
                    let _ = core.abort_undo_transaction();
                    return;
                }
            }
            if core.end_undo_transaction() {
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = weak.upgrade() {
                    if let Some(clip) = (0..tracks.row_count())
                        .filter_map(|row| tracks.row_data(row))
                        .flat_map(|track| track.clips.iter().collect::<Vec<_>>())
                        .find(|clip| clip.id == clip_id)
                    {
                        ui.set_selected_clip_trim_start(clip.trim_start);
                        ui.set_selected_clip_trim_end(clip.trim_end);
                    }
                    ui.set_last_action(
                        format!(
                            "TRIMMED {} CLIP{}",
                            edits.len(),
                            if edits.len() == 1 { "" } else { "S" }
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
            let ratio = ratio.clamp(0.5, 2.0);
            let targets = region_targets(&tracks, clip_id);
            if targets.is_empty() {
                return;
            }
            core.begin_undo_transaction("Change Selected Clip Playback Speed");
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
                            "CHANGED PLAYBACK SPEED ON {} CLIP{}: {:.2}x",
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
    ui.global::<EditorActions>()
        .on_pitch_preserve_warp_changed({
            let weak = weak.clone();
            let core = core.clone();
            let tracks = tracks.clone();
            move |clip_id, enabled| {
                let targets = region_targets(&tracks, clip_id);
                if targets.is_empty() {
                    return;
                }
                core.begin_undo_transaction("Change Pitch-Preserving Warp");
                for (track_id, target_id) in &targets {
                    if !core.set_region_pitch_preserve_warp(*track_id, *target_id, enabled) {
                        let _ = core.abort_undo_transaction();
                        return;
                    }
                }
                if core.end_undo_transaction() {
                    sync_tracks_from_engine(&tracks, &core);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_selected_clip_pitch_preserve_warp(enabled);
                        ui.set_last_action(
                            format!(
                                "{} PITCH-PRESERVING WARP ON {} CLIP{}",
                                if enabled { "ENABLED" } else { "DISABLED" },
                                targets.len(),
                                if targets.len() == 1 { "" } else { "S" }
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
    ui.global::<EditorActions>().on_analyze_audio_pitch({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id| {
            if track_id <= 0 || region_id <= 0 {
                return;
            }
            let sample_rate = core.get_sample_rate();
            if !core.start_region_audio_note_analysis(
                track_id as u32,
                region_id as u32,
                sample_rate,
            ) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            "pitch analysis failed: select a decoded audio region",
                        )
                        .into(),
                    );
                }
                return;
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("PITCH ANALYSIS RUNNING…".into());
            }
            poll_audio_pitch_analysis(
                weak.clone(),
                core.clone(),
                tracks.clone(),
                track_id as u32,
                region_id as u32,
            );
        }
    });
    ui.global::<EditorActions>().on_adjust_audio_pitch_segment({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id, start_seconds, end_seconds, pitch_cents, formant_cents| {
            if track_id <= 0
                || region_id <= 0
                || !start_seconds.is_finite()
                || !end_seconds.is_finite()
                || !pitch_cents.is_finite()
                || !formant_cents.is_finite()
            {
                return;
            }
            let changed = core.set_region_audio_note_segment(
                track_id as u32,
                region_id as u32,
                start_seconds as f64,
                end_seconds as f64,
                pitch_cents as f64,
                formant_cents as f64,
            );
            if changed {
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if changed {
                    format!("PITCH SEGMENT UPDATED · {pitch_cents:+.0} CENTS").into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "pitch segment edit was rejected by Core",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<EditorActions>().on_set_audio_pitch_anchor({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id, segment_start, position, pitch, _formant| {
            if track_id <= 0
                || region_id <= 0
                || !segment_start.is_finite()
                || !position.is_finite()
                || !pitch.is_finite()
            {
                return;
            }
            let changed = core.set_region_audio_note_pitch_anchor(
                track_id as u32,
                region_id as u32,
                segment_start as f64,
                position as f64,
                pitch as f64,
            );
            if changed {
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if changed {
                    format!("PITCH CURVE POINT UPDATED · {pitch:+.0} CENTS").into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "pitch curve point was rejected by Core",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<EditorActions>().on_set_audio_formant_anchor({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id, segment_start, position, formant| {
            if track_id <= 0
                || region_id <= 0
                || !segment_start.is_finite()
                || !position.is_finite()
                || !formant.is_finite()
            {
                return;
            }
            let changed = core.set_region_audio_note_formant_anchor(
                track_id as u32,
                region_id as u32,
                segment_start as f64,
                position as f64,
                formant as f64,
            );
            if changed {
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if changed {
                    format!("FORMANT CURVE POINT UPDATED · {formant:+.0} CENTS").into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "formant curve point was rejected by Core",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<EditorActions>().on_move_audio_note_anchor({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id, segment_start, old_position, position, value, formant| {
            if track_id <= 0
                || region_id <= 0
                || !segment_start.is_finite()
                || !old_position.is_finite()
                || !position.is_finite()
                || !value.is_finite()
            {
                return;
            }
            let changed = core.move_region_audio_note_anchor(
                track_id as u32,
                region_id as u32,
                segment_start as f64,
                old_position as f64,
                position as f64,
                value as f64,
                formant,
            );
            if changed {
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if changed {
                    format!(
                        "{} CURVE ANCHOR MOVED",
                        if formant { "FORMANT" } else { "PITCH" }
                    )
                    .into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "curve anchor move was rejected by Core",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<EditorActions>().on_warp_audio_pitch_segment({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, region_id, segment_start, new_start, new_end| {
            if track_id <= 0
                || region_id <= 0
                || !segment_start.is_finite()
                || !new_start.is_finite()
                || !new_end.is_finite()
                || new_end <= new_start
            {
                return;
            }
            let changed = core.warp_region_audio_note_segment(
                track_id as u32,
                region_id as u32,
                segment_start as f64,
                new_start as f64,
                new_end as f64,
            );
            if changed {
                sync_tracks_from_engine(&tracks, &core);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if changed {
                    "PITCH SEGMENT TIMING UPDATED · UNDO AVAILABLE".into()
                } else {
                    ui_error_message(
                        UiErrorKind::Project,
                        "pitch segment timing could not be applied; it may overlap another note",
                    )
                    .into()
                });
            }
        }
    });
    ui.global::<EditorActions>()
        .on_extract_audio_pitch_to_midi({
            let weak = weak.clone();
            let core = core.clone();
            let tracks = tracks.clone();
            move |track_id, region_id| {
                if track_id <= 0 || region_id <= 0 {
                    return;
                }
                let Some(source_track) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id == track_id)
                else {
                    return;
                };
                let Some(region) = source_track.clips.iter().find(|clip| clip.id == region_id)
                else {
                    return;
                };
                let sample_rate = core.get_sample_rate().max(1.0);
                let region_start_sample = core.beats_to_samples(region.start_beat.max(0.0) as f64);
                let region_end_sample = core.beats_to_samples(
                    (region.start_beat.max(0.0) + region.length_beats.max(0.0)) as f64,
                );
                let region_start_beat = region.start_beat.max(0.0) as f64;
                let segments = region.audio_pitch_segments.iter().collect::<Vec<_>>();
                let mut extracted = Vec::new();
                for segment in segments {
                    if !segment.start_seconds.is_finite()
                        || !segment.end_seconds.is_finite()
                        || !segment.detected_pitch_cents.is_finite()
                        || !segment.pitch_offset_cents.is_finite()
                    {
                        continue;
                    }
                    let start_offset =
                        (segment.start_seconds.max(0.0) as f64 * sample_rate).round();
                    let end_offset = (segment.end_seconds.max(0.0) as f64 * sample_rate).round();
                    if end_offset <= start_offset {
                        continue;
                    }
                    let start_sample = region_start_sample.saturating_add(start_offset as u64);
                    let end_sample = region_start_sample
                        .saturating_add(end_offset as u64)
                        .min(region_end_sample);
                    if end_sample <= start_sample {
                        continue;
                    }
                    let start_beat = core.samples_to_beats(start_sample).max(region_start_beat);
                    let end_beat = core.samples_to_beats(end_sample);
                    let length_beats = (end_beat - start_beat).max(0.015625);
                    let pitch = ((segment.detected_pitch_cents + segment.pitch_offset_cents)
                        / 100.0)
                        .round()
                        .clamp(0.0, 127.0) as i32;
                    extracted.push(ZNote {
                        region_id: 0,
                        pitch,
                        midi_channel: 0,
                        start_beat: start_beat as f32,
                        length_beats: length_beats as f32,
                        velocity: 100,
                        articulation: 0,
                        vibrato_amount: 0.0,
                        vibrato_rate_millihz: 5000,
                        phoneme: "".into(),
                        pitch_curve_cents: slint::ModelRc::default(),
                        portamento_samples: 0,
                        probability: 100,
                        repeat_count: 1,
                        selected: false,
                        lyric: "".into(),
                    });
                }
                if extracted.is_empty() {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            "MIDI EXTRACTION FAILED · NO VALID PITCH SEGMENTS".into(),
                        );
                    }
                    return;
                }

                let mut target_track = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument"))
                    .map(|track| track.id);
                if target_track.is_none() {
                    let created = core.add_midi_track();
                    if created == 0 || !sync_tracks_from_engine(&tracks, &core) {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_last_action(
                                ui_error_message(
                                    UiErrorKind::Project,
                                    "could not create a MIDI track",
                                )
                                .into(),
                            );
                        }
                        return;
                    }
                    target_track = Some(created as i32);
                }
                let Some(target_track) = target_track else {
                    return;
                };
                let Some(row) = (0..tracks.row_count()).find(|row| {
                    tracks
                        .row_data(*row)
                        .is_some_and(|track| track.id == target_track)
                }) else {
                    return;
                };
                let Some(mut track) = tracks.row_data(row) else {
                    return;
                };
                let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
                let mut added = 0usize;
                for note in extracted {
                    if notes.iter().any(|existing| {
                        existing.pitch == note.pitch
                            && (existing.start_beat - note.start_beat).abs() < 0.02
                            && (existing.length_beats - note.length_beats).abs() < 0.04
                    }) {
                        continue;
                    }
                    notes.push(note);
                    added += 1;
                }
                if added == 0 {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action("MIDI EXTRACTION · NOTES ALREADY EXIST".into());
                    }
                    return;
                }
                track.piano_roll_notes = slint::ModelRc::new(VecModel::from(notes));
                tracks.set_row_data(row, track);
                if !sync_midi_notes_to_core("", &tracks, &core) {
                    sync_tracks_from_engine(&tracks, &core);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                "could not commit extracted MIDI notes",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("EXTRACTED {added} MIDI NOTES").into());
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
            let mut pattern_copies = Vec::new();
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
                pattern_copies.push((track_id, source_id, new_id));
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
                for (track_id, source_id, new_id) in pattern_copies {
                    copy_step_sequencer_pattern(&ui, track_id, source_id, new_id);
                }
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
