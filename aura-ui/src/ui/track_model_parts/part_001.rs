// Track model hydration and MIDI synchronization.

use crate::slint_ui::{ZNote, Z_AutomationLane, Z_AutomationPoint, Z_Clip, Z_Track};
use crate::ui::sync::replace_track;
use aura_core_bridge::AuraCore;
use slint::Model;
use std::collections::{HashMap, HashSet};

type PreviousMidiMetadata = HashMap<(u32, u8, u64, u64), (String, String, Vec<i16>, u16, u16, u32)>;

pub(crate) fn sync_midi_notes_to_core(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &AuraCore,
) {
    let record_undo = project_path.is_empty();
    if record_undo {
        core.begin_undo_transaction("Edit MIDI Notes");
    }
    let mut packed = Vec::new();
    let mut scheduled = Vec::new();
    let previous_metadata: PreviousMidiMetadata =
        serde_json::from_str::<serde_json::Value>(&core.midi_notes_json())
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|note| {
                let key = (
                    note.get("track_id")?.as_u64()? as u32,
                    note.get("pitch")?.as_u64()? as u8,
                    note.get("start_sample")?.as_u64()?,
                    note.get("length_samples")?.as_u64()?,
                );
                Some((
                    key,
                    (
                        note.get("lyric")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        note.get("phoneme")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        note.get("pitch_curve_cents")
                            .and_then(|v| v.as_array())
                            .map(|values| {
                                values
                                    .iter()
                                    .filter_map(|v| v.as_i64().and_then(|n| i16::try_from(n).ok()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        note.get("vibrato_depth_cents")
                            .and_then(|v| v.as_u64())
                            .unwrap_or_default() as u16,
                        note.get("vibrato_rate_millihz")
                            .and_then(|v| v.as_u64())
                            .and_then(|value| u16::try_from(value).ok())
                            .unwrap_or(5_000),
                        note.get("portamento_samples")
                            .and_then(|v| v.as_u64())
                            .unwrap_or_default() as u32,
                    ),
                ))
            })
            .collect();
    for row in 0..tracks.row_count() {
        let Some(track) = tracks.row_data(row) else {
            continue;
        };
        for note in track.piano_roll_notes.iter() {
            let start_beat = note.start_beat.max(0.0) as f64;
            let end_beat = start_beat + note.length_beats.max(0.015625) as f64;
            let start = core.beats_to_samples(start_beat);
            let end = core.beats_to_samples(end_beat);
            let length = end.saturating_sub(start).max(1);
            packed.extend([
                track.id.max(0) as u64,
                note.pitch.clamp(0, 127) as u64,
                note.velocity.clamp(1, 127) as u64,
                start,
                length,
            ]);
            let key = (
                track.id.max(0) as u32,
                note.pitch.clamp(0, 127) as u8,
                start,
                length,
            );
            let metadata = previous_metadata.get(&key).cloned().unwrap_or_else(|| {
                (
                    note.lyric.to_string(),
                    String::new(),
                    Vec::new(),
                    (note.vibrato_amount.clamp(0.0, 1.0) * 1200.0).round() as u16,
                    u16::try_from(note.vibrato_rate_millihz.max(500)).unwrap_or(5_000),
                    0,
                )
            });
            scheduled.push((
                track.id.max(0) as u32,
                note.pitch.clamp(0, 127) as u8,
                note.velocity.clamp(1, 127) as u8,
                start,
                length,
                metadata,
            ));
        }
    }
    if core.replace_midi_notes(packed, record_undo) {
        // Do not clear authoring metadata until the bounded native snapshot
        // has been accepted. A rejected snapshot must leave lyrics intact.
        core.clear_midi_note_metadata();
        for (
            track_id,
            pitch,
            velocity,
            start,
            length,
            (lyric, phoneme, pitch_curve, vibrato, vibrato_rate, portamento),
        ) in scheduled
        {
            let _ = core.set_midi_note_lyric(track_id, pitch, velocity, start, length, &lyric);
            // Apply empty articulation too: otherwise a reload/reconciliation
            // leaves stale phoneme, pitch-curve, or vibrato metadata attached
            // to a note that was intentionally cleared in the UI.
            let _ = core.set_midi_note_articulation(
                track_id,
                pitch,
                start,
                &phoneme,
                &pitch_curve,
                vibrato,
                portamento,
            );
            let _ =
                core.set_midi_note_vibrato_rate_without_undo(track_id, pitch, start, vibrato_rate);
        }
        if record_undo {
            let _ = core.end_undo_transaction();
        }
    } else if record_undo {
        // The transaction contains no accepted mutation. Closing it as a
        // normal transaction would leave a misleading empty undo item.
        let _ = core.abort_undo_transaction();
    }
}

/// Rebuild the UI piano-roll notes from the native playback snapshot after a
/// native Undo/Redo. Native undo restores audio notes atomically, while lyric
/// text remains UI/project metadata and is preserved when its note identity
/// still matches the restored snapshot.
pub(crate) fn sync_midi_notes_from_core(tracks: &slint::VecModel<Z_Track>, core: &AuraCore) {
    let mut lyrics = HashMap::new();
    let mut articulation = HashMap::new();
    for row in 0..tracks.row_count() {
        let Some(track) = tracks.row_data(row) else {
            continue;
        };
        for note in track.piano_roll_notes.iter() {
            let start = core.beats_to_samples(note.start_beat.max(0.0) as f64);
            let end = core.beats_to_samples(
                (note.start_beat.max(0.0) + note.length_beats.max(0.015625)) as f64,
            );
            lyrics.insert(
                (
                    track.id.max(0) as u32,
                    note.pitch.clamp(0, 127) as u8,
                    start,
                    end.saturating_sub(start).max(1),
                ),
                note.lyric.to_string(),
            );
        }
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&core.midi_notes_json()) {
        if let Some(notes) = value.as_array() {
            for note in notes {
                let (Some(track), Some(pitch), Some(start), Some(length)) = (
                    note.get("track_id").and_then(|v| v.as_u64()),
                    note.get("pitch").and_then(|v| v.as_u64()),
                    note.get("start_sample").and_then(|v| v.as_u64()),
                    note.get("length_samples").and_then(|v| v.as_u64()),
                ) else {
                    continue;
                };
                let depth = note
                    .get("vibrato_depth_cents")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(1200) as f32
                    / 1200.0;
                let rate = note
                    .get("vibrato_rate_millihz")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u16::try_from(v).ok())
                    .unwrap_or(5_000);
                articulation.insert((track as u32, pitch as u8, start, length), (depth, rate));
            }
        }
    }

    let packed = core.midi_notes_snapshot();
    if !packed.len().is_multiple_of(5) {
        return;
    }
    let mut by_track: HashMap<u32, Vec<ZNote>> = HashMap::new();
    for item in packed.as_chunks::<5>().0 {
        let (track_id, pitch, velocity, start, length) =
            (item[0], item[1], item[2], item[3], item[4]);
        if track_id == 0 || pitch > 127 || velocity == 0 || velocity > 127 || length == 0 {
            continue;
        }
        let lyric = lyrics
            .get(&(track_id as u32, pitch as u8, start, length))
            .cloned()
            .unwrap_or_default();
        let (vibrato_amount, vibrato_rate_millihz) = articulation
            .get(&(track_id as u32, pitch as u8, start, length))
            .copied()
            .unwrap_or((0.0, 5_000));
        let start_beat = core.samples_to_beats(start).max(0.0) as f32;
        let end_beat = core
            .samples_to_beats(start.saturating_add(length))
            .max(start_beat as f64) as f32;
        by_track.entry(track_id as u32).or_default().push(ZNote {
            pitch: pitch as i32,
            start_beat,
            length_beats: (end_beat - start_beat).max(0.015625),
            velocity: velocity as i32,
            articulation: 0,
            vibrato_amount,
            vibrato_rate_millihz: i32::from(vibrato_rate_millihz),
            selected: false,
            lyric: lyric.into(),
        });
    }
    for row in 0..tracks.row_count() {
        let Some(mut track) = tracks.row_data(row) else {
            continue;
        };
        let notes = by_track
            .remove(&(track.id.max(0) as u32))
            .unwrap_or_default();
        track.piano_roll_notes = slint::ModelRc::new(slint::VecModel::from(notes));
        let _ = replace_track(tracks, row, track);
    }
}

pub(crate) fn sync_tracks_from_engine(
    tracks_model: &slint::VecModel<Z_Track>,
    core: &aura_core_bridge::AuraCore,
) -> bool {
    sync_tracks_from_engine_with_empty_policy(tracks_model, core, false)
}

/// Hydration entry point used immediately after a validated project load.
/// An empty track array is a valid persisted project in this context; the
/// ordinary telemetry synchronizer keeps rejecting empty snapshots so a
/// device-offline/startup blip cannot erase a usable UI model.
pub(crate) fn sync_tracks_from_engine_allow_empty(
    tracks_model: &slint::VecModel<Z_Track>,
    core: &aura_core_bridge::AuraCore,
) -> bool {
    sync_tracks_from_engine_with_empty_policy(tracks_model, core, true)
}
