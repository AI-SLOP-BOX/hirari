//! AmeUI application shell — the migration target for the Slint surface.
//!
//! Slice 1 boots the real engine inside an AmeUI window: a transport bar
//! (play/stop, tempo, playhead), a live master meter, the shared plot Canvas,
//! and the track list read from the native project layout. Audio playback is
//! real; the Slint UI remains the default entry until the port reaches parity.
//!
//! HirariCore is deliberately not `Send` (its engine stores raw pointers), so
//! every engine read happens on the UI thread. A background ticker only sends
//! `AppMessage("tick")` through the event-loop proxy; `on_event` does the poll.

use std::rc::Rc;
use std::time::Duration;

use ame_ui::app::{
    App, AppEventSender, CanvasBounds, CanvasContext, State, StateContext, Viewport, WindowConfig,
};
use ame_ui::event::AmeEvent;
use ame_ui::theme::Theme;
use ame_ui::view::{ButtonView, SliderView, View, ViewTree};
use ame_ui::CanvasTexture;
use hirari_core_bridge::HirariCore;
use vello::wgpu;

use crate::ui::gpu_canvas::PlotFrameStore;
use crate::ui::plot_raster;

/// One row of the native project layout, reduced to what the shell displays.
#[derive(Clone, PartialEq)]
struct TrackRow {
    id: u32,
    name: String,
    kind: String,
}

pub struct HirariAmeApp {
    core: Option<Rc<HirariCore>>,
    plots: PlotFrameStore,
    playing: State<bool>,
    tempo: State<f32>,
    playhead_beats: State<f64>,
    sample_rate: State<f64>,
    master_peak: State<f32>,
    tracks: State<Vec<TrackRow>>,
    status: State<String>,
    waveform_image: Option<CanvasTexture>,
    waveform_reusable: Option<wgpu::Texture>,
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
        tracks: context.state(Vec::<TrackRow>::new()),
        status: context.state(status),
        waveform_image: None,
        waveform_reusable: None,
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
            self.tracks
                .set(parse_tracks(&core.get_project_layout_json()));
        }
    }
}

/// Reduce the native layout JSON to display rows; malformed input is empty.
fn parse_tracks(layout_json: &str) -> Vec<TrackRow> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(layout_json) else {
        return Vec::new();
    };
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|track| {
            Some(TrackRow {
                id: track.get("id")?.as_u64()? as u32,
                name: track.get("name")?.as_str()?.to_owned(),
                kind: track.get("type")?.as_str()?.to_owned(),
            })
        })
        .collect()
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

    fn view(&mut self) -> View {
        let playing = self.playing.get();
        let tempo = self.tempo.get();
        let beats = self.playhead_beats.get();
        let sample_rate = self.sample_rate.get();
        let peak = self.master_peak.get();
        let tracks = self.tracks.get();
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
                            View::Caption(track.kind.clone()),
                        ])])
                    })
                    .collect(),
            )],
        };

        View::VStack(vec![
            transport,
            meter,
            View::canvas("waveform").grow(1),
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
