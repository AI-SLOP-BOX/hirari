//! Canvas interaction + drawing helpers for the AmeUI shell.
use crate::ameui::*;

impl HirariAmeApp {
    pub(in crate::ameui) fn canvas_pointer_event_impl(
        &mut self,
        canvas_id: &str,
        event: CanvasPointerEvent,
    ) -> bool {
        if canvas_id == "piano" {
            return self.on_piano_pointer(event);
        }
        if canvas_id != "arrange" {
            return false;
        }
        match event.phase {
            CanvasPointerPhase::Down => {
                // Double-click splits the clip under the cursor at the
                // click beat (the landed click_count makes the second
                // press reliable instead of arming a move/seek).
                if event.click_count >= 2
                    && (event.y as f32) >= self.arrange_geometry.ruler_height
                    && (event.x as f32) >= self.arrange_geometry.header_width
                {
                    let tracks = self.tracks.get();
                    if let Some((track_id, clip_id)) = clip_at(
                        &tracks,
                        &self.arrange_geometry,
                        event.x as f32,
                        event.y as f32,
                    ) {
                        if let Some(core) = self.core.clone() {
                            let beat = f64::from(self.arrange_geometry.beat_for_x(event.x as f32));
                            if core.split_region(track_id, clip_id as u32, beat) {
                                self.selected_clip.set(Some((track_id, clip_id)));
                                self.status.set(format!("CLIP SPLIT AT {beat:.2} BEATS"));
                                self.refresh_arrange_state(&core);
                            }
                        }
                        return true;
                    }
                }
                if (event.y as f32) < self.arrange_geometry.ruler_height {
                    // Ruler drag pans; lane drags seek (slice 4).
                    self.panning = true;
                    self.pan_last_x = event.x;
                    true
                } else if (event.y as f32) >= self.arrange_geometry.ruler_height
                    && (event.x as f32) >= self.arrange_geometry.header_width
                    && self.auto_mode.get()
                {
                    // Automation mode: press near a point to move it,
                    // elsewhere on the lane to add one (parity).
                    if self.arrange_locked.get() {
                        return true;
                    }
                    let tracks = self.tracks.get();
                    let lane_index = ((event.y as f32 - self.arrange_geometry.ruler_height)
                        / self.arrange_geometry.lane_height)
                        as usize;
                    let lane = self.auto_lane.get();
                    let mut found: Option<AutoEdit> = None;
                    if let Some(track) = tracks.get(lane_index) {
                        let points: &[crate::ui_shared::arrange_model::AutomationPoint] = match lane
                        {
                            0 => &track.volume_automation,
                            _ => &track.pan_automation,
                        };
                        let local_y = event.y as f32;
                        let lane_top = self.arrange_geometry.ruler_height
                            + lane_index as f32 * self.arrange_geometry.lane_height;
                        let inner = (self.arrange_geometry.lane_height - 8.0).max(4.0);
                        let (min_value, max_value) =
                            if lane == 0 { (0.0, 2.0) } else { (-1.0, 1.0) };
                        let span = max_value - min_value;
                        let nearest = points.iter().enumerate().min_by(|(_, a), (_, b)| {
                            let da = ((a.beat - self.arrange_geometry.beat_for_x(event.x as f32))
                                * self.arrange_geometry.px_per_beat)
                                .abs();
                            let db = ((b.beat - self.arrange_geometry.beat_for_x(event.x as f32))
                                * self.arrange_geometry.px_per_beat)
                                .abs();
                            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        if let Some((point_index, point)) = nearest {
                            let py =
                                lane_top + 4.0 + inner * (1.0 - (point.value - min_value) / span);
                            if (py - local_y).abs() <= 6.0 {
                                found = Some(AutoEdit {
                                    track_id: track.id,
                                    lane,
                                    point_index: Some(point_index),
                                    down_x: event.x,
                                    down_y: event.y,
                                    orig_beat: point.beat,
                                    orig_value: point.value,
                                    delta_beats: 0.0,
                                    delta_value: 0.0,
                                });
                                self.auto_point.set(Some(point_index));
                                self.auto_track.set(track.id);
                            }
                        }
                        if found.is_none() {
                            let beat = self.arrange_geometry.beat_for_x(event.x as f32);
                            let value =
                                min_value + span * (1.0 - ((local_y - lane_top - 4.0) / inner));
                            self.auto_track.set(track.id);
                            found = Some(AutoEdit {
                                track_id: track.id,
                                lane,
                                point_index: None,
                                down_x: event.x,
                                down_y: event.y,
                                orig_beat: beat,
                                orig_value: value.clamp(min_value, max_value),
                                delta_beats: 0.0,
                                delta_value: 0.0,
                            });
                        }
                    }
                    if let Some(edit) = found {
                        self.auto_edit = Some(edit);
                        return true;
                    }
                    return false;
                } else {
                    // Provisional until the click threshold is crossed:
                    // a clip press selects immediately (parity) and
                    // arms a move; empty space arms a seek.
                    self.seeking = true;
                    self.down_x = event.x;
                    self.down_y = event.y;
                    let tracks = self.tracks.get();
                    self.down_clip = clip_at(
                        &tracks,
                        &self.arrange_geometry,
                        event.x as f32,
                        event.y as f32,
                    );
                    match self.down_clip {
                        Some((track_id, clip_id)) => {
                            self.selected_clip.set(Some((track_id, clip_id)));
                            let clip = tracks.iter().find(|track| track.id == track_id).and_then(
                                |track| track.clips.iter().find(|clip| clip.id == clip_id),
                            );
                            let track_kind = tracks
                                .iter()
                                .find(|track| track.id == track_id)
                                .map(|track| track.kind.clone());
                            match clip {
                                Some(_) if self.arrange_locked.get() => {
                                    self.down_mode = LaneDrag::Hold
                                }
                                Some(clip) if clip.midi => self.down_mode = LaneDrag::Hold,
                                Some(clip) => {
                                    // Edge zones trim (Audio/Vocal only, as
                                    // in the dock); the body moves.
                                    let edge = track_kind
                                        .as_deref()
                                        .filter(|kind| *kind == "Audio" || *kind == "Vocal")
                                        .and_then(|_| {
                                            trim_edge_at(
                                                &self.arrange_geometry,
                                                clip,
                                                event.x as f32,
                                            )
                                        });
                                    match edge {
                                        Some(TrimEdge::Left) => {
                                            self.down_mode = LaneDrag::TrimLeft;
                                            self.trim_clip = Some((track_id, clip_id));
                                            self.trim_edge = TrimEdge::Left;
                                            self.trim_delta_beats = 0.0;
                                        }
                                        Some(TrimEdge::Right) => {
                                            self.down_mode = LaneDrag::TrimRight;
                                            self.trim_clip = Some((track_id, clip_id));
                                            self.trim_edge = TrimEdge::Right;
                                            self.trim_delta_beats = 0.0;
                                        }
                                        None => {
                                            self.down_mode = LaneDrag::Move;
                                            self.move_clip = Some((track_id, clip_id));
                                            self.move_start_beat = clip.start_beat;
                                            self.move_delta_beats = 0.0;
                                        }
                                    }
                                }
                                None => self.down_mode = LaneDrag::Seek,
                            }
                        }
                        None => self.down_mode = LaneDrag::Seek,
                    }
                    true
                }
            }
            CanvasPointerPhase::Moved => {
                if self.auto_edit.is_some() {
                    if let Some(edit) = self.auto_edit.as_mut() {
                        let px_per_beat = self.arrange_geometry.px_per_beat;
                        let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                        let raw_beats = if px_per_beat > 0.0 {
                            ((event.x - edit.down_x) / f64::from(px_per_beat)) as f32
                        } else {
                            0.0
                        };
                        edit.delta_beats = (raw_beats / step).round() * step;
                        // Value delta from vertical travel, sign follows the
                        // drawn mapping (up raises the value). Both lanes
                        // span 2 value units, so the scale is uniform.
                        let inner = (self.arrange_geometry.lane_height - 8.0).max(4.0);
                        if inner > 0.0 {
                            edit.delta_value = ((edit.down_y - event.y) as f32 / inner) * 2.0;
                        }
                    }
                    return true;
                }
                if self.panning {
                    let px_per_beat = self.arrange_geometry.px_per_beat;
                    if px_per_beat > 0.0 {
                        let delta_beats =
                            ((event.x - self.pan_last_x) / f64::from(px_per_beat)) as f32;
                        self.arrange_scroll_beats
                            .set((self.arrange_scroll_beats.get() - delta_beats).max(0.0));
                    }
                    self.pan_last_x = event.x;
                    true
                } else if self.seeking {
                    match self.down_mode {
                        LaneDrag::Hold => true,
                        LaneDrag::TrimLeft | LaneDrag::TrimRight => {
                            if self.click_distance(&event) <= 4.0 {
                                true
                            } else {
                                let px_per_beat = self.arrange_geometry.px_per_beat;
                                if px_per_beat > 0.0 {
                                    let raw =
                                        ((event.x - self.down_x) / f64::from(px_per_beat)) as f32;
                                    let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                                    let snapped = (raw / step).round() * step;
                                    // Clamp the preview delta through the
                                    // same fraction math the commit uses.
                                    self.trim_delta_beats = self.clamped_trim_delta(snapped);
                                }
                                true
                            }
                        }
                        LaneDrag::Move => {
                            if self.click_distance(&event) <= 4.0 {
                                true
                            } else {
                                let px_per_beat = self.arrange_geometry.px_per_beat;
                                if px_per_beat > 0.0 {
                                    let raw =
                                        ((event.x - self.down_x) / f64::from(px_per_beat)) as f32;
                                    let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                                    let snapped = (raw / step).round() * step;
                                    self.move_delta_beats = snapped.max(-self.move_start_beat);
                                }
                                true
                            }
                        }
                        LaneDrag::Seek => {
                            if self.click_distance(&event) <= 4.0 {
                                // Still a click candidate; seek on release.
                                true
                            } else {
                                self.seek_arrange(event.x)
                            }
                        }
                    }
                } else {
                    false
                }
            }
            CanvasPointerPhase::Up | CanvasPointerPhase::Cancelled => {
                if self.auto_edit.is_some() {
                    if event.phase == CanvasPointerPhase::Up {
                        self.commit_auto_edit();
                    } else {
                        self.auto_edit = None;
                    }
                    return true;
                }
                if event.phase == CanvasPointerPhase::Up {
                    match self.down_mode {
                        LaneDrag::Move => self.commit_clip_move(),
                        LaneDrag::TrimLeft | LaneDrag::TrimRight => self.commit_clip_trim(),
                        LaneDrag::Hold => {}
                        LaneDrag::Seek => {
                            if self.seeking && self.click_distance(&event) <= 4.0 {
                                // Click on empty lane space seeks on release.
                                self.seek_arrange(self.down_x);
                            }
                        }
                    }
                }
                self.seeking = false;
                self.panning = false;
                self.down_clip = None;
                self.move_clip = None;
                self.move_delta_beats = 0.0;
                self.trim_clip = None;
                self.trim_delta_beats = 0.0;
                true
            }
        }
    }

    pub(in crate::ameui) fn draw_canvas_impl(
        &mut self,
        canvas_id: &str,
        scene: &mut vello::Scene,
        bounds: CanvasBounds,
        context: &mut CanvasContext<'_>,
        _viewport: Viewport,
        _tree: &ViewTree,
    ) {
        if canvas_id == "arrange" {
            self.draw_arrange(scene, bounds, context);
            return;
        }
        if canvas_id == "notation" {
            self.draw_notation(scene, bounds, context);
            return;
        }
        if canvas_id == "piano" {
            self.draw_piano(scene, bounds, context);
            return;
        }
        if canvas_id == "video" {
            let Some(frame) = self.video_frame.take() else {
                return;
            };
            let Some((width, height)) = self.video_size else {
                return;
            };
            let texture = self.video_reusable.take().unwrap_or_else(|| {
                context.create_render_target(
                    width,
                    height,
                    Some("hirari-video"),
                    wgpu::TextureUsages::COPY_DST,
                )
            });
            context.queue().write_texture(
                texture.as_image_copy(),
                &frame,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if let Some(image) = &self.video_image {
                match context.replace_texture(image, texture) {
                    Ok(previous) => self.video_reusable = Some(previous),
                    Err(error) => {
                        eprintln!("could not refresh video canvas: {error}");
                        return;
                    }
                }
            } else {
                match context.register_texture(texture) {
                    Ok(image) => self.video_image = Some(image),
                    Err(error) => {
                        eprintln!("could not register video canvas: {error}");
                        return;
                    }
                }
            }
            if let Some(image) = &self.video_image {
                context.draw_texture(scene, image, bounds);
            }
            return;
        }
        if canvas_id == "spectrum" {
            let frame = self.plots.snapshot();
            let pixels = plot_raster::spectrum_rgba(&frame);
            let width = plot_raster::WIDTH;
            let height = 96u32;
            let texture = self.spectrum_reusable.take().unwrap_or_else(|| {
                context.create_render_target(
                    width,
                    height,
                    Some("hirari-spectrum"),
                    wgpu::TextureUsages::COPY_DST,
                )
            });
            context.queue().write_texture(
                texture.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if let Some(image) = &self.spectrum_image {
                match context.replace_texture(image, texture) {
                    Ok(previous) => self.spectrum_reusable = Some(previous),
                    Err(error) => {
                        eprintln!("could not refresh spectrum canvas: {error}");
                        return;
                    }
                }
            } else {
                match context.register_texture(texture) {
                    Ok(image) => self.spectrum_image = Some(image),
                    Err(error) => {
                        eprintln!("could not register spectrum canvas: {error}");
                        return;
                    }
                }
            }
            if let Some(image) = &self.spectrum_image {
                context.draw_texture(scene, image, bounds);
            }
            return;
        }
        if canvas_id != "waveform" {
            return;
        }
        let frame = self.plots.snapshot();
        let pixels = plot_raster::waveform_rgba(&frame);
        let width = plot_raster::WIDTH;
        let height = plot_raster::HEIGHT;

        // Reuse the prior frame's texture so steady-state drawing allocates
        // nothing: write into it, swap it into the registered image, and keep
        // the texture the image released for the next frame.
        let texture = self.waveform_reusable.take().unwrap_or_else(|| {
            context.create_render_target(
                width,
                height,
                Some("hirari-waveform"),
                wgpu::TextureUsages::COPY_DST,
            )
        });
        context.queue().write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        if let Some(image) = &self.waveform_image {
            match context.replace_texture(image, texture) {
                Ok(previous) => self.waveform_reusable = Some(previous),
                Err(error) => {
                    eprintln!("could not update waveform canvas: {error}");
                    return;
                }
            }
        } else {
            match context.register_texture(texture) {
                Ok(image) => self.waveform_image = Some(image),
                Err(error) => {
                    eprintln!("could not register waveform canvas: {error}");
                    return;
                }
            }
        }

        if let Some(image) = &self.waveform_image {
            context.draw_texture(scene, image, bounds);
        }
    }
}
