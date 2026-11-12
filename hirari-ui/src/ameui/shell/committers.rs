//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Pointer travel since the lane press, in logical pixels. Clicks
    /// within this distance select; beyond it the press becomes a seek drag.
    pub(in crate::ameui) fn click_distance(&self, event: &CanvasPointerEvent) -> f64 {
        (event.x - self.down_x).hypot(event.y - self.down_y)
    }

    /// Clamps a trim beat delta through the dock fraction math, so
    /// the preview and the commit agree: fractions clamp to [0, 1] with a
    /// positive span, then convert back to beats.
    pub(in crate::ameui) fn clamped_trim_delta(&self, snapped: f32) -> f32 {
        let Some((track_id, clip_id)) = self.trim_clip else {
            return 0.0;
        };
        let tracks = self.tracks.get();
        let Some(clip) = tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
        else {
            return 0.0;
        };
        let span = clip.trim_end - clip.trim_start;
        if span <= 0.0 || clip.length_beats <= 0.0 {
            return 0.0;
        }
        let fraction_delta = snapped / clip.length_beats.max(0.0001) * span;
        let next = match self.trim_edge {
            TrimEdge::Left => (clip.trim_start + fraction_delta)
                .max(0.0)
                .min(clip.trim_end - 0.0001),
            TrimEdge::Right => (clip.trim_end + fraction_delta)
                .max(clip.trim_start + 0.0001)
                .min(1.0),
        };
        let orig = match self.trim_edge {
            TrimEdge::Left => clip.trim_start,
            TrimEdge::Right => clip.trim_end,
        };
        (next - orig) / span * clip.length_beats
    }

    /// Commits the dragged clip trim through the engine, mirroring the
    /// `on_trim_clip` path: one undo transaction, abort on failure,
    /// immediate layout refresh on success.
    pub(in crate::ameui) fn commit_clip_trim(&mut self) {
        let Some((track_id, clip_id)) = self.trim_clip else {
            return;
        };
        if self.trim_delta_beats.abs() <= 0.0001 {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        let tracks = self.tracks.get();
        let Some(clip) = tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
        else {
            return;
        };
        let span = clip.trim_end - clip.trim_start;
        if span <= 0.0 || clip.length_beats <= 0.0 {
            return;
        }
        let fraction_delta = self.trim_delta_beats / clip.length_beats.max(0.0001) * span;
        let (next_start, next_end) = match self.trim_edge {
            TrimEdge::Left => (
                (clip.trim_start + fraction_delta)
                    .max(0.0)
                    .min(clip.trim_end - 0.0001),
                clip.trim_end,
            ),
            TrimEdge::Right => (
                clip.trim_start,
                (clip.trim_end + fraction_delta)
                    .max(clip.trim_start + 0.0001)
                    .min(1.0),
            ),
        };
        if next_start >= next_end {
            return;
        }
        core.begin_undo_transaction("Trim Selected Clips");
        if !core.set_region_trim(track_id, clip_id as u32, next_start, next_end) {
            let _ = core.abort_undo_transaction();
            return;
        }
        if core.end_undo_transaction() {
            self.refresh_arrange_state(&core);
        }
    }

    /// Commits the piano-roll gesture as one full note-list replace,
    /// mirroring the `sync_midi_notes_to_core` path: every note
    /// keeps its authoring contract, the replace is one undo
    /// transaction, and the roll republishes from the engine after.
    pub(in crate::ameui) fn commit_piano_edit(&mut self) {
        let Some(edit) = self.piano_edit.take() else {
            return;
        };
        if self.core.is_none() {
            return;
        }
        let track_id = self.piano_track.get();
        let step = 4.0 / self.arrange_snap_division.get().max(1.0);
        let mut notes = self.piano_notes.get();
        match edit.kind {
            PianoEditKind::Add => {
                let start = (edit.orig_start / step).round() * step;
                let length = edit.delta_beats.max(step);
                notes.push(PianoNote {
                    track_id,
                    region_id: 0,
                    pitch: edit.orig_pitch,
                    midi_channel: 0,
                    articulation: 0,
                    velocity: 100,
                    start_beat: start.max(0.0),
                    length_beats: length,
                    lyric: String::new(),
                    phoneme: String::new(),
                    pitch_curve_cents: Vec::new(),
                    vibrato_depth_cents: 0,
                    vibrato_rate_millihz: 5_000,
                    portamento_samples: 0,
                    probability: 100,
                    repeat_count: 1,
                });
            }
            PianoEditKind::Move => {
                if let Some(note) = edit.note_index.and_then(|index| notes.get_mut(index)) {
                    note.start_beat = (edit.orig_start + edit.delta_beats).max(0.0);
                    note.pitch =
                        (i32::from(edit.orig_pitch) + edit.delta_pitches).clamp(0, 127) as u8;
                } else {
                    return;
                }
            }
            PianoEditKind::Resize => {
                if let Some(note) = edit.note_index.and_then(|index| notes.get_mut(index)) {
                    note.length_beats = (edit.orig_length + edit.delta_beats).max(step);
                } else {
                    return;
                }
            }
        }
        self.replace_notes(notes);
        self.selected_note.set(None);
    }

    pub(in crate::ameui) fn replace_notes(&self, notes: Vec<PianoNote>) {
        if let Some(core) = self.core.clone() {
            replace_notes(&core, &self.piano_notes, notes);
        }
    }

    /// Commits the dragged clip move through the engine, mirroring the
    /// `on_clip_moved` path: one undo transaction, sync-group aware,
    /// abort on failure, immediate layout refresh on success.
    pub(in crate::ameui) fn commit_clip_move(&mut self) {
        let Some((track_id, clip_id)) = self.move_clip else {
            return;
        };
        let delta = self.move_delta_beats;
        if delta == 0.0 {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        let tracks = self.tracks.get();
        let Some(clip) = tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
        else {
            return;
        };
        let next = (clip.start_beat + delta).max(0.0);
        core.begin_undo_transaction("Move Selected Clips");
        let moved = if clip.sync_group > 0 {
            core.move_region_sync_group(track_id, clip_id as u32, f64::from(next))
        } else {
            core.move_region(track_id, clip_id as u32, f64::from(next))
        };
        if !moved {
            let _ = core.abort_undo_transaction();
            return;
        }
        if core.end_undo_transaction() {
            self.refresh_arrange_state(&core);
        }
    }

    /// Moves the engine playhead to the beat under a canvas x. Seek only:
    /// playback state is untouched, and the next poll republishes the new
    /// playhead position into `playhead_beats`.
    pub(in crate::ameui) fn seek_arrange(&self, x: f64) -> bool {
        let Some(core) = self.core.clone() else {
            return false;
        };
        if self.arrange_geometry.px_per_beat <= 0.0 {
            return false;
        }
        let beat = self
            .arrange_geometry
            .beat_for_x(x as f32)
            .min(self.timeline_end_beats.get());
        core.set_playhead(core.beats_to_samples(f64::from(beat)));
        true
    }
}
