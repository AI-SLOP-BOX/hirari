//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Draws the selected track's notes fit-to-width, with beat grid and
    /// playhead. Read-only for now (no add/move/resize yet); note
    /// geometry comes from `piano_roll_geometry`.
    /// Scene coordinates are absolute window pixels, offset by `bounds`.
    /// Grand-staff renderer for the selected MIDI/Instrument track. Two
    /// staves with 4/4 barlines (from the engine's measure-starts list),
    /// diatonic noteheads with ledger lines and stem direction per rule,
    /// hollow heads for half+ notes, accidentals as ASCII `#`. Fontless
    /// glyphs only — no music font is required.
    pub(in crate::ameui) fn draw_notation(
        &mut self,
        scene: &mut vello::Scene,
        bounds: CanvasBounds,
        context: &CanvasContext,
    ) {
        let x0 = f64::from(bounds.x);
        let y0 = f64::from(bounds.y);
        let width = f64::from(bounds.width);
        let height = f64::from(bounds.height);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            themed(context.theme().background, 255),
            None,
            &Rect::new(x0, y0, x0 + width, y0 + height),
        );
        const WINDOW_BEATS: f64 = 64.0;
        let px_per_beat = width / WINDOW_BEATS;
        let track_id = self.piano_track.get();
        let line_w = 6.0;
        let treble_bottom = y0 + height * 0.40;
        let treble_top = treble_bottom - 4.0 * line_w;
        let bass_bottom = y0 + height * 0.90;
        let bass_top = bass_bottom - 4.0 * line_w;
        let staff_color = themed(context.theme().surface_border, 200);
        let staff_s = Stroke::new(1.25);
        for line in 0..5 {
            let y = treble_top + f64::from(line) * line_w;
            scene.stroke(
                &staff_s,
                Affine::IDENTITY,
                staff_color,
                None,
                &Line::new((x0, y), (x0 + width, y)),
            );
        }
        for line in 0..5 {
            let y = bass_top + f64::from(line) * line_w;
            scene.stroke(
                &staff_s,
                Affine::IDENTITY,
                staff_color,
                None,
                &Line::new((x0, y), (x0 + width, y)),
            );
        }
        let bar_color = themed(context.theme().text, 130);
        let bar_s = Stroke::new(1.0);
        for measure in self.measure_starts.get() {
            let beat = f64::from(measure.beat);
            if beat <= 0.0 || beat >= WINDOW_BEATS {
                continue;
            }
            let x = x0 + beat * px_per_beat;
            scene.stroke(
                &bar_s,
                Affine::IDENTITY,
                bar_color,
                None,
                &Line::new((x, treble_top), (x, bass_bottom)),
            );
        }
        context.draw_text(
            scene,
            &bounds,
            "TREBLE",
            x0 + 3.0,
            treble_top - 11.0,
            10.0,
            context.theme().secondary_text,
            None,
        );
        context.draw_text(
            scene,
            &bounds,
            "BASS",
            x0 + 3.0,
            bass_top - 11.0,
            10.0,
            context.theme().secondary_text,
            None,
        );
        // Diatonic steps from C4 (C4 == 0), 7 per octave.
        const DIATONIC: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
        fn diatonic_steps(pitch: u8) -> i32 {
            ((pitch / 12) as i32 - 5) * 7 + DIATONIC[(pitch % 12) as usize]
        }
        let ledger = Stroke::new(1.25);
        let note_stroke = Stroke::new(1.0);
        let head_radius = 4.5;
        let note_color = themed(context.theme().text, 235);
        let staff_background = themed(context.theme().background, 255);
        let notes = self.piano_notes.get();
        for note in notes.iter() {
            if note.track_id != track_id {
                continue;
            }
            let beat = f64::from(note.start_beat.max(0.0));
            if beat >= WINDOW_BEATS {
                continue;
            }
            let x = x0 + beat * px_per_beat;
            let steps = diatonic_steps(note.pitch);
            let on_treble = steps >= 2;
            let y = if on_treble {
                treble_top + (10.0 - steps as f64) * (line_w / 2.0)
            } else {
                bass_top + (-2.0 - steps as f64) * (line_w / 2.0)
            };
            // Ledger lines on even diatonic steps beyond each staff.
            let mut line_step = if on_treble { 12 } else { 0 };
            while line_step <= steps {
                let line_y = if on_treble {
                    treble_top + (10.0 - line_step as f64) * (line_w / 2.0)
                } else {
                    bass_top + (-2.0 - line_step as f64) * (line_w / 2.0)
                };
                scene.stroke(
                    &ledger,
                    Affine::IDENTITY,
                    staff_color,
                    None,
                    &Line::new((x - 6.0, line_y), (x + 6.0, line_y)),
                );
                line_step += 2;
            }
            let mut line_step = if on_treble { 0 } else { -12 };
            while line_step >= steps {
                let line_y = if on_treble {
                    treble_top + (10.0 - line_step as f64) * (line_w / 2.0)
                } else {
                    bass_top + (-2.0 - line_step as f64) * (line_w / 2.0)
                };
                scene.stroke(
                    &ledger,
                    Affine::IDENTITY,
                    staff_color,
                    None,
                    &Line::new((x - 6.0, line_y), (x + 6.0, line_y)),
                );
                line_step -= 2;
            }
            // Notehead: filled for < 1 beat, else hollow; whole notes drop the
            // stem entirely.
            let whole = note.length_beats >= 4.0;
            let filled = note.length_beats < 1.0;
            let ellipse = vello::kurbo::Ellipse::new(
                (x, y),
                vello::kurbo::Vec2::new(head_radius * 0.72, head_radius),
                0.0,
            );
            if filled {
                scene.fill(Fill::NonZero, Affine::IDENTITY, note_color, None, &ellipse);
            } else {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    staff_background,
                    None,
                    &ellipse,
                );
                scene.stroke(&note_stroke, Affine::IDENTITY, note_color, None, &ellipse);
            }
            // Stem direction from the middle line down convention.
            let stem_up = if on_treble { steps < 6 } else { steps >= -6 };
            if !whole {
                let stem_len = 22.0;
                if stem_up {
                    scene.stroke(
                        &note_stroke,
                        Affine::IDENTITY,
                        note_color,
                        None,
                        &Line::new(
                            (x + head_radius * 0.7, y),
                            (x + head_radius * 0.7, y - stem_len),
                        ),
                    );
                } else {
                    scene.stroke(
                        &note_stroke,
                        Affine::IDENTITY,
                        note_color,
                        None,
                        &Line::new(
                            (x - head_radius * 0.7, y),
                            (x - head_radius * 0.7, y + stem_len),
                        ),
                    );
                }
            }
            // Accidental marker for black keys (ASCII #).
            if matches!(note.pitch % 12, 1 | 3 | 6 | 8 | 10) {
                context.draw_text(
                    scene,
                    &bounds,
                    "#",
                    x - 12.0,
                    y - 4.0,
                    11.0,
                    context.theme().text,
                    None,
                );
            }
        }
    }
}
