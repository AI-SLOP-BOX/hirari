//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Draws the arrangement (ruler, lanes, clips, markers, playhead) as
    /// vector shapes. This is the read-only parity step: geometry matches
    /// the dock, but there is no zoom/scroll, selection or drag yet,
    /// and no text inside the canvas (lane names live in the list below).
    /// Scene coordinates are absolute window pixels, offset by `bounds`.
    pub(in crate::ameui) fn draw_arrange(
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
        self.arrange_bounds = (bounds.x, bounds.y, bounds.width, bounds.height);
        let tracks = self.tracks.get();
        let selected_clip = self.selected_clip.get();
        let peaks = self.clip_peaks.get();
        let markers = self.markers.get();
        let measure_starts = self.measure_starts.get();
        let timeline_end = self.timeline_end_beats.get();
        // The TimelineView ruler above the canvas owns the ruler band;
        // the canvas draws only the lanes below it, so the ruler band is
        // collapsed out of the canvas geometry.
        let mut geometry = arrange_canvas_geometry(
            bounds.width,
            bounds.height,
            timeline_end,
            tracks.len(),
            self.arrange_zoom.get(),
            self.arrange_scroll_beats.get(),
        );
        if tracks.len() > 0 {
            geometry.ruler_height = 0.0;
            geometry.lane_height = bounds.height.max(0.0) / tracks.len() as f32;
        }
        self.arrange_geometry = geometry;
        let ruler = f64::from(geometry.ruler_height);
        let lane_height = f64::from(geometry.lane_height);
        let px_per_beat = f64::from(geometry.px_per_beat);
        let x_for_beat = |beat: f32| x0 + f64::from(geometry.x_for_beat(beat));
        let grid = Stroke::new(1.0);
        let selected_grid = Stroke::new(2.0);

        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            themed(context.theme().background, 255),
            None,
            &Rect::new(x0, y0, x0 + width, y0 + height),
        );

        // Lane-header column (104px): name band for the text labels.
        let header = f64::from(geometry.header_width);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            themed(context.theme().surface, 255),
            None,
            &Rect::new(x0, y0, x0 + header, y0 + height),
        );
        scene.stroke(
            &grid,
            Affine::IDENTITY,
            themed(context.theme().surface_border, 255),
            None,
            &Line::new((x0 + header, y0), (x0 + header, y0 + height)),
        );
        let theme_text = context.theme().text;
        let theme_secondary = context.theme().secondary_text;

        // Faint beat grid, then stronger measure lines (opacities).
        let steps = (f64::from(timeline_end) / f64::from(geometry.beat_step)).ceil() as u32;
        for step in 0..=steps {
            let x = x_for_beat(step as f32 * geometry.beat_step);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                themed(context.theme().surface_border, 45),
                None,
                &Line::new((x, y0 + ruler), (x, y0 + height)),
            );
        }
        for measure in &measure_starts {
            let x = x_for_beat(measure.beat);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                themed(context.theme().surface_border, 140),
                None,
                &Line::new((x, y0), (x, y0 + height)),
            );
        }

        for (index, track) in tracks.iter().enumerate() {
            let top = y0 + f64::from(geometry.y_for_lane(index));
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                themed(context.theme().surface_border, 255),
                None,
                &Line::new((x0, top), (x0 + width, top)),
            );
            // Lane color strip, as in the header.
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgb8(track.color[0], track.color[1], track.color[2]),
                None,
                &Rect::new(x0, top, x0 + 3.0, top + lane_height),
            );
            let name = context.elide_text(&track.name, (header - 12.0) as f32, 11.0, &bounds, None);
            context.draw_text(
                scene,
                &bounds,
                &name,
                x0 + 8.0,
                top + 5.0,
                11.0,
                theme_text,
                None,
            );
            let sub = context.elide_text(
                &format!("{} · {} clips", track.kind, track.clips.len()),
                (header - 12.0) as f32,
                9.0,
                &bounds,
                None,
            );
            context.draw_text(
                scene,
                &bounds,
                &sub,
                x0 + 8.0,
                top + 19.0,
                9.0,
                theme_secondary,
                None,
            );
            let clip_height = (lane_height - 4.0).clamp(2.0, 42.0);
            for clip in &track.clips {
                let selected = selected_clip == Some((track.id, clip.id));
                // Live move preview: the dragged clip follows the pointer
                // until release commits it (`drag-current-delta`).
                let drag_delta = if self.move_clip == Some((track.id, clip.id)) {
                    self.move_delta_beats
                } else {
                    0.0
                };
                let mut clip_x = x_for_beat(clip.start_beat + drag_delta);
                let mut clip_width = (f64::from(clip.length_beats) * px_per_beat).max(4.0) - 2.0;
                if self.trim_clip == Some((track.id, clip.id)) {
                    match self.trim_edge {
                        TrimEdge::Left => {
                            clip_x += f64::from(self.trim_delta_beats) * px_per_beat;
                            clip_width -= f64::from(self.trim_delta_beats) * px_per_beat;
                        }
                        TrimEdge::Right => {
                            clip_width += f64::from(self.trim_delta_beats) * px_per_beat;
                        }
                    }
                    clip_width = clip_width.max(2.0);
                }
                let clip_y = top + (lane_height - clip_height) / 2.0;
                let rect = RoundedRect::new(
                    clip_x,
                    clip_y,
                    clip_x + clip_width,
                    clip_y + clip_height,
                    3.0,
                );
                // selection: brighter fill and a 2px border.
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::from_rgba8(
                        track.color[0],
                        track.color[1],
                        track.color[2],
                        if selected { 185 } else { 115 },
                    ),
                    None,
                    &rect,
                );
                scene.stroke(
                    if selected { &selected_grid } else { &grid },
                    Affine::IDENTITY,
                    if selected {
                        Color::from_rgb8(235, 239, 244)
                    } else {
                        Color::from_rgb8(track.color[0], track.color[1], track.color[2])
                    },
                    None,
                    &rect,
                );
                if clip_width >= 24.0 {
                    // Elision and vertical centering come from the landed
                    // draw_text_in_rect; no local label math needed.
                    context.draw_text_in_rect(
                        scene,
                        &bounds,
                        &clip.name,
                        clip_x + 4.0,
                        clip_y,
                        clip_width - 8.0,
                        clip_height,
                        10.0,
                        theme_text,
                        None,
                        CanvasTextVAlign::Center,
                    );
                }
                // Waveform peaks, as in the dock: 128 pairs windowed
                // by trim (and flipped when reversed), drawn only when the
                // clip is wide enough to read (48px gate, as in the dock).
                if let Some(points) = peaks.get(&(track.id, clip.id)) {
                    if points.len() >= 256 && clip_width >= 48.0 {
                        let span = clip.trim_end - clip.trim_start;
                        let mid_y = clip_y + clip_height / 2.0;
                        let amp = clip_height / 3.0;
                        for pair in 0..128 {
                            let fraction = (pair as f32 + 0.5) / 128.0;
                            let source = if clip.reverse {
                                ((clip.trim_end - fraction * span) * 128.0).floor()
                            } else {
                                ((clip.trim_start + fraction * span) * 128.0).floor()
                            }
                            .min(127.0) as usize;
                            if points.len() < (source + 1) * 2 {
                                continue;
                            }
                            let x = clip_x + 5.0 + pair as f64 * (clip_width - 10.0) / 128.0;
                            let peak = f64::from(points[source * 2]);
                            let height =
                                f64::from(points[source * 2] - points[source * 2 + 1]) * amp;
                            scene.stroke(
                                &grid,
                                Affine::IDENTITY,
                                Color::from_rgba8(235, 239, 244, 143),
                                None,
                                &Line::new(
                                    (x, mid_y - peak * amp),
                                    (x, mid_y - peak * amp + height),
                                ),
                            );
                        }
                    }
                }
            }
        }

        // Automation overlays: volume/pan polylines through each lane,
        // mirroring the automation values (read-only lanes until
        // AUTO mode; in AUTO mode the gesture preview replaces them).
        for (index, track) in tracks.iter().enumerate() {
            let top = y0 + f64::from(geometry.y_for_lane(index));
            let inner = (lane_height - 8.0).max(4.0);
            let preview_points =
                |lane: u32,
                 points: &[crate::ui_shared::arrange_model::AutomationPoint]|
                 -> Vec<crate::ui_shared::arrange_model::AutomationPoint> {
                    let mut points = points.to_vec();
                    if let Some(edit) = &self.auto_edit {
                        if edit.track_id == track.id && edit.lane == lane {
                            match edit.point_index {
                                Some(point_index) => {
                                    if let Some(point) = points.get_mut(point_index) {
                                        point.beat = (edit.orig_beat + edit.delta_beats).max(0.0);
                                        point.value += edit.delta_value;
                                    }
                                }
                                None => {
                                    points.push(crate::ui_shared::arrange_model::AutomationPoint {
                                        beat: edit.orig_beat.max(0.0),
                                        value: (edit.orig_value + edit.delta_value).clamp(
                                            if lane == 0 { 0.0 } else { -1.0 },
                                            if lane == 0 { 2.0 } else { 1.0 },
                                        ),
                                    });
                                    points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                                }
                            }
                        }
                    }
                    points
                };
            let volume_points = preview_points(0, &track.volume_automation);
            let pan_points = preview_points(1, &track.pan_automation);
            let mut draw_automation =
                |points: &[crate::ui_shared::arrange_model::AutomationPoint],
                 min_value: f32,
                 max_value: f32,
                 color: Color| {
                    let span = (max_value - min_value).max(f32::EPSILON);
                    for pair in points.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let y = |value: f32| {
                            top + 4.0 + f64::from(inner) * (1.0 - (value - min_value) / span) as f64
                        };
                        scene.stroke(
                            &grid,
                            Affine::IDENTITY,
                            color,
                            None,
                            &Line::new(
                                (x_for_beat(a.beat), y(a.value)),
                                (x_for_beat(b.beat), y(b.value)),
                            ),
                        );
                    }
                };
            if !volume_points.is_empty() {
                draw_automation(
                    &volume_points,
                    0.0,
                    2.0,
                    Color::from_rgba8(240, 180, 90, 220),
                );
            }
            if !pan_points.is_empty() {
                draw_automation(
                    &pan_points,
                    -1.0,
                    1.0,
                    Color::from_rgba8(120, 220, 240, 200),
                );
            }
        }

        for marker in &markers {
            let x = x_for_beat(marker.beat as f32);
            let color = Color::from_rgb8(marker.color[0], marker.color[1], marker.color[2]);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                Color::from_rgba8(marker.color[0], marker.color[1], marker.color[2], 90),
                None,
                &Line::new((x, y0 + ruler), (x, y0 + height)),
            );
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                color,
                None,
                &Rect::new(x - 3.0, y0 + 4.0, x + 3.0, y0 + 12.0),
            );
        }

        // Style.playhead (#30D158), drawn only inside the timeline.
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
