//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Piano-roll gestures: press a note to move it (right edge resizes),
    /// press empty space to add one, drag to preview, release to commit.
    /// The arrange edit lock holds edits here too (selection only).
    pub(in crate::ameui) fn on_piano_pointer(&mut self, event: CanvasPointerEvent) -> bool {
        let track_id = self.piano_track.get();
        let geometry = self.piano_geometry;
        // Edits and previews must redraw: bump the dirty generation.
        self.dirty_piano
            .set((track_id, self.dirty_piano.get().1.wrapping_add(1)));
        match event.phase {
            CanvasPointerPhase::Down => {
                let notes = self.piano_notes.get();
                let drum = self.drum_mode.get();
                let visible: Vec<(usize, PianoNote)> = notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| {
                        note.track_id == track_id && !(drum && !(35..=81).contains(&note.pitch))
                    })
                    .map(|(index, note)| (index, note.clone()))
                    .collect();
                let visible_notes: Vec<PianoNote> =
                    visible.iter().map(|(_, note)| note.clone()).collect();
                let hit = note_at(&visible_notes, &geometry, event.x as f32, event.y as f32);
                match hit {
                    Some(visible_index) => {
                        let (global_index, note) = visible[visible_index].clone();
                        self.selected_note.set(Some(global_index));
                        if self.arrange_locked.get() {
                            return true;
                        }
                        let right_edge = geometry.x_for_beat(note.start_beat + note.length_beats);
                        let kind = if (event.x as f32 - right_edge).abs() <= 6.0 {
                            PianoEditKind::Resize
                        } else {
                            PianoEditKind::Move
                        };
                        self.piano_edit = Some(PianoEdit {
                            kind,
                            note_index: Some(global_index),
                            down_x: event.x,
                            down_y: event.y,
                            orig_pitch: note.pitch,
                            orig_start: note.start_beat,
                            orig_length: note.length_beats,
                            delta_beats: 0.0,
                            delta_pitches: 0,
                        });
                        true
                    }
                    None => {
                        if self.arrange_locked.get() {
                            return false;
                        }
                        self.piano_edit = Some(PianoEdit {
                            kind: PianoEditKind::Add,
                            note_index: None,
                            down_x: event.x,
                            down_y: event.y,
                            orig_pitch: geometry.pitch_at_y(event.y as f32),
                            orig_start: geometry.beat_at_x(event.x as f32),
                            orig_length: 0.0,
                            delta_beats: 0.0,
                            delta_pitches: 0,
                        });
                        true
                    }
                }
            }
            CanvasPointerPhase::Moved => {
                let Some(edit) = self.piano_edit.as_mut() else {
                    return false;
                };
                let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                let raw_beats = if geometry.px_per_beat > 0.0 {
                    ((event.x - edit.down_x) / f64::from(geometry.px_per_beat)) as f32
                } else {
                    0.0
                };
                edit.delta_beats = (raw_beats / step).round() * step;
                if geometry.px_per_pitch > 0.0 {
                    edit.delta_pitches =
                        ((edit.down_y - event.y) / f64::from(geometry.px_per_pitch)).round() as i32;
                }
                true
            }
            CanvasPointerPhase::Up => {
                if self.piano_edit.is_some() {
                    self.commit_piano_edit();
                }
                true
            }
            CanvasPointerPhase::Cancelled => {
                self.piano_edit = None;
                true
            }
        }
    }
}
