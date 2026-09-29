// Track model hydration and MIDI synchronization.

use crate::slint_ui::{ZNote, Z_AudioPitchAnchor, Z_AudioPitchSegment, Z_AutomationLane, Z_AutomationPoint, Z_Clip, Z_Fx, Z_Track};
use crate::ui::sync::replace_track;
use hirari_core_bridge::HirariCore;
use slint::Model;
use std::collections::{HashMap, HashSet};

pub(crate) fn sync_midi_notes_to_core(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    let record_undo = project_path.is_empty();
    if record_undo {
        core.begin_undo_transaction("Edit MIDI Notes");
    }
    let mut scheduled = Vec::new();
    let mut vibrato_rates = Vec::new();
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
            let pitch_curve_cents = note
                .pitch_curve_cents
                .iter()
                .filter_map(|value| i16::try_from(value).ok())
                .take(256)
                .collect();
            let phoneme = note.phoneme.to_string();
            if phoneme.len() > 128 || phoneme.contains('\0') {
                if record_undo {
                    let _ = core.abort_undo_transaction();
                }
                sync_midi_notes_from_core(tracks, core);
                return false;
            }
            scheduled.push(hirari_core_bridge::project_contracts::MidiNoteContract {
                region_id: note.region_id.max(0) as u32,
                midi_channel: note.midi_channel.clamp(0, 15) as u8,
                articulation: note.articulation.clamp(0, 127) as u8,
                track_id: track.id.max(0) as u32,
                pitch: note.pitch.clamp(0, 127) as u8,
                velocity: note.velocity.clamp(1, 127) as u8,
                start_sample: start,
                length_samples: length,
                lyric: note.lyric.to_string(),
                phoneme,
                pitch_curve_cents,
                vibrato_depth_cents: (note.vibrato_amount.clamp(0.0, 1.0) * 1200.0).round() as u16,
                vibrato_rate_millihz: note.vibrato_rate_millihz.clamp(500, 20_000) as u16,
                portamento_samples: (note.portamento_samples.max(0) as u32)
                    .min(length.min(u64::from(u32::MAX)) as u32),
                probability: note.probability.clamp(0, 100) as u8,
                repeat_count: note.repeat_count.clamp(1, i32::from(u16::MAX)) as u16,
            });
            vibrato_rates.push((
                track.id.max(0) as u32,
                note.region_id.max(0) as u32,
                note.pitch.clamp(0, 127) as u8,
                start,
                note.vibrato_rate_millihz.clamp(500, 20_000) as u16,
            ));
        }
    }
    if core.replace_midi_note_contracts(scheduled, record_undo) {
        let mut metadata_applied = true;
        for (track_id, region_id, pitch, start, rate) in vibrato_rates {
            metadata_applied &= core.set_midi_note_vibrato_rate_for_region_without_undo(
                track_id, region_id, pitch, start, rate,
            );
        }
        if record_undo {
            let _ = core.end_undo_transaction();
        }
        if !metadata_applied {
            // MIDI edit handlers update the Slint model optimistically. If
            // Core accepted the note snapshot but rejected any authoring
            // metadata, republish the authoritative native state so the UI
            // cannot keep showing an edit that Core did not accept.
            sync_midi_notes_from_core(tracks, core);
        }
        metadata_applied
    } else if record_undo {
        // The transaction contains no accepted mutation. Closing it as a
        // normal transaction would leave a misleading empty undo item.
        let _ = core.abort_undo_transaction();
        sync_midi_notes_from_core(tracks, core);
        false
    } else {
        sync_midi_notes_from_core(tracks, core);
        false
    }
}

/// Rebuild the UI piano-roll notes from the native playback snapshot after a
/// native Undo/Redo or project hydration. Core note metadata is authoritative;
/// matching UI metadata remains a fallback for older snapshots.
pub(crate) fn sync_midi_notes_from_core(tracks: &slint::VecModel<Z_Track>, core: &HirariCore) {
    let mut lyrics = HashMap::new();
    let mut articulation = HashMap::new();
    let mut articulation_ids = HashMap::new();
    let mut vocal_metadata = HashMap::new();
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
                    note.region_id.max(0) as u32,
                    note.pitch.clamp(0, 127) as u8,
                    note.midi_channel.clamp(0, 15) as u8,
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
                let midi_channel = note
                    .get("midi_channel")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
                    .min(15) as u8;
                let region_id = note
                    .get("region_id")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
                    .min(u64::from(u32::MAX)) as u32;
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
                if let Some(lyric) = note.get("lyric").and_then(|value| value.as_str()) {
                    lyrics.insert(
                        (track as u32, region_id, pitch as u8, midi_channel, start, length),
                        lyric.to_owned(),
                    );
                }
                let note_key = (track as u32, region_id, pitch as u8, midi_channel, start, length);
                articulation.insert(note_key, (depth, rate));
                articulation_ids.insert(
                    note_key,
                    note.get("articulation")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                        .min(127) as i32,
                );
                let pitch_curve_cents = note
                    .get("pitch_curve_cents")
                    .and_then(|value| value.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_i64().and_then(|n| i32::try_from(n).ok()))
                            .take(256)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                vocal_metadata.insert(
                    note_key,
                    (
                        note.get("phoneme")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        pitch_curve_cents,
                        note.get("portamento_samples")
                            .and_then(|value| value.as_u64())
                            .unwrap_or_default()
                            .min(i64::from(i32::MAX) as u64) as i32,
                        note.get("probability")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(100)
                            .min(100) as i32,
                        note.get("repeat_count")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(1)
                            .clamp(1, u64::from(u16::MAX)) as i32,
                    ),
                );
            }
        }
    }

    let packed = core.midi_notes_snapshot();
    if !packed.len().is_multiple_of(9) {
        return;
    }
    let mut by_track: HashMap<u32, Vec<ZNote>> = HashMap::new();
    for item in packed.as_chunks::<9>().0 {
        let (track_id, pitch, velocity, midi_channel, start, length, probability, region_id, articulation_id) =
            (item[0], item[1], item[2], item[3], item[4], item[5], item[6], item[7], item[8]);
        if track_id == 0
            || pitch > 127
            || velocity == 0
            || velocity > 127
            || midi_channel > 15
            || probability > 100
            || region_id > u64::from(u32::MAX)
            || articulation_id > 127
            || length == 0
        {
            continue;
        }
        let lyric = lyrics
            .get(&(
                track_id as u32,
                region_id as u32,
                pitch as u8,
                midi_channel as u8,
                start,
                length,
            ))
            .cloned()
            .unwrap_or_default();
        let (vibrato_amount, vibrato_rate_millihz) = articulation
            .get(&(
                track_id as u32,
                region_id as u32,
                pitch as u8,
                midi_channel as u8,
                start,
                length,
            ))
            .copied()
            .unwrap_or((0.0, 5_000));
        let (phoneme, pitch_curve_cents, portamento_samples, probability, repeat_count) =
            vocal_metadata
                .get(&(
                    track_id as u32,
                    region_id as u32,
                    pitch as u8,
                    midi_channel as u8,
                    start,
                    length,
                ))
                .cloned()
                .unwrap_or_else(|| (String::new(), Vec::new(), 0, probability as i32, 1));
        let start_beat = core.samples_to_beats(start).max(0.0) as f32;
        let end_beat = core
            .samples_to_beats(start.saturating_add(length))
            .max(start_beat as f64) as f32;
        by_track.entry(track_id as u32).or_default().push(ZNote {
            region_id: region_id as i32,
            midi_channel: midi_channel as i32,
            pitch: pitch as i32,
            start_beat,
            length_beats: (end_beat - start_beat).max(0.015625),
            velocity: velocity as i32,
            articulation: articulation_ids
                .get(&(track_id as u32, region_id as u32, pitch as u8, midi_channel as u8, start, length))
                .copied()
                .unwrap_or(articulation_id as i32),
            vibrato_amount,
            vibrato_rate_millihz: i32::from(vibrato_rate_millihz),
            phoneme: phoneme.into(),
            pitch_curve_cents: slint::ModelRc::new(slint::VecModel::from(pitch_curve_cents)),
            portamento_samples,
            probability,
            repeat_count,
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
    core: &hirari_core_bridge::HirariCore,
) -> bool {
    sync_tracks_from_engine_with_empty_policy(tracks_model, core, false)
}

/// Hydration entry point used immediately after a validated project load.
/// An empty track array is a valid persisted project in this context; the
/// ordinary telemetry synchronizer keeps rejecting empty snapshots so a
/// device-offline/startup blip cannot erase a usable UI model.
pub(crate) fn sync_tracks_from_engine_allow_empty(
    tracks_model: &slint::VecModel<Z_Track>,
    core: &hirari_core_bridge::HirariCore,
) -> bool {
    sync_tracks_from_engine_with_empty_policy(tracks_model, core, true)
}
