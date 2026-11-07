//! AmeUI application shell — the migration target for the Slint surface.
//!
//! Slice 1 boots the real engine inside an AmeUI window: a transport bar
//! (play/stop, tempo, playhead), a live master meter, the shared plot Canvas,
//! and the track list read from the native project layout. Slice 3 adds the
//! arrangement Canvas: lanes, clips, measure ruler, markers and playhead
//! drawn from the neutral arrange model. Slice 4 makes that canvas
//! seekable (click/drag moves the playhead); slice 5 adds zoom (toolbar
//! −/+/FIT) and ruler-drag panning; slice 6 adds click clip selection;
//! slice 7 adds clip drag-move (live preview, commit on release via the
//! engine undo transaction); slice 8 adds edge-drag trim (Audio/Vocal
//! only, fraction-clamped like the Slint dock). Audio playback is real;
//! the Slint UI remains the default entry until the port reaches parity.
//!
//! HirariCore is deliberately not `Send` (its engine stores raw pointers), so
//! every engine read happens on the UI thread. A background ticker only sends
//! `AppMessage("tick")` through the event-loop proxy; `on_event` does the poll.

use std::rc::Rc;
use std::time::Duration;

use ame_ui::app::{
    App, AppEventSender, CanvasBounds, CanvasContext, CanvasPointerEvent, CanvasPointerPhase,
    State, StateContext, Viewport, WindowConfig,
};
use ame_ui::event::AmeEvent;
use ame_ui::theme::Theme;
use ame_ui::view::{ButtonView, SliderView, View, ViewTree};
use ame_ui::CanvasTexture;
use hirari_core_bridge::HirariCore;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::wgpu;

use crate::ui::arrange_model::{
    arrange_canvas_geometry, arrangement_measure_starts, arrangement_timeline_end, clip_at,
    last_clip_end, parse_arrange_tracks, parse_markers, trim_edge_at, valid_time_signatures,
    ArrangeCanvasGeometry, ArrangeMarker, ArrangeTrack, MeasureStart, TrimEdge, ARRANGE_ZOOM_MAX,
    ARRANGE_ZOOM_MIN,
};
use crate::ui::gpu_canvas::PlotFrameStore;
use crate::ui::plot_raster;

/// Clip-move snap in beats: the Slint default division (1/16 of a 4-beat
/// bar). A snap picker is a later slice; the move itself already matches.
const MOVE_SNAP_BEATS: f32 = 0.25;

/// What a lane press turned into once it crossed the click threshold.
#[derive(Clone, Copy, PartialEq)]
enum LaneDrag {
    /// Empty lane space: drag seeks the playhead.
    Seek,
    /// A non-MIDI clip: drag moves it, commit on release.
    Move,
    /// An Audio/Vocal clip edge: drag trims it, commit on release.
    TrimLeft,
    TrimRight,
    /// A MIDI clip: select on press, drag does nothing (Slint parity).
    Hold,
}

pub struct HirariAmeApp {
    core: Option<Rc<HirariCore>>,
    plots: PlotFrameStore,
    playing: State<bool>,
    tempo: State<f32>,
    playhead_beats: State<f64>,
    sample_rate: State<f64>,
    master_peak: State<f32>,
    tracks: State<Vec<ArrangeTrack>>,
    markers: State<Vec<ArrangeMarker>>,
    measure_starts: State<Vec<MeasureStart>>,
    timeline_end_beats: State<f32>,
    arrange_zoom: State<f32>,
    arrange_scroll_beats: State<f32>,
    /// Selected clip as `(track_id, clip_id)`, mirroring the Slint
    /// selection the later drag/trim slice will mutate through.
    selected_clip: State<Option<(u32, i32)>>,
    status: State<String>,
    waveform_image: Option<CanvasTexture>,
    waveform_reusable: Option<wgpu::Texture>,
    /// Last drawn arrange layout, used to map pointer x back to a beat.
    arrange_geometry: ArrangeCanvasGeometry,
    seeking: bool,
    panning: bool,
    pan_last_x: f64,
    down_x: f64,
    down_y: f64,
    down_clip: Option<(u32, i32)>,
    down_mode: LaneDrag,
    /// Clip being dragged, with its original start and snapped delta for
    /// the live preview; committed to the engine on release.
    move_clip: Option<(u32, i32)>,
    move_start_beat: f32,
    move_delta_beats: f32,
    /// Clip being trimmed, its edge, and the snapped beat delta for the
    /// live preview; committed on release like the move.
    trim_clip: Option<(u32, i32)>,
    trim_edge: TrimEdge,
    trim_delta_beats: f32,
    tick: u64,
}

/// Build the app on the event-loop (main) thread, where the engine must live.
pub fn create_app(context: &StateContext) -> HirariAmeApp {
    let (core, status) = match HirariCore::new() {
        Ok(core) => (Some(Rc::new(core)), String::from("ENGINE READY")),
        Err(error) => (None, format!("ENGINE INIT FAILED: {error}")),
    };
    HirariAmeApp {
        core,
        plots: PlotFrameStore::default(),
        playing: context.state(false),
        tempo: context.state(120.0f32),
        playhead_beats: context.state(0.0f64),
        sample_rate: context.state(0.0f64),
        master_peak: context.state(0.0f32),
        tracks: context.state(Vec::<ArrangeTrack>::new()),
        markers: context.state(Vec::<ArrangeMarker>::new()),
        measure_starts: context.state(Vec::<MeasureStart>::new()),
        timeline_end_beats: context.state(128.0f32),
        arrange_zoom: context.state(1.0f32),
        arrange_scroll_beats: context.state(0.0f32),
        selected_clip: context.state(None),
        status: context.state(status),
        waveform_image: None,
        waveform_reusable: None,
        arrange_geometry: arrange_canvas_geometry(0.0, 0.0, 128.0, 0, 1.0, 0.0),
        seeking: false,
        panning: false,
        pan_last_x: 0.0,
        down_x: 0.0,
        down_y: 0.0,
        down_clip: None,
        down_mode: LaneDrag::Seek,
        move_clip: None,
        move_start_beat: 0.0,
        move_delta_beats: 0.0,
        trim_clip: None,
        trim_edge: TrimEdge::Left,
        trim_delta_beats: 0.0,
        tick: 0,
    }
}

/// Run the AmeUI shell. Called from `main` under the `ameui` feature.
pub fn run() {
    if let Err(error) = ame_ui::app::run_with(create_app) {
        eprintln!("Hirari AmeUI event loop failed: {error}");
    }
}

impl HirariAmeApp {
    /// Sample the engine on the UI thread and publish reactive state.
    fn poll_engine(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        self.playing.set(core.is_playing());
        self.tempo.set(core.get_tempo());
        self.sample_rate.set(core.get_sample_rate());
        self.playhead_beats
            .set(core.samples_to_beats(core.get_playhead()));

        let loudness = core.get_master_loudness();
        let peak = 10.0f32
            .powf(loudness.true_peak_l / 20.0)
            .max(10.0f32.powf(loudness.true_peak_r / 20.0))
            .clamp(0.0, 1.0);
        self.master_peak.set(peak);
        self.plots.publish_meters(&[peak, peak]);

        // The layout walk is comparatively costly; refresh it a few times a
        // second rather than on every 30 Hz tick.
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 10 == 1 {
            self.refresh_arrange_state(&core);
        }
    }

    /// Republishes lanes, markers, ruler and timeline end from the engine.
    /// Shared by the poll cadence and by clip-move commits, which refresh
    /// immediately instead of waiting for the next layout tick.
    fn refresh_arrange_state(&self, core: &HirariCore) {
        // The neutral arrange model is the one layout parser for both
        // surfaces, so this shell and the Slint arranger reduce the
        // same native layout to the same lanes, ruler and timeline.
        let tracks = parse_arrange_tracks(&core.get_project_layout_json(), |samples| {
            core.samples_to_beats(samples)
        });
        let raw_time_signatures = core.get_time_signature_events();
        let timeline_end = arrangement_timeline_end(
            last_clip_end(&tracks),
            &valid_time_signatures(&raw_time_signatures),
        );
        self.markers.set(parse_markers(&core.markers_json()));
        self.measure_starts.set(arrangement_measure_starts(
            &raw_time_signatures,
            timeline_end,
        ));
        self.timeline_end_beats.set(timeline_end);
        self.tracks.set(tracks);
    }

    /// Pointer travel since the lane press, in logical pixels. Clicks
    /// within this distance select; beyond it the press becomes a seek drag.
    fn click_distance(&self, event: &CanvasPointerEvent) -> f64 {
        (event.x - self.down_x).hypot(event.y - self.down_y)
    }

    /// Clamps a trim beat delta through the Slint dock fraction math, so
    /// the preview and the commit agree: fractions clamp to [0, 1] with a
    /// positive span, then convert back to beats.
    fn clamped_trim_delta(&self, snapped: f32) -> f32 {
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
    /// Slint `on_trim_clip` path: one undo transaction, abort on failure,
    /// immediate layout refresh on success.
    fn commit_clip_trim(&mut self) {
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

    /// Commits the dragged clip move through the engine, mirroring the
    /// Slint `on_clip_moved` path: one undo transaction, sync-group aware,
    /// abort on failure, immediate layout refresh on success.
    fn commit_clip_move(&mut self) {
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
    fn seek_arrange(&self, x: f64) -> bool {
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

    /// Draws the arrangement (ruler, lanes, clips, markers, playhead) as
    /// vector shapes. This is the read-only parity step: geometry matches
    /// the Slint dock, but there is no zoom/scroll, selection or drag yet,
    /// and no text inside the canvas (lane names live in the list below).
    /// Scene coordinates are absolute window pixels, offset by `bounds`.
    fn draw_arrange(&mut self, scene: &mut vello::Scene, bounds: CanvasBounds) {
        let x0 = f64::from(bounds.x);
        let y0 = f64::from(bounds.y);
        let width = f64::from(bounds.width);
        let height = f64::from(bounds.height);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let tracks = self.tracks.get();
        let selected_clip = self.selected_clip.get();
        let markers = self.markers.get();
        let measure_starts = self.measure_starts.get();
        let timeline_end = self.timeline_end_beats.get();
        let geometry = arrange_canvas_geometry(
            bounds.width,
            bounds.height,
            timeline_end,
            tracks.len(),
            self.arrange_zoom.get(),
            self.arrange_scroll_beats.get(),
        );
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
            Color::from_rgb8(15, 17, 19),
            None,
            &Rect::new(x0, y0, x0 + width, y0 + height),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(22, 25, 29),
            None,
            &Rect::new(x0, y0, x0 + width, y0 + ruler),
        );

        // Faint beat grid, then stronger measure lines (Slint opacities).
        let steps = (f64::from(timeline_end) / f64::from(geometry.beat_step)).ceil() as u32;
        for step in 0..=steps {
            let x = x_for_beat(step as f32 * geometry.beat_step);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                Color::from_rgba8(120, 128, 140, 40),
                None,
                &Line::new((x, y0 + ruler), (x, y0 + height)),
            );
        }
        for measure in &measure_starts {
            let x = x_for_beat(measure.beat);
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                Color::from_rgba8(120, 128, 140, 140),
                None,
                &Line::new((x, y0), (x, y0 + height)),
            );
        }

        for (index, track) in tracks.iter().enumerate() {
            let top = y0 + f64::from(geometry.y_for_lane(index));
            scene.stroke(
                &grid,
                Affine::IDENTITY,
                Color::from_rgb8(38, 42, 46),
                None,
                &Line::new((x0, top), (x0 + width, top)),
            );
            // Lane color strip, as in the Slint header.
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgb8(track.color[0], track.color[1], track.color[2]),
                None,
                &Rect::new(x0, top, x0 + 3.0, top + lane_height),
            );
            let clip_height = (lane_height - 4.0).clamp(2.0, 42.0);
            for clip in &track.clips {
                let selected = selected_clip == Some((track.id, clip.id));
                // Live move preview: the dragged clip follows the pointer
                // until release commits it (Slint `drag-current-delta`).
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
                // Slint selection: brighter fill and a 2px border.
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

        // Slint Style.playhead (#30D158), drawn only inside the timeline.
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

impl App for HirariAmeApp {
    fn window_config(&self) -> WindowConfig {
        WindowConfig {
            title: "Hirari (AmeUI)".to_owned(),
            width: 1280.0,
            height: 800.0,
            min_width: 900.0,
            min_height: 600.0,
            ..WindowConfig::default()
        }
    }

    fn full_bleed_layout(&self) -> bool {
        true
    }

    fn content_max_width(&self) -> Option<f32> {
        None
    }

    fn theme(&self) -> Theme {
        Theme::dark()
    }

    fn on_event_loop_ready(&mut self, sender: AppEventSender) {
        // The ticker never touches the engine; it only nudges the UI thread.
        // When the loop closes the proxy disconnects and `send` returns false.
        std::thread::spawn(move || {
            while sender.send("tick") {
                std::thread::sleep(Duration::from_millis(33));
            }
        });
    }

    fn on_event(&mut self, event: &AmeEvent) {
        if let AmeEvent::AppMessage(message) = event {
            if message == "tick" {
                self.poll_engine();
            }
        }
    }

    fn on_canvas_pointer_event(&mut self, canvas_id: &str, event: CanvasPointerEvent) -> bool {
        if canvas_id != "arrange" {
            return false;
        }
        match event.phase {
            CanvasPointerPhase::Down => {
                if (event.y as f32) < self.arrange_geometry.ruler_height {
                    // Ruler drag pans; lane drags seek (slice 4).
                    self.panning = true;
                    self.pan_last_x = event.x;
                    true
                } else {
                    // Provisional until the click threshold is crossed:
                    // a clip press selects immediately (Slint parity) and
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
                                Some(clip) if clip.midi => self.down_mode = LaneDrag::Hold,
                                Some(clip) => {
                                    // Edge zones trim (Audio/Vocal only, as
                                    // in the Slint dock); the body moves.
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
                                    let snapped = (raw / MOVE_SNAP_BEATS).round() * MOVE_SNAP_BEATS;
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
                                    let snapped = (raw / MOVE_SNAP_BEATS).round() * MOVE_SNAP_BEATS;
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

    fn view(&mut self) -> View {
        let playing = self.playing.get();
        let tempo = self.tempo.get();
        let beats = self.playhead_beats.get();
        let sample_rate = self.sample_rate.get();
        let peak = self.master_peak.get();
        let tracks = self.tracks.get();
        let zoom = self.arrange_zoom.get();
        let status = self.status.get();

        let play_core = self.core.clone();
        let play_state = self.playing.clone();
        let play_button = ButtonView::new(if playing { "Playing" } else { "Play" })
            .action_id("transport.play")
            .primary()
            .on_press(move |_| {
                if let Some(core) = &play_core {
                    core.set_playing(true);
                }
                play_state.set(true);
            });

        let stop_core = self.core.clone();
        let stop_state = self.playing.clone();
        let stop_button = ButtonView::new("Stop")
            .action_id("transport.stop")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &stop_core {
                    core.set_playing(false);
                }
                stop_state.set(false);
            });

        let tempo_core = self.core.clone();
        let tempo_state = self.tempo.clone();
        let tempo_slider = SliderView::new(tempo, 20.0, 300.0)
            .action_id("transport.tempo")
            .on_change(move |value| {
                if let Some(core) = &tempo_core {
                    core.set_tempo(value);
                }
                tempo_state.set(value);
            });

        let transport = View::Card(vec![View::HStack(vec![
            play_button.into(),
            stop_button.into(),
            View::Text(format!("{beats:.2} beats")).font_size(18),
            View::Spacer,
            View::Text(format!("{tempo:.1} BPM")),
            tempo_slider.into(),
            View::Caption(format!("{sample_rate:.0} Hz")),
        ])]);

        let meter = View::Card(vec![View::HStack(vec![
            View::Text("Master".to_owned()),
            View::ProgressBar((peak * 100.0) as u8).grow(1),
            View::Caption(format!("{:.0}%", peak * 100.0)),
            View::Caption(status),
        ])]);

        // Arrange toolbar (Slint dock header): zoom steps match the Slint
        // ±1.25 buttons; FIT returns to fit-to-width. Ruler drag pans.
        let zoom_out_state = self.arrange_zoom.clone();
        let zoom_out = ButtonView::new("−")
            .action_id("arrange.zoom_out")
            .secondary()
            .on_press(move |_| {
                zoom_out_state.set((zoom_out_state.get() / 1.25).max(ARRANGE_ZOOM_MIN));
            });
        let zoom_in_state = self.arrange_zoom.clone();
        let zoom_in = ButtonView::new("+")
            .action_id("arrange.zoom_in")
            .secondary()
            .on_press(move |_| {
                zoom_in_state.set((zoom_in_state.get() * 1.25).min(ARRANGE_ZOOM_MAX));
            });
        let fit_zoom_state = self.arrange_zoom.clone();
        let fit_scroll_state = self.arrange_scroll_beats.clone();
        let zoom_fit = ButtonView::new("FIT")
            .action_id("arrange.zoom_fit")
            .secondary()
            .on_press(move |_| {
                fit_zoom_state.set(1.0);
                fit_scroll_state.set(0.0);
            });
        let arrange_toolbar = View::Card(vec![View::HStack(vec![
            View::Text("ARRANGEMENT".to_owned()),
            zoom_out.into(),
            zoom_in.into(),
            zoom_fit.into(),
            View::Caption(format!("ZOOM {}%", (zoom * 100.0).round() as u32)),
            View::Caption("ruler: pan · empty lane: seek · clip: move · edges: trim".to_owned()),
        ])]);

        // The canvas draws geometry only (no text); the list below keeps
        // the lane names the Slint header shows, with clip counts.
        let track_list = View::ScrollView {
            height: 180,
            follow_bottom: false,
            scroll_to: None,
            children: vec![View::VStack(
                tracks
                    .iter()
                    .map(|track| {
                        View::Card(vec![View::HStack(vec![
                            View::Text(format!("#{}", track.id)),
                            View::Text(track.name.clone()).grow(1),
                            View::Caption(format!("{} · {} clips", track.kind, track.clips.len())),
                        ])])
                    })
                    .collect(),
            )],
        };

        View::VStack(vec![
            transport,
            meter,
            arrange_toolbar,
            View::canvas("waveform").grow(1),
            View::canvas("arrange").grow(3),
            track_list,
        ])
    }

    fn draw_canvas(
        &mut self,
        canvas_id: &str,
        scene: &mut vello::Scene,
        bounds: CanvasBounds,
        context: &mut CanvasContext<'_>,
        _viewport: Viewport,
        _tree: &ViewTree,
    ) {
        if canvas_id == "arrange" {
            self.draw_arrange(scene, bounds);
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
