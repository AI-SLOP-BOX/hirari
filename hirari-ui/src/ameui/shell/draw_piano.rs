//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    pub(in crate::ameui) fn draw_piano(
        &mut self,
        scene: &mut vello::Scene,
        bounds: CanvasBounds,
        context: &CanvasContext,
    ) {
        let x0 = f64::from(bounds.x);
        let y0 = f64::from(bounds.y);
        let _ = y0;
        let width = f64::from(bounds.width);
        let height = f64::from(bounds.height);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let track_id = self.piano_track.get();
        let notes = self.piano_notes.get();
        let drum = self.drum_mode.get();
        let visible: Vec<PianoNote> = notes
            .iter()
            .filter(|note| note.track_id == track_id && !(drum && !(35..=81).contains(&note.pitch)))
            .cloned()
            .collect();
        let timeline_end = self.timeline_end_beats.get();
        let geometry = piano_roll_geometry(bounds.width, bounds.height, timeline_end, &visible);
        self.piano_geometry = geometry;
        let grid = Stroke::new(1.0);
        let x_for_beat = |beat: f32| x0 + f64::from(geometry.x_for_beat(beat));

        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            themed(context.theme().background, 255),
            None,
            &Rect::new(x0, y0, x0 + width, y0 + height),
        );
        let step = arrange_canvas_geometry(bounds.width, bounds.height, timeline_end, 1, 1.0, 0.0)
            .beat_step;
        let steps = (f64::from(timeline_end) / f64::from(step)).ceil() as u32;
        for index in 0..=steps {
            let x = x_for_beat(index as f32 * step);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                themed(context.theme().surface_border, 45),
                None,
                &Line::new((x, y0), (x, y0 + height)),
            );
        }
        const PITCH_NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        for pitch in geometry.bottom_pitch..=geometry.top_pitch {
            let y = y0 + f64::from(geometry.y_for_pitch(pitch));
            let stronger = pitch % 12 == 0;
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                themed(
                    context.theme().surface_border,
                    if stronger { 90 } else { 35 },
                ),
                None,
                &Line::new((x0, y), (x0 + width, y)),
            );
            if drum {
                context.draw_text(
                    scene,
                    &bounds,
                    hirari_core_bridge::piano_roll_editor::drum_lane_label(pitch),
                    x0 + 2.0,
                    y + 1.0,
                    9.0,
                    context.theme().secondary_text,
                    None,
                );
            } else if stronger {
                let label = format!("{}{}", PITCH_NAMES[(pitch % 12) as usize], pitch / 12 - 1);
                context.draw_text(
                    scene,
                    &bounds,
                    &label,
                    x0 + 2.0,
                    y + 1.0,
                    9.0,
                    context.theme().secondary_text,
                    None,
                );
            }
        }
        // Notes are drawn from the full list so the edit preview can
        // find its note by global index; the preview (move/resize/add)
        // replaces the committed geometry until release.
        let step = 4.0 / self.arrange_snap_division.get().max(1.0);
        let mut draw_note = |pitch: u8, start_beat: f32, length_beats: f32, velocity: u8| {
            let x = x_for_beat(start_beat);
            let note_width = (f64::from(length_beats) * f64::from(geometry.px_per_beat)).max(2.0);
            let y = y0 + f64::from(geometry.y_for_pitch(pitch));
            let row_height = f64::from(geometry.px_per_pitch);
            let alpha = 90.0 + f64::from(velocity) * 80.0 / 127.0;
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgba8(139, 124, 255, alpha as u8),
                None,
                &RoundedRect::new(x, y + 1.0, x + note_width, y + row_height - 1.0, 2.0),
            );
        };
        for (global_index, note) in notes.iter().enumerate() {
            if note.track_id != track_id || (drum && !(35..=81).contains(&note.pitch)) {
                continue;
            }
            let mut pitch = note.pitch;
            let mut start_beat = note.start_beat;
            let mut length_beats = note.length_beats;
            if let Some(edit) = &self.piano_edit {
                if edit.note_index == Some(global_index) {
                    match edit.kind {
                        PianoEditKind::Move => {
                            start_beat = (edit.orig_start + edit.delta_beats).max(0.0);
                            pitch = (i32::from(edit.orig_pitch) + edit.delta_pitches).clamp(0, 127)
                                as u8;
                        }
                        PianoEditKind::Resize => {
                            length_beats = (edit.orig_length + edit.delta_beats).max(step);
                        }
                        PianoEditKind::Add => {}
                    }
                }
            }
            draw_note(pitch, start_beat, length_beats, note.velocity);
        }
        if let Some(edit) = &self.piano_edit {
            if matches!(edit.kind, PianoEditKind::Add) {
                let start = (edit.orig_start / step).round() * step;
                draw_note(
                    edit.orig_pitch,
                    start.max(0.0),
                    edit.delta_beats.max(step),
                    100,
                );
            }
        }
        let playhead = self.playhead_beats.get();
        if (0.0..=f64::from(timeline_end)).contains(&playhead) {
            let x = x_for_beat(playhead as f32);
            scene.stroke(
                &Stroke::new(2.0),
                Affine::IDENTITY,
                Color::from_rgb8(48, 209, 88),
                None,
                &Line::new((x, y0), (x, y0 + height)),
            );
        }
    }
}
