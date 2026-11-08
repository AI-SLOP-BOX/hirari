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
//! only, fraction-clamped like the Slint dock); slice 9 draws clip
//! waveform peaks from the async decode queue; slice 10 adds the snap
//! division picker; slice 11 adds the edit lock; slice 12 turns the
//! track list into mixer strips (fader/pan/mute/solo/arm); slice 13 adds
//! project open/save/new; slice 14 adds loop/metronome toggles; slice 15
//! adds the read-only piano roll; slice 16 adds its edit gestures
//! (add/move/resize/delete via a full note-list replace); slice 17 adds
//! quantize and velocity stepping; slice 18 adds chord insert and
//! delete-all; slice 19 adds note lyric editing; slice 20 adds the
//! articulation stepper; slice 21 adds keyboard shortcuts (Space
//! plays, L loops); slice 22 adds progression insert; slice 23 adds
//! articulation-map keyswitch editing; slice 24 adds articulation-name
//! editing; slice 25 adds the selected-clip fade/gain inspector;
//! slice 26 adds the per-track plugin strip; slice 27 adopts canvas
//! text (lane/clip names, measure numbers, pitch labels); slice 28
//! adopts canvas wheel (scroll and Ctrl-wheel zoom). Audio playback
//! is real; the Slint UI remains the default entry until
//! the port reaches parity.
//!
//! HirariCore is deliberately not `Send` (its engine stores raw pointers), so
//! every engine read happens on the UI thread. A background ticker only sends
//! `AppMessage("tick")` through the event-loop proxy; `on_event` does the poll.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use ame_ui::app::{
    App, AppEventSender, CanvasBounds, CanvasContext, CanvasPointerEvent, CanvasPointerPhase,
    CanvasScrollEvent, State, StateContext, Viewport, WindowConfig,
};
use ame_ui::event::{AmeEvent, KeyModifiers};
use ame_ui::theme::Theme;
use ame_ui::view::{ButtonView, SliderView, TextFieldView, View, ViewTree};
use ame_ui::CanvasTexture;
use hirari_core_bridge::HirariCore;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::wgpu;

use crate::ui::arrange_model::{
    arrange_canvas_geometry, arrangement_measure_starts, arrangement_timeline_end, clip_at,
    last_clip_end, note_at, parse_arrange_tracks, parse_markers, parse_midi_notes,
    piano_roll_geometry, plugin_name, trim_edge_at, valid_time_signatures, ArrangeCanvasGeometry,
    ArrangeMarker, ArrangeTrack, MeasureStart, PianoNote, PianoRollGeometry, TrimEdge,
    ARRANGE_ZOOM_MAX, ARRANGE_ZOOM_MIN,
};
use crate::ui::gpu_canvas::PlotFrameStore;
use crate::ui::plot_raster;
use hirari_core_bridge::project_contracts::MidiNoteContract;

/// Snap divisions offered by the toolbar picker (Slint `snap_val`
/// denominators; the step is 4 beats / division, so 1/16 snaps to 0.25).
const SNAP_DIVISIONS: [f32; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

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

/// In-progress piano-roll gesture; previewed in the canvas and committed
/// to the engine as one full note-list replace on release.
enum PianoEditKind {
    Add,
    Move,
    Resize,
}

struct PianoEdit {
    kind: PianoEditKind,
    /// Index into the full `piano_notes` list; `None` for an added note.
    note_index: Option<usize>,
    down_x: f64,
    down_y: f64,
    orig_pitch: u8,
    orig_start: f32,
    orig_length: f32,
    delta_beats: f32,
    delta_pitches: i32,
}

/// Full note-list replace in one undo transaction, then republish from
/// the engine (mirrors `sync_midi_notes_to_core`). Free function so view
/// closures (which cannot borrow the app) can commit too.
fn replace_notes(core: &HirariCore, notes_state: &State<Vec<PianoNote>>, notes: Vec<PianoNote>) {
    core.begin_undo_transaction("Edit MIDI Notes");
    if core.replace_midi_note_contracts(notes_to_contracts(core, &notes), true) {
        let _ = core.end_undo_transaction();
    } else {
        let _ = core.abort_undo_transaction();
    }
    notes_state.set(parse_midi_notes(&core.midi_notes_json(), |samples| {
        core.samples_to_beats(samples)
    }));
}

/// Elides `text` with an ellipsis to fit `max_width` at `font_size`,
/// using the same measure the canvas draws with.
fn elide_text(
    context: &CanvasContext,
    text: &str,
    max_width: f32,
    font_size: f32,
    bounds: &CanvasBounds,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if context.measure_text(text, font_size, bounds).0 <= max_width {
        return text.to_owned();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let trial = format!("{out}{ch}…");
        if context.measure_text(&trial, font_size, bounds).0 > max_width {
            break;
        }
        out.push(ch);
    }
    format!("{out}…")
}

/// Converts the parsed notes back to engine contracts for a full-list
/// replace, with the same field clamping as the Slint commit path.
fn notes_to_contracts(core: &HirariCore, notes: &[PianoNote]) -> Vec<MidiNoteContract> {
    notes
        .iter()
        .map(|note| {
            let start = core.beats_to_samples(f64::from(note.start_beat.max(0.0)));
            let end = core.beats_to_samples(f64::from(
                note.start_beat.max(0.0) + note.length_beats.max(0.015625),
            ));
            let length = end.saturating_sub(start).max(1);
            MidiNoteContract {
                track_id: note.track_id,
                region_id: note.region_id,
                pitch: note.pitch.clamp(0, 127),
                midi_channel: note.midi_channel.clamp(0, 15),
                articulation: note.articulation.clamp(0, 127),
                velocity: note.velocity.clamp(1, 127),
                start_sample: start,
                length_samples: length,
                lyric: note.lyric.clone(),
                phoneme: note.phoneme.clone(),
                pitch_curve_cents: note.pitch_curve_cents.clone(),
                vibrato_depth_cents: note.vibrato_depth_cents,
                vibrato_rate_millihz: note.vibrato_rate_millihz.clamp(500, 20_000),
                portamento_samples: note
                    .portamento_samples
                    .min(length.min(u64::from(u32::MAX)) as u32),
                probability: note.probability.clamp(0, 100),
                repeat_count: note.repeat_count.max(1),
            }
        })
        .collect()
}

pub struct HirariAmeApp {
    core: Option<Rc<HirariCore>>,
    plots: PlotFrameStore,
    playing: State<bool>,
    loop_enabled: State<bool>,
    metronome_enabled: State<bool>,
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
    /// Snap division denominator (Slint default 16); step is 4/division.
    arrange_snap_division: State<f32>,
    /// Edit lock (Slint `is_locked`): while on, clip drags select only and
    /// never move or trim. Seek and pan still work.
    arrange_locked: State<bool>,
    /// Selected clip as `(track_id, clip_id)`, mirroring the Slint
    /// selection the later drag/trim slice will mutate through.
    selected_clip: State<Option<(u32, i32)>>,
    /// Decoded peak vectors per clip, keyed by `(track_id, clip_id)`.
    /// Filled asynchronously; see `poll_waveforms`.
    clip_peaks: State<HashMap<(u32, i32), Vec<f32>>>,
    /// Piano roll track (0 = unset; defaults to the first track with
    /// notes) and its parsed notes.
    piano_track: State<u32>,
    piano_notes: State<Vec<PianoNote>>,
    /// Chord-insert settings (Slint piano roll chord controls).
    chord_root: State<u32>,
    chord_quality: State<u32>,
    chord_length: State<f32>,
    chord_inversion: State<u32>,
    progression_preset: State<u32>,
    /// Articulation-map id being edited (1..255).
    map_articulation_id: State<u32>,
    /// Plugin type the ADD button installs (built-in chain 0..=10).
    add_plugin_type: State<u32>,
    /// Selected note index into `piano_notes` (for delete).
    selected_note: State<Option<usize>>,
    status: State<String>,
    waveform_image: Option<CanvasTexture>,
    waveform_reusable: Option<wgpu::Texture>,
    /// Last drawn arrange layout, used to map pointer x back to a beat.
    arrange_geometry: ArrangeCanvasGeometry,
    piano_geometry: PianoRollGeometry,
    piano_edit: Option<PianoEdit>,
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
    /// In-flight waveform decodes: request id -> (track, clip, generation).
    waveform_requests: HashMap<u64, (u32, u32, u64)>,
    failed_waveforms: HashSet<(u64, u32, u32)>,
    /// Cached project generation, recomputed only when the state revision
    /// moves (serializing the project every tick stalls the UI thread).
    gen_revision: u64,
    gen_value: u64,
    last_gen: u64,
    /// Last opened/saved project path, for save-without-dialog.
    project_path: Rc<RefCell<Option<String>>>,
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
        loop_enabled: context.state(false),
        metronome_enabled: context.state(false),
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
        arrange_snap_division: context.state(16.0f32),
        arrange_locked: context.state(false),
        selected_clip: context.state(None),
        clip_peaks: context.state(HashMap::new()),
        piano_track: context.state(0u32),
        piano_notes: context.state(Vec::<PianoNote>::new()),
        chord_root: context.state(60u32),
        chord_quality: context.state(0u32),
        chord_length: context.state(4.0f32),
        chord_inversion: context.state(0u32),
        progression_preset: context.state(0u32),
        map_articulation_id: context.state(1u32),
        add_plugin_type: context.state(0u32),
        selected_note: context.state(None),
        status: context.state(status),
        waveform_image: None,
        waveform_reusable: None,
        arrange_geometry: arrange_canvas_geometry(0.0, 0.0, 128.0, 0, 1.0, 0.0),
        piano_geometry: piano_roll_geometry(0.0, 0.0, 128.0, &[]),
        piano_edit: None,
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
        waveform_requests: HashMap::new(),
        failed_waveforms: HashSet::new(),
        gen_revision: u64::MAX,
        gen_value: 0,
        last_gen: u64::MAX,
        project_path: Rc::new(RefCell::new(None)),
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
        self.loop_enabled.set(core.is_loop_enabled());
        self.metronome_enabled.set(core.is_metronome_enabled());
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
            self.poll_waveforms(&core);
        }
    }

    /// Consumes finished waveform decodes and queues missing clip peaks,
    /// mirroring the Slint telemetry queue: at most 8 detached decodes in
    /// flight, generation-guarded results, failures remembered per
    /// generation. Runs on the layout cadence, never on the UI event path.
    fn poll_waveforms(&mut self, core: &HirariCore) {
        const MAX_REQUESTS: usize = 8;
        let revision = core.project_state_revision();
        let generation = if revision == self.gen_revision {
            self.gen_value
        } else {
            let generation = core.project_generation();
            self.gen_revision = revision;
            self.gen_value = generation;
            generation
        };
        if generation != self.last_gen {
            self.last_gen = generation;
            self.clip_peaks.set(HashMap::new());
            self.failed_waveforms.clear();
        }
        let tracks = self.tracks.get();
        let completed: Vec<(u64, u32, u32, u64)> = self
            .waveform_requests
            .iter()
            .map(|(id, &(track, clip, generation))| (*id, track, clip, generation))
            .collect();
        for (request, track_id, clip_id, request_generation) in completed {
            if core.region_waveform_pending(request) {
                continue;
            }
            let waveform = core.poll_region_waveform(request);
            if request_generation == generation && !waveform.is_empty() {
                let still_exists = tracks.iter().any(|track| {
                    track.id == track_id && track.clips.iter().any(|clip| clip.id == clip_id as i32)
                });
                if still_exists {
                    let mut peaks = self.clip_peaks.get();
                    peaks.insert((track_id, clip_id as i32), waveform);
                    self.clip_peaks.set(peaks);
                }
            } else if request_generation == generation {
                self.failed_waveforms
                    .insert((generation, track_id, clip_id));
            }
            self.waveform_requests.remove(&request);
        }
        self.failed_waveforms
            .retain(|(gen, _, _)| *gen == generation);
        'queue: for track in &tracks {
            for clip in &track.clips {
                if clip.midi || clip.id <= 0 {
                    continue;
                }
                let key = (generation, track.id, clip.id as u32);
                if self.clip_peaks.get().contains_key(&(track.id, clip.id))
                    || self.failed_waveforms.contains(&key)
                    || self.waveform_requests.values().any(
                        |&(queued_track, queued_clip, queued_gen)| {
                            queued_gen == generation
                                && queued_track == track.id
                                && queued_clip == clip.id as u32
                        },
                    )
                {
                    continue;
                }
                if self.waveform_requests.len() >= MAX_REQUESTS {
                    break 'queue;
                }
                let request = core.queue_region_waveform(track.id, clip.id as u32);
                if request != 0 {
                    self.waveform_requests
                        .insert(request, (track.id, clip.id as u32, generation));
                }
            }
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
        let notes = parse_midi_notes(&core.midi_notes_json(), |samples| {
            core.samples_to_beats(samples)
        });
        if self.piano_track.get() == 0 {
            let default = notes
                .first()
                .map(|note| note.track_id)
                .unwrap_or_else(|| tracks.first().map(|track| track.id).unwrap_or(0));
            self.piano_track.set(default);
        }
        self.piano_notes.set(notes);
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

    /// Commits the piano-roll gesture as one full note-list replace,
    /// mirroring the Slint `sync_midi_notes_to_core` path: every note
    /// keeps its authoring contract, the replace is one undo
    /// transaction, and the roll republishes from the engine after.
    fn commit_piano_edit(&mut self) {
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

    fn replace_notes(&self, notes: Vec<PianoNote>) {
        if let Some(core) = self.core.clone() {
            replace_notes(&core, &self.piano_notes, notes);
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

    /// Draws the selected track's notes fit-to-width, with beat grid and
    /// playhead. Read-only for now (no add/move/resize yet); note
    /// geometry comes from `piano_roll_geometry`.
    /// Scene coordinates are absolute window pixels, offset by `bounds`.
    fn draw_piano(
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
        let track_id = self.piano_track.get();
        let notes = self.piano_notes.get();
        let visible: Vec<PianoNote> = notes
            .iter()
            .filter(|note| note.track_id == track_id)
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
            Color::from_rgb8(15, 17, 19),
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
                Color::from_rgba8(120, 128, 140, 40),
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
                Color::from_rgba8(120, 128, 140, if stronger { 90 } else { 35 }),
                None,
                &Line::new((x0, y), (x0 + width, y)),
            );
            if stronger {
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
            if note.track_id != track_id {
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

    /// Draws the arrangement (ruler, lanes, clips, markers, playhead) as
    /// vector shapes. This is the read-only parity step: geometry matches
    /// the Slint dock, but there is no zoom/scroll, selection or drag yet,
    /// and no text inside the canvas (lane names live in the list below).
    /// Scene coordinates are absolute window pixels, offset by `bounds`.
    fn draw_arrange(
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
        let tracks = self.tracks.get();
        let selected_clip = self.selected_clip.get();
        let peaks = self.clip_peaks.get();
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
        // Lane-header column (Slint 104px): name band for the text labels.
        let header = f64::from(geometry.header_width);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(22, 25, 29),
            None,
            &Rect::new(x0, y0, x0 + header, y0 + height),
        );
        scene.stroke(
            &grid,
            Affine::IDENTITY,
            Color::from_rgb8(38, 42, 46),
            None,
            &Line::new((x0 + header, y0), (x0 + header, y0 + height)),
        );
        let theme_text = context.theme().text;
        let theme_secondary = context.theme().secondary_text;

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
            context.draw_text(
                scene,
                &bounds,
                &measure.number.to_string(),
                x + 4.0,
                y0 + 4.0,
                10.0,
                theme_secondary,
                None,
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
            let name = elide_text(context, &track.name, (header - 12.0) as f32, 11.0, &bounds);
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
            let sub = elide_text(
                context,
                &format!("{} · {} clips", track.kind, track.clips.len()),
                (header - 12.0) as f32,
                9.0,
                &bounds,
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
                if clip_width >= 24.0 {
                    let label = elide_text(
                        context,
                        &clip.name,
                        (clip_width - 8.0) as f32,
                        10.0,
                        &bounds,
                    );
                    context.draw_text(
                        scene,
                        &bounds,
                        &label,
                        clip_x + 4.0,
                        clip_y + (clip_height - 10.0) / 2.0,
                        10.0,
                        theme_text,
                        None,
                    );
                }
                // Waveform peaks, as in the Slint dock: 128 pairs windowed
                // by trim (and flipped when reversed), drawn only when the
                // clip is wide enough to read (48px gate, as in Slint).
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
    /// Piano-roll gestures: press a note to move it (right edge resizes),
    /// press empty space to add one, drag to preview, release to commit.
    /// The arrange edit lock holds edits here too (selection only).
    fn on_piano_pointer(&mut self, event: CanvasPointerEvent) -> bool {
        let track_id = self.piano_track.get();
        let geometry = self.piano_geometry;
        match event.phase {
            CanvasPointerPhase::Down => {
                let notes = self.piano_notes.get();
                let visible: Vec<(usize, PianoNote)> = notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| note.track_id == track_id)
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

    /// App-wide shortcuts (Slint parity: Space plays, L loops). The
    /// framework only calls this with no text editor focused, so the
    /// lyric field keeps Space/L as text input.
    fn on_shortcut(&mut self, key: &str, modifiers: KeyModifiers) -> bool {
        if modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        match key {
            "Named(Space)" => {
                if let Some(core) = &self.core {
                    let playing = !self.playing.get();
                    core.set_playing(playing);
                    self.playing.set(playing);
                }
                true
            }
            "l" | "L" => {
                if let Some(core) = &self.core {
                    let loop_on = !self.loop_enabled.get();
                    core.set_loop(loop_on);
                    self.loop_enabled.set(loop_on);
                }
                true
            }
            _ => false,
        }
    }

    fn on_canvas_scroll(&mut self, canvas_id: &str, event: CanvasScrollEvent) -> bool {
        if canvas_id != "arrange" {
            return false;
        }
        let geometry = self.arrange_geometry;
        if geometry.px_per_beat <= 0.0 {
            return false;
        }
        if event.modifiers.control || event.modifiers.super_key {
            // Zoom, anchored at the pointer beat: reconstruct the timeline
            // width from the current scale, then re-anchor under the cursor.
            let zoom_old = self.arrange_zoom.get();
            let factor = if event.delta_y > 0.0 {
                1.25
            } else {
                1.0 / 1.25
            };
            let zoom_new = (zoom_old * factor).clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX);
            let timeline_end = self.timeline_end_beats.get();
            let timeline_width = geometry.px_per_beat * timeline_end / zoom_old;
            let px_new = timeline_width / timeline_end * zoom_new;
            let beat = geometry.beat_for_x(event.x as f32);
            let scroll_new = beat - (event.x as f32 - geometry.header_width) / px_new as f32;
            self.arrange_zoom.set(zoom_new);
            self.arrange_scroll_beats.set(scroll_new.max(0.0));
        } else {
            // Plain wheel scrolls the timeline (DAW convention).
            let delta_beats =
                ((event.delta_x + event.delta_y) / f64::from(geometry.px_per_beat)) as f32;
            self.arrange_scroll_beats
                .set((self.arrange_scroll_beats.get() + delta_beats).max(0.0));
        }
        true
    }

    fn on_canvas_pointer_event(&mut self, canvas_id: &str, event: CanvasPointerEvent) -> bool {
        if canvas_id == "piano" {
            return self.on_piano_pointer(event);
        }
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
                                Some(_) if self.arrange_locked.get() => {
                                    self.down_mode = LaneDrag::Hold
                                }
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

        // Project bar: open/save/new. The file dialog blocks the UI thread
        // while it is open, same as the Slint project actions.
        let open_core = self.core.clone();
        let open_path = self.project_path.clone();
        let open_selected = self.selected_clip.clone();
        let open_status = self.status.clone();
        let open_button = ButtonView::new("Open")
            .action_id("project.open")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &open_core else { return };
                let dialog = rfd::FileDialog::new().add_filter("Hirari project", &["hirari"]);
                let dialog = match open_path.borrow().as_deref() {
                    Some(path) => dialog.set_directory(
                        std::path::Path::new(path)
                            .parent()
                            .map(|parent| parent.to_path_buf())
                            .unwrap_or_default(),
                    ),
                    None => dialog,
                };
                let Some(output) = dialog.pick_file() else {
                    return;
                };
                let path = output.to_string_lossy().into_owned();
                if core.load_project(&path) {
                    *open_path.borrow_mut() = Some(path.clone());
                    open_selected.set(None);
                    open_status.set(format!(
                        "OPENED {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or(path)
                    ));
                } else {
                    open_status.set("LOAD FAILED".to_owned());
                }
            });
        let save_core = self.core.clone();
        let save_path = self.project_path.clone();
        let save_status = self.status.clone();
        let save_button = ButtonView::new("Save")
            .action_id("project.save")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &save_core else { return };
                let path = match save_path.borrow().clone() {
                    Some(path) => path,
                    None => {
                        let dialog = rfd::FileDialog::new()
                            .set_file_name("Untitled.hirari")
                            .add_filter("Hirari project", &["hirari"]);
                        let Some(output) = dialog.save_file() else {
                            return;
                        };
                        output.to_string_lossy().into_owned()
                    }
                };
                if core.save_project(&path) {
                    *save_path.borrow_mut() = Some(path.clone());
                    save_status.set(format!(
                        "SAVED {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or(path)
                    ));
                } else {
                    save_status.set("SAVE FAILED".to_owned());
                }
            });
        let new_core = self.core.clone();
        let new_path = self.project_path.clone();
        let new_selected = self.selected_clip.clone();
        let new_status = self.status.clone();
        let new_button = ButtonView::new("New")
            .action_id("project.new")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &new_core {
                    core.new_project();
                    *new_path.borrow_mut() = None;
                    new_selected.set(None);
                    new_status.set("NEW PROJECT".to_owned());
                }
            });
        let project_bar = View::Card(vec![View::HStack(vec![
            View::Text("PROJECT".to_owned()),
            open_button.into(),
            save_button.into(),
            new_button.into(),
            View::Caption(status.clone()),
        ])]);

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

        let loop_core = self.core.clone();
        let loop_state = self.loop_enabled.clone();
        let loop_on = self.loop_enabled.get();
        let loop_button = ButtonView::new(if loop_on { "Loop On" } else { "Loop" })
            .action_id("transport.loop")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &loop_core {
                    core.set_loop(!loop_on);
                }
                loop_state.set(!loop_on);
            });
        let metro_core = self.core.clone();
        let metro_state = self.metronome_enabled.clone();
        let metro_on = self.metronome_enabled.get();
        let metro_button = ButtonView::new(if metro_on { "Click On" } else { "Click" })
            .action_id("transport.metronome")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &metro_core {
                    core.set_metronome_enabled(!metro_on);
                }
                metro_state.set(!metro_on);
            });

        let transport = View::Card(vec![View::HStack(vec![
            play_button.into(),
            stop_button.into(),
            loop_button.into(),
            metro_button.into(),
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
        let snap_state = self.arrange_snap_division.clone();
        let snap_label = format!("SNAP 1/{}", self.arrange_snap_division.get() as u32);
        let snap_button = ButtonView::new(snap_label)
            .action_id("arrange.snap_cycle")
            .secondary()
            .on_press(move |_| {
                let current = snap_state.get();
                let next = SNAP_DIVISIONS
                    .iter()
                    .find(|division| **division > current)
                    .copied()
                    .unwrap_or(SNAP_DIVISIONS[0]);
                snap_state.set(next);
            });
        let lock_state = self.arrange_locked.clone();
        let locked = self.arrange_locked.get();
        let lock_button = ButtonView::new(if locked { "UNLOCK" } else { "LOCK" })
            .action_id("arrange.lock_toggle")
            .secondary()
            .on_press(move |_| {
                lock_state.set(!lock_state.get());
            });
        let arrange_toolbar = View::Card(vec![View::HStack(vec![
            View::Text("ARRANGEMENT".to_owned()),
            zoom_out.into(),
            zoom_in.into(),
            zoom_fit.into(),
            snap_button.into(),
            lock_button.into(),
            View::Caption(format!("ZOOM {}%", (zoom * 100.0).round() as u32)),
            View::Caption("ruler: pan · empty lane: seek · clip: move · edges: trim".to_owned()),
        ])]);

        // Clip inspector for the selected audio clip: fade in/out and
        // gain sliders, committing through the engine undo transactions
        // like the Slint clip editor. MIDI clips have no region to edit.
        let inspector = self
            .selected_clip
            .get()
            .and_then(|(selected_track, selected_id)| {
                let clip = tracks
                    .iter()
                    .find(|track| track.id == selected_track)
                    .and_then(|track| track.clips.iter().find(|clip| clip.id == selected_id))?;
                if clip.midi || clip.id <= 0 {
                    return None;
                }
                let clip = clip.clone();
                let clip_fade_in = clip.fade_in;
                let clip_fade_out = clip.fade_out;
                let fade_core = self.core.clone();
                let fade_tracks = self.tracks.clone();
                let fade_locked = self.arrange_locked.get();
                let fade_in = SliderView::new(clip.fade_in, 0.0, 1.0)
                    .action_id(format!("clip.fade_in.{}", clip.id))
                    .disabled(fade_locked)
                    .on_change(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &fade_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Fades");
                        let accepted = core.set_region_fades(
                            selected_track,
                            clip.id as u32,
                            value,
                            clip_fade_out,
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            fade_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                let fade_out_core = self.core.clone();
                let fade_out_tracks = self.tracks.clone();
                let fade_out = SliderView::new(clip.fade_out, 0.0, 1.0)
                    .action_id(format!("clip.fade_out.{}", clip.id))
                    .disabled(fade_locked)
                    .on_change(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &fade_out_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Fades");
                        let accepted = core.set_region_fades(
                            selected_track,
                            clip.id as u32,
                            clip_fade_in,
                            value,
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            fade_out_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                let gain_core = self.core.clone();
                let gain_tracks = self.tracks.clone();
                let gain = SliderView::new(clip.gain, 0.0, 2.0)
                    .action_id(format!("clip.gain.{}", clip.id))
                    .disabled(fade_locked)
                    .on_change(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &gain_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Gain");
                        let accepted = core.set_region_gain(
                            selected_track,
                            clip.id as u32,
                            value.clamp(0.0, 2.0),
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            gain_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                Some(View::Card(vec![View::HStack(vec![
                    View::Text(format!("CLIP {} · {}", clip.id, clip.name)),
                    View::Caption("Fade In".to_owned()),
                    fade_in.into(),
                    View::Caption("Fade Out".to_owned()),
                    fade_out.into(),
                    View::Caption("Gain".to_owned()),
                    gain.into(),
                ])]))
            });

        // Plugin strip for the selected track: bypass toggles, removal,
        // and an add stepper over the built-in chain (Slint fx list).
        let plugin_strip = tracks
            .iter()
            .find(|track| track.id == self.piano_track.get())
            .filter(|track| !track.plugin_types.is_empty())
            .map(|track| {
                let track_id = track.id;
                let mut rows: Vec<View> = vec![View::Text("PLUGINS".to_owned())];
                for (index, plugin_type) in track.plugin_types.iter().enumerate() {
                    let bypassed = track.plugin_bypass.get(index).copied().unwrap_or(false);
                    let bypass_core = self.core.clone();
                    let bypass_tracks = self.tracks.clone();
                    let bypass_locked = self.arrange_locked.get();
                    let bypass_button = ButtonView::new(if bypassed { "Enable" } else { "Bypass" })
                        .action_id(format!("plugin.bypass.{index}"))
                        .secondary()
                        .on_press(move |_| {
                            if bypass_locked {
                                return;
                            }
                            if let Some(core) = &bypass_core {
                                if core.set_plugin_bypass(track_id, index as u32, !bypassed) {
                                    bypass_tracks.set(parse_arrange_tracks(
                                        &core.get_project_layout_json(),
                                        |samples| core.samples_to_beats(samples),
                                    ));
                                }
                            }
                        });
                    let remove_core = self.core.clone();
                    let remove_tracks = self.tracks.clone();
                    let remove_button = ButtonView::new("Remove")
                        .action_id(format!("plugin.remove.{index}"))
                        .secondary()
                        .on_press(move |_| {
                            if bypass_locked {
                                return;
                            }
                            if let Some(core) = &remove_core {
                                if core.remove_plugin(track_id, index as u32) {
                                    remove_tracks.set(parse_arrange_tracks(
                                        &core.get_project_layout_json(),
                                        |samples| core.samples_to_beats(samples),
                                    ));
                                }
                            }
                        });
                    rows.push(View::HStack(vec![
                        View::Text(plugin_name(*plugin_type, index)),
                        bypass_button.into(),
                        remove_button.into(),
                    ]));
                }
                let add_type_state = self.add_plugin_type.clone();
                let add_button =
                    ButtonView::new(format!("ADD {}", plugin_name(add_type_state.get(), 0)))
                        .action_id("plugin.add")
                        .secondary()
                        .on_press({
                            let add_core = self.core.clone();
                            let add_tracks = self.tracks.clone();
                            let add_locked = self.arrange_locked.get();
                            let add_type = add_type_state.get();
                            move |_| {
                                if add_locked {
                                    return;
                                }
                                if let Some(core) = &add_core {
                                    if core.add_plugin(track_id, add_type) {
                                        add_tracks.set(parse_arrange_tracks(
                                            &core.get_project_layout_json(),
                                            |samples| core.samples_to_beats(samples),
                                        ));
                                    }
                                }
                            }
                        });
                let cycle_state = self.add_plugin_type.clone();
                let cycle_button = ButtonView::new("TYPE")
                    .action_id("plugin.add_type_cycle")
                    .secondary()
                    .on_press(move |_| {
                        cycle_state.set((cycle_state.get() + 1) % 11);
                    });
                rows.push(View::HStack(vec![cycle_button.into(), add_button.into()]));
                View::Card(vec![View::VStack(rows)])
            });

        // Mixer strips: the canvas draws geometry only (no text), so the
        // list below carries names plus the fader/pan/mute/solo/arm
        // controls, wired to the same engine setters as the Slint mixer.
        let mixer_core = self.core.clone();
        let track_list = View::ScrollView {
            height: 260,
            follow_bottom: false,
            scroll_to: None,
            children: vec![View::VStack(
                tracks
                    .iter()
                    .map(|track| {
                        let track_id = track.id;
                        let volume_core = mixer_core.clone();
                        let volume = SliderView::new(track.volume, 0.0, 2.0)
                            .action_id(format!("mixer.volume.{}", track.id))
                            .on_change(move |value| {
                                if let Some(core) = &volume_core {
                                    core.set_track_fader(track_id, value.clamp(0.0, 2.0));
                                }
                            });
                        let pan_core = mixer_core.clone();
                        let pan = SliderView::new(track.pan, -1.0, 1.0)
                            .action_id(format!("mixer.pan.{}", track.id))
                            .on_change(move |value| {
                                if let Some(core) = &pan_core {
                                    core.set_track_pan(track_id, value.clamp(-1.0, 1.0));
                                }
                            });
                        let mute_core = mixer_core.clone();
                        let muted = track.muted;
                        let mute_button = ButtonView::new(if muted { "Unmute" } else { "Mute" })
                            .action_id(format!("mixer.mute.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &mute_core {
                                    core.set_mute(track_id, !muted);
                                }
                            });
                        let solo_core = mixer_core.clone();
                        let solo = track.solo;
                        let solo_button = ButtonView::new(if solo { "Unsolo" } else { "Solo" })
                            .action_id(format!("mixer.solo.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &solo_core {
                                    core.set_solo(track_id, !solo);
                                }
                            });
                        let arm_core = mixer_core.clone();
                        let armed = track.record_armed;
                        let arm_button = ButtonView::new(if armed { "Disarm" } else { "Arm" })
                            .action_id(format!("mixer.arm.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &arm_core {
                                    let _ = core.set_track_armed(track_id, !armed);
                                }
                            });
                        View::Card(vec![View::VStack(vec![
                            View::HStack(vec![
                                View::Text(format!("#{}", track.id)),
                                View::Text(track.name.clone()).grow(1),
                                View::Caption(format!(
                                    "{} · {} clips",
                                    track.kind,
                                    track.clips.len()
                                )),
                            ]),
                            View::HStack(vec![
                                View::Caption("Vol".to_owned()),
                                volume.into(),
                                View::Caption("Pan".to_owned()),
                                pan.into(),
                            ]),
                            View::HStack(vec![
                                mute_button.into(),
                                solo_button.into(),
                                arm_button.into(),
                            ]),
                        ])])
                    })
                    .collect(),
            )],
        };

        // Piano roll (read-only for now): track picker + note canvas.
        let piano_track = self.piano_track.get();
        let mut piano_buttons: Vec<View> = vec![View::Text("PIANO ROLL".to_owned())];
        for track in &tracks {
            let selector_track = self.piano_track.clone();
            let track_id = track.id;
            let button = ButtonView::new(format!("T{}", track.id))
                .action_id(format!("piano.track.{}", track.id))
                .on_press(move |_| selector_track.set(track_id));
            let button = if track_id == piano_track {
                button.primary()
            } else {
                button.secondary()
            };
            piano_buttons.push(button.into());
        }
        let delete_core = self.core.clone();
        let delete_notes = self.piano_notes.clone();
        let delete_selected = self.selected_note.clone();
        let delete_button = ButtonView::new("DELETE")
            .action_id("piano.delete_note")
            .secondary()
            .on_press(move |_| {
                if delete_core.is_none() {
                    return;
                }
                let Some(index) = delete_selected.get() else {
                    return;
                };
                let mut notes = delete_notes.get();
                if index < notes.len() {
                    notes.remove(index);
                    if let Some(core) = &delete_core {
                        replace_notes(core, &delete_notes, notes);
                    }
                }
                delete_selected.set(None);
            });
        piano_buttons.push(delete_button.into());
        let quantize_core = self.core.clone();
        let quantize_notes = self.piano_notes.clone();
        let quantize_track = self.piano_track.clone();
        let quantize_snap = self.arrange_snap_division.clone();
        let quantize_locked = self.arrange_locked.clone();
        let quantize_button = ButtonView::new("QUANTIZE")
            .action_id("piano.quantize")
            .secondary()
            .on_press(move |_| {
                if quantize_locked.get() {
                    return;
                }
                let Some(core) = &quantize_core else { return };
                let step = 4.0 / quantize_snap.get().max(1.0);
                let track_id = quantize_track.get();
                let mut notes = quantize_notes.get();
                let mut changed = false;
                for note in notes.iter_mut().filter(|note| note.track_id == track_id) {
                    let snapped = (note.start_beat / step).round() * step;
                    if snapped != note.start_beat {
                        note.start_beat = snapped.max(0.0);
                        changed = true;
                    }
                }
                if changed {
                    replace_notes(core, &quantize_notes, notes);
                }
            });
        piano_buttons.push(quantize_button.into());
        for (label, delta) in [("VEL-", -1), ("VEL+", 1)] {
            let velocity_core = self.core.clone();
            let velocity_notes = self.piano_notes.clone();
            let velocity_selected = self.selected_note.clone();
            let velocity_locked = self.arrange_locked.clone();
            let button = ButtonView::new(label)
                .action_id(format!("piano.velocity.{label}"))
                .secondary()
                .on_press(move |_| {
                    if velocity_locked.get() {
                        return;
                    }
                    let Some(core) = &velocity_core else { return };
                    let Some(index) = velocity_selected.get() else {
                        return;
                    };
                    let mut notes = velocity_notes.get();
                    let Some(note) = notes.get_mut(index) else {
                        return;
                    };
                    note.velocity = (i32::from(note.velocity) + delta).clamp(1, 127) as u8;
                    replace_notes(core, &velocity_notes, notes);
                });
            piano_buttons.push(button.into());
        }
        // Chord controls (Slint piano roll): root/quality/length/inversion
        // steppers plus the insert action at the playhead.
        let chord_root_state = self.chord_root.clone();
        let root_down_core = self.chord_root.clone();
        let root_down = ButtonView::new("ROOT-")
            .action_id("piano.chord_root_down")
            .secondary()
            .on_press(move |_| {
                root_down_core.set(root_down_core.get().saturating_sub(1));
            });
        let root_up_core = self.chord_root.clone();
        let root_up = ButtonView::new("ROOT+")
            .action_id("piano.chord_root_up")
            .secondary()
            .on_press(move |_| {
                root_up_core.set((root_up_core.get() + 1).min(127));
            });
        let quality_state = self.chord_quality.clone();
        let quality_label = match self.chord_quality.get() {
            1 => "MIN",
            2 => "7TH",
            3 => "SUS",
            _ => "MAJ",
        };
        let quality_button = ButtonView::new(quality_label)
            .action_id("piano.chord_quality")
            .secondary()
            .on_press(move |_| {
                quality_state.set((quality_state.get() + 1) % 4);
            });
        let length_state = self.chord_length.clone();
        let length_label = format!("LEN{}", self.chord_length.get() as u32);
        let length_button = ButtonView::new(length_label)
            .action_id("piano.chord_length")
            .secondary()
            .on_press(move |_| {
                let next = match length_state.get() {
                    1.0 => 2.0,
                    2.0 => 4.0,
                    _ => 1.0,
                };
                length_state.set(next);
            });
        let inversion_state = self.chord_inversion.clone();
        let inversion_button = ButtonView::new(format!("INV{}", self.chord_inversion.get()))
            .action_id("piano.chord_inversion")
            .secondary()
            .on_press(move |_| {
                inversion_state.set((inversion_state.get() + 1) % 3);
            });
        let chord_core = self.core.clone();
        let chord_notes = self.piano_notes.clone();
        let chord_track = self.piano_track.clone();
        let chord_root = self.chord_root.clone();
        let chord_quality_state = self.chord_quality.clone();
        let chord_length_state = self.chord_length.clone();
        let chord_inversion_state = self.chord_inversion.clone();
        let chord_locked = self.arrange_locked.clone();
        let playhead_state = self.playhead_beats.clone();
        let chord_button = ButtonView::new("CHORD")
            .action_id("piano.chord_insert")
            .secondary()
            .on_press(move |_| {
                if chord_locked.get() {
                    return;
                }
                let Some(core) = &chord_core else { return };
                let root = chord_root.get();
                let quality = chord_quality_state.get();
                let length = chord_length_state.get();
                let inversion = chord_inversion_state.get();
                if !(0..=127).contains(&root)
                    || !(0.0625..=64.0).contains(&length)
                    || quality > 3
                    || inversion > 2
                {
                    return;
                }
                let base_intervals: &[i32] = match quality {
                    0 => &[0, 4, 7],
                    1 => &[0, 3, 7],
                    2 => &[0, 4, 7, 10],
                    _ => &[0, 5, 7],
                };
                let mut intervals = base_intervals.to_vec();
                let rotation = (inversion as usize).min(intervals.len().saturating_sub(1));
                for _ in 0..rotation {
                    let first = intervals.remove(0) + 12;
                    intervals.push(first);
                }
                let track_id = chord_track.get();
                let beat = playhead_state.get().max(0.0) as f32;
                let mut notes = chord_notes.get();
                for interval in &intervals {
                    let pitch = root as i32 + *interval;
                    if !(0..=127).contains(&pitch) {
                        continue;
                    }
                    let pitch = pitch as u8;
                    if notes.iter().any(|note| {
                        note.track_id == track_id
                            && note.pitch == pitch
                            && (note.start_beat - beat).abs() < 0.01
                    }) {
                        continue;
                    }
                    notes.push(PianoNote {
                        track_id,
                        region_id: 0,
                        pitch,
                        midi_channel: 0,
                        articulation: 0,
                        velocity: 100,
                        start_beat: beat,
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
                replace_notes(core, &chord_notes, notes);
            });
        let clear_core = self.core.clone();
        let clear_notes = self.piano_notes.clone();
        let clear_track = self.piano_track.clone();
        let clear_locked = self.arrange_locked.clone();
        let clear_button = ButtonView::new("DELETE ALL")
            .action_id("piano.delete_all")
            .secondary()
            .on_press(move |_| {
                if clear_locked.get() {
                    return;
                }
                let Some(core) = &clear_core else { return };
                let track_id = clear_track.get();
                let notes: Vec<PianoNote> = clear_notes
                    .get()
                    .into_iter()
                    .filter(|note| note.track_id != track_id)
                    .collect();
                replace_notes(core, &clear_notes, notes);
            });
        piano_buttons.extend([
            root_down.into(),
            View::Text(format!("ROOT{}", chord_root_state.get())),
            root_up.into(),
            quality_button.into(),
            length_button.into(),
            inversion_button.into(),
            chord_button.into(),
            clear_button.into(),
        ]);
        // Progression insert (Slint insert-progression): four chords
        // from the preset tables at the playhead, each a chord-length
        // apart, with the engine chord-symbol events.
        let preset_state = self.progression_preset.clone();
        let preset_button = ButtonView::new(format!("P{}", self.progression_preset.get() + 1))
            .action_id("piano.progression_preset")
            .secondary()
            .on_press(move |_| {
                preset_state.set((preset_state.get() + 1) % 3);
            });
        let prog_core = self.core.clone();
        let prog_notes = self.piano_notes.clone();
        let prog_track = self.piano_track.clone();
        let prog_root = self.chord_root.clone();
        let prog_length = self.chord_length.clone();
        let prog_inversion = self.chord_inversion.clone();
        let prog_preset = self.progression_preset.clone();
        let prog_locked = self.arrange_locked.clone();
        let prog_playhead = self.playhead_beats.clone();
        let prog_button = ButtonView::new("PROG")
            .action_id("piano.progression_insert")
            .secondary()
            .on_press(move |_| {
                if prog_locked.get() {
                    return;
                }
                let Some(core) = &prog_core else { return };
                let root = prog_root.get() as i32;
                let length = prog_length.get();
                let inversion = prog_inversion.get();
                let preset = prog_preset.get();
                if !(0..=127).contains(&root)
                    || !(0.0625..=64.0).contains(&length)
                    || preset > 2
                    || inversion > 2
                {
                    return;
                }
                let progressions: [[i32; 4]; 3] = [[0, 7, 9, 5], [9, 5, 0, 7], [0, 9, 5, 7]];
                let qualities: [[i32; 4]; 3] = [[0, 0, 1, 0], [1, 0, 0, 0], [0, 1, 0, 0]];
                let track_id = prog_track.get();
                let beat = prog_playhead.get().max(0.0) as f32;
                let mut notes = prog_notes.get();
                for chord_index in 0..4 {
                    let chord_root = root + progressions[preset as usize][chord_index];
                    let chord_quality = qualities[preset as usize][chord_index];
                    let base_intervals: &[i32] = if chord_quality == 0 {
                        &[0, 4, 7]
                    } else {
                        &[0, 3, 7]
                    };
                    let mut intervals = base_intervals.to_vec();
                    let rotation = (inversion as usize).min(intervals.len().saturating_sub(1));
                    for _ in 0..rotation {
                        let first = intervals.remove(0) + 12;
                        intervals.push(first);
                    }
                    let chord_beat = beat + chord_index as f32 * length;
                    for interval in &intervals {
                        let pitch = chord_root + *interval;
                        if !(0..=127).contains(&pitch) {
                            continue;
                        }
                        let pitch = pitch as u8;
                        if notes.iter().any(|note| {
                            note.track_id == track_id
                                && note.pitch == pitch
                                && (note.start_beat - chord_beat).abs() < 0.01
                        }) {
                            continue;
                        }
                        notes.push(PianoNote {
                            track_id,
                            region_id: 0,
                            pitch,
                            midi_channel: 0,
                            articulation: 0,
                            velocity: 100,
                            start_beat: chord_beat,
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
                    if (0..=127).contains(&chord_root) {
                        let _ = core.add_chord_event(
                            (f64::from(chord_beat.max(0.0)) * 480.0).round() as u64,
                            chord_root as u8,
                            intervals
                                .iter()
                                .map(|interval| (*interval).clamp(0, 127) as u8)
                                .collect(),
                            if chord_quality == 0 { "major" } else { "minor" },
                        );
                    }
                }
                replace_notes(core, &prog_notes, notes);
            });
        piano_buttons.push(preset_button.into());
        piano_buttons.push(prog_button.into());
        // Articulation map (Slint set-articulation-map): MAP cycles the
        // articulation id; KS steps the keyswitch pitch (off, 60, down).
        let map_state = self.map_articulation_id.clone();
        let map_button = ButtonView::new(format!("MAP{}", self.map_articulation_id.get()))
            .action_id("piano.map_cycle")
            .secondary()
            .on_press(move |_| {
                let next = map_state.get() + 1;
                map_state.set(if next > 255 { 1 } else { next });
            });
        piano_buttons.push(map_button.into());
        let ks_core = self.core.clone();
        let ks_tracks = self.tracks.clone();
        let ks_track = self.piano_track.clone();
        let ks_map_id = self.map_articulation_id.clone();
        let ks_locked = self.arrange_locked.clone();
        let ks_current = tracks
            .iter()
            .find(|track| track.id == piano_track)
            .and_then(|track| {
                track
                    .expression_map
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == self.map_articulation_id.get())
                    .and_then(|entry| entry.keyswitch_pitch)
            });
        let ks_label = match ks_current {
            Some(pitch) => format!("KS{pitch}"),
            None => "KS OFF".to_owned(),
        };
        let ks_button = ButtonView::new(ks_label)
            .action_id("piano.map_keyswitch")
            .secondary()
            .on_press(move |_| {
                if ks_locked.get() {
                    return;
                }
                let Some(core) = &ks_core else { return };
                let track_id = ks_track.get();
                let articulation_id = ks_map_id.get();
                if !(1..=255).contains(&articulation_id) {
                    return;
                }
                let tracks = ks_tracks.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut entries = track.expression_map.clone();
                let current = entries
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == articulation_id)
                    .and_then(|entry| entry.keyswitch_pitch);
                let pitch: Option<u8> = match current {
                    None => Some(60),
                    Some(0) => None,
                    Some(pitch) => Some(pitch - 1),
                };
                let id = articulation_id as u8;
                if let Some(entry) = entries.iter_mut().find(|entry| entry.articulation_id == id) {
                    entry.outputs.retain(|output| {
                        !matches!(
                            output,
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                ..
                            }
                        )
                    });
                    if let Some(pitch) = pitch {
                        entry.outputs.insert(
                            0,
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        );
                        entry.keyswitch_pitch = Some(pitch);
                    } else {
                        entry.keyswitch_pitch = None;
                    }
                } else if let Some(pitch) = pitch {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name: format!("Articulation {id}"),
                        keyswitch_pitch: Some(pitch),
                        channel: 0,
                        group: 0,
                        outputs: vec![
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        ],
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                let accepted =
                    core.set_midi_expression_map(track_id, entries, articulation_id as u8);
                if accepted {
                    ks_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                }
            });
        piano_buttons.push(ks_button.into());
        // Articulation name field for the MAP id (Slint set-articulation-name).
        let name_core = self.core.clone();
        let name_tracks = self.tracks.clone();
        let name_track = self.piano_track.clone();
        let name_map_id = self.map_articulation_id.clone();
        let name_locked = self.arrange_locked.clone();
        let current_name = tracks
            .iter()
            .find(|track| track.id == piano_track)
            .and_then(|track| {
                track
                    .expression_map
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == self.map_articulation_id.get())
                    .map(|entry| entry.name.clone())
            })
            .unwrap_or_default();
        let name_text = Rc::new(RefCell::new(current_name.clone()));
        let name_change_text = name_text.clone();
        let name_submit_text = name_text.clone();
        let name_field = TextFieldView::new(current_name, "Articulation name")
            .change_action_id("piano.map_name")
            .submit_action_id("piano.map_name")
            .on_change(move |value| {
                *name_change_text.borrow_mut() = value;
            })
            .on_submit(move || {
                if name_locked.get() {
                    return;
                }
                let Some(core) = &name_core else { return };
                let track_id = name_track.get();
                let articulation_id = name_map_id.get();
                if !(1..=255).contains(&articulation_id) {
                    return;
                }
                let name = name_submit_text.borrow().clone();
                if name.len() > 128 || name.contains('\0') {
                    return;
                }
                let tracks = name_tracks.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut entries = track.expression_map.clone();
                let id = articulation_id as u8;
                if let Some(entry) = entries.iter_mut().find(|entry| entry.articulation_id == id) {
                    entry.name = name;
                } else {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name,
                        keyswitch_pitch: None,
                        channel: 0,
                        group: 0,
                        outputs: Vec::new(),
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                if core.set_midi_expression_map(track_id, entries, articulation_id as u8) {
                    name_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                }
            });
        piano_buttons.push(name_field.into());
        // Lyric editor for the selected note; commits on submit, like the
        // Slint lyric field. The text buffer is shared between the change
        // and submit closures so Enter commits the typed value.
        let lyric_notes = self.piano_notes.clone();
        let lyric_selected = self.selected_note.clone();
        let lyric_core = self.core.clone();
        let lyric_locked = self.arrange_locked.clone();
        let lyric_text = Rc::new(RefCell::new(
            self.selected_note
                .get()
                .and_then(|index| {
                    self.piano_notes
                        .get()
                        .get(index)
                        .map(|note| note.lyric.clone())
                })
                .unwrap_or_default(),
        ));
        let lyric_change_text = lyric_text.clone();
        let lyric_submit_text = lyric_text.clone();
        let lyric_field = TextFieldView::new(lyric_text.borrow().clone(), "lyric (Enter to apply)")
            .change_action_id("piano.lyric")
            .submit_action_id("piano.lyric")
            .on_change(move |value| {
                *lyric_change_text.borrow_mut() = value;
            })
            .on_submit(move || {
                if lyric_locked.get() {
                    return;
                }
                let Some(core) = &lyric_core else { return };
                let Some(index) = lyric_selected.get() else {
                    return;
                };
                let mut notes = lyric_notes.get();
                let Some(note) = notes.get_mut(index) else {
                    return;
                };
                let lyric = lyric_submit_text.borrow().clone();
                if lyric.len() > 1_024 || lyric.contains('\0') {
                    return;
                }
                note.lyric = lyric;
                replace_notes(core, &lyric_notes, notes);
            });
        piano_buttons.push(lyric_field.into());
        // Articulation stepper for the selected note (Slint set-note-articulation).
        for (label, delta) in [("ART-", -1), ("ART+", 1)] {
            let art_core = self.core.clone();
            let art_notes = self.piano_notes.clone();
            let art_selected = self.selected_note.clone();
            let art_locked = self.arrange_locked.clone();
            let button = ButtonView::new(label)
                .action_id(format!("piano.articulation.{label}"))
                .secondary()
                .on_press(move |_| {
                    if art_locked.get() {
                        return;
                    }
                    let Some(core) = &art_core else { return };
                    let Some(index) = art_selected.get() else {
                        return;
                    };
                    let mut notes = art_notes.get();
                    let Some(note) = notes.get_mut(index) else {
                        return;
                    };
                    note.articulation = (i32::from(note.articulation) + delta).clamp(0, 127) as u8;
                    replace_notes(core, &art_notes, notes);
                });
            piano_buttons.push(button.into());
        }
        let piano_selector = View::Card(vec![View::HStack(piano_buttons)]);

        let mut views = vec![
            project_bar,
            transport,
            meter,
            arrange_toolbar,
            View::canvas("waveform").grow(1),
            View::canvas("arrange").grow(3),
        ];
        if let Some(inspector) = inspector {
            views.push(inspector);
        }
        if let Some(plugin_strip) = plugin_strip {
            views.push(plugin_strip);
        }
        views.extend([piano_selector, View::canvas("piano").grow(2), track_list]);
        View::VStack(views)
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
            self.draw_arrange(scene, bounds, context);
            return;
        }
        if canvas_id == "piano" {
            self.draw_piano(scene, bounds, context);
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
