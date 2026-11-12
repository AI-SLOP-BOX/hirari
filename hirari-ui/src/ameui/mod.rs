//! AmeUI application shell — the sole UI surface.
//!
//! Slice 1 boots the real engine inside an AmeUI window: a transport bar
//! (play/stop, tempo, playhead), a live master meter, the shared plot
//! Canvas, and the track list read from the native project layout.
//! Slice 3 adds the arrangement Canvas: lanes, clips, measure ruler,
//! markers and playhead drawn from the neutral arrange model. Slice 4
//! makes that canvas seekable (click/drag moves the playhead); slice 5
//! adds zoom (toolbar −/+/FIT) and ruler-drag panning; slice 6 adds
//! click clip selection; slice 7 adds clip drag-move (live preview,
//! commit on release via the engine undo transaction); slice 8 adds
//! edge-drag trim (Audio/Vocal only, fraction-clamped like the arrange dock); slice 9 draws clip waveform peaks from the async decode
//! queue; slice 10 adds the snap division picker; slice 11 adds the
//! edit lock; slice 12 turns the track list into mixer strips
//! (fader/pan/mute/solo/arm); slice 13 adds project open/save/new;
//! slice 14 adds loop/metronome toggles; slice 15 adds the read-only
//! piano roll; slice 16 adds its edit gestures (add/move/resize/delete
//! via a full note-list replace); slice 17 adds quantize and velocity
//! stepping; slice 18 adds chord insert and delete-all; slice 19 adds
//! note lyric editing; slice 20 adds the articulation stepper;
//! slice 21 adds keyboard shortcuts (Space plays, L loops); slice 22
//! adds progression insert; slice 23 adds articulation-map keyswitch
//! editing; slice 24 adds articulation-name editing; slice 25 adds the
//! selected-clip fade/gain inspector; slice 26 adds the per-track
//! plugin strip; slice 27 adopts canvas text (lane/clip names, measure
//! numbers, pitch labels); slice 28 adopts canvas wheel (scroll and
//! Ctrl-wheel zoom); slice 29 adopts the native file dialog (async,
//! delivered on the poll); slice 30 adds dropped-file audio import;
//! slice 31 adopts dirty-canvas redraw; slice 32 adopts pinch/pan
//! gestures; slice 33 adopts theme colors in the canvases; slice 34
//! adds the plugin parameter strip; slice 35 adds drum mode;
//! slice 36 adds read-only automation overlays; slice 37 adds
//! automation editing (AUTO mode); slice 38 adds audio settings;
//! slice 39 adds device selection; slice 40 adds plugin preset
//! save/load; slice 41 adds the master gain slider; slice 42 adopts
//! slider release callbacks; slice 43 adopts position-aware cursors;
//! slice 44 adds canvas accessibility regions; slice 45 fixes the
//! waveform canvas height; slice 46 adds the audio library;
//! engine E2 adds the measured I/O latency readout. Audio playback is
//! real; this is the default build.
//!
//! HirariCore is deliberately not `Send` (its engine stores raw pointers), so
//! every engine read happens on the UI thread. A background ticker only sends
//! `AppMessage("tick")` through the event-loop proxy; `on_event` does the poll.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use ame_ui::app::{
    App, AppEventSender, CanvasAccessibilityItem, CanvasBounds, CanvasContext, CanvasGestureEvent,
    CanvasGestureKind, CanvasItemRole, CanvasPointerEvent, CanvasPointerPhase, CanvasScrollEvent,
    CanvasTextVAlign, State, StateContext, Viewport, WindowConfig,
};
use ame_ui::event::{AmeEvent, KeyModifiers};
use ame_ui::theme::Theme;
use ame_ui::view::{
    ButtonView, CheckboxView, SliderView, TextFieldView, TimelineMarker, TimelineView, View,
    ViewTree,
};
use ame_ui::CanvasTexture;
use hirari_core_bridge::HirariCore;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::wgpu;

use crate::ui_shared::arrange_model::{
    arrange_canvas_geometry, arrangement_measure_starts, arrangement_timeline_end, clip_at,
    last_clip_end, note_at, parse_arrange_tracks, parse_audio_devices, parse_comping_takes,
    parse_markers, parse_midi_notes, parse_preview_catalog, parse_track_stacks,
    piano_roll_geometry, plugin_name, trim_edge_at, valid_time_signatures, ArrangeCanvasGeometry,
    ArrangeMarker, ArrangeTrack, AudioDevice, CompTake, LibraryAsset, MeasureStart, PianoNote,
    PianoRollGeometry, TrimEdge, ARRANGE_ZOOM_MAX, ARRANGE_ZOOM_MIN,
};
use crate::ui_shared::gpu_canvas::PlotFrameStore;
use crate::ui_shared::moufu_connection::{load_moufu_address, save_moufu_address};
use crate::ui_shared::moufu_preview::{accept_moufu_version, linked_project_preview};
use crate::ui_shared::plot_raster;
use crate::ui_shared::render_ops::{
    cancellation_status, kick_stem, prepare_rejected, render_format_label, render_tail_seconds,
    stem_step, StemStep,
};
use crate::ui_shared::render_path::default_render_output_path;
use crate::ui_shared::template_starter::create_template_project;
use hirari_core_bridge::project_contracts::MidiNoteContract;
use hirari_moufu_adapter::{
    LinkStatus, MoufuEvent, HIRARI_PROJECT_DATA_TYPE, HIRARI_PROJECT_ENTITY,
};
use std::sync::atomic::Ordering;

enum SpotlightCommand {
    TogglePlay,
    ToggleLoop,
    ToggleMetronome,
    Save,
    New,
    SnapCycle,
    ZoomIn,
    ZoomOut,
    Fit,
    RenderMaster,
    ExportMidi,
    LockToggle,
    Undo,
}

/// Helper used by the spotlight palette to pick a reasonable default save path.
fn core_default_project_path() -> String {
    // Mirror a default save: most recent path fallback to a temp dir-derived name.
    std::env::temp_dir()
        .join("hirari-spotlight.hirari")
        .to_string_lossy()
        .into_owned()
}

/// Snap divisions offered by the toolbar picker (`snap_val`
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
    /// A MIDI clip: select on press, drag does nothing (parity).
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

/// In-progress automation gesture on one lane.
struct AutoEdit {
    track_id: u32,
    lane: u32,
    /// Point index in the lane's point list; `None` for an added point.
    point_index: Option<usize>,
    down_x: f64,
    down_y: f64,
    orig_beat: f32,
    orig_value: f32,
    delta_beats: f32,
    delta_value: f32,
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

/// Re-alphas a theme color for canvas fills/strokes, so canvases follow
/// the active theme instead of hard-coded RGB values.
fn themed(color: vello::peniko::Color, alpha: u8) -> Color {
    let rgba = color.to_rgba8().to_u8_array();
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], alpha)
}

/// Converts the parsed notes back to engine contracts for a full-list
/// replace, with the same field clamping as the commit path.
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
    show_genesis: State<bool>,
    plots: PlotFrameStore,
    playing: State<bool>,
    loop_enabled: State<bool>,
    metronome_enabled: State<bool>,
    tempo: State<f32>,
    playhead_beats: State<f64>,
    sample_rate: State<f64>,
    audio_buffer: State<u32>,
    audio_ready: State<bool>,
    io_latency_ms: State<f32>,
    device_count: State<u32>,
    audio_devices: State<Vec<AudioDevice>>,
    device_index: State<u32>,
    master_peak: State<f32>,
    master_gain: State<f32>,
    tracks: State<Vec<ArrangeTrack>>,
    markers: State<Vec<ArrangeMarker>>,
    measure_starts: State<Vec<MeasureStart>>,
    timeline_end_beats: State<f32>,
    arrange_zoom: State<f32>,
    arrange_scroll_beats: State<f32>,
    /// Snap division denominator (default 16); step is 4/division.
    arrange_snap_division: State<f32>,
    /// Edit lock (`is_locked`): while on, clip drags select only and
    /// never move or trim. Seek and pan still work.
    arrange_locked: State<bool>,
    /// Automation edit mode: lane gestures edit the active automation lane
    /// instead of seeking/moving. Lane 0 = volume, 1 = pan.
    auto_mode: State<bool>,
    auto_lane: State<u32>,
    /// Selected automation point index in the active lane.
    auto_point: State<Option<usize>>,
    /// Track of the selected automation point (for DELETE).
    auto_track: State<u32>,
    /// Export format (engine native numbering: 0=PCM16, 1=Wave64,
    /// 2=Float32, 3=PCM24).
    export_format: State<u32>,
    /// Effect-tail preset index into `[0, 1, 2, 5, 10, 30]`f32 seconds.
    export_tail_index: State<u32>,
    /// Set while the stems queue is armed; the poll then watches for
    /// engine bounce completion and advances the batch, like the
    /// render telemetry.
    stem_export_active: State<bool>,
    /// Caption shown inside the export card (render state / progress /
    /// stem counts).
    render_caption: State<String>,
    /// Currently selected track-stack id for the STACKS card (0 = none).
    stack_selected_id: State<u32>,
    /// Gain in the persisted stack, mirrored from `track_stacks_json()`.
    stack_gain: State<f32>,
    /// Collapsed state mirrored from `track_stacks_json()`.
    stack_collapsed: State<bool>,
    /// Label of the currently selected stack from `track_stacks_json()`.
    stack_name: State<String>,
    /// Sync-group input text for flex-pitch.
    flex_sync_group_text: State<String>,
    /// Strength for the quantize-region-sync-group operation (0..1).
    flex_quantize_strength: State<f32>,
    /// Grid index fed to the engine (index into the GRID_BEATS table).
    flex_quantize_grid: State<u32>,
    /// Swing ratio for the quantize call.
    flex_quantize_swing: State<f32>,
    /// The last comping snapshot, refreshed each poll tick.
    comping_takes: State<Vec<CompTake>>,
    /// Crossfade (0..1 normalized) applied by the COMPING card's slider.
    comp_crossfade: State<f32>,
    /// Synth engine selector: 0=SYNTH, 1=WAVETABLE, 2=SAMPLER.
    synth_engine: State<u32>,
    /// Local echo of the 8 engine macro values (no engine getter exists).
    synth_macros: State<[f32; 8]>,
    /// Focus mode toggle: hide everything except transport, meter and the
    /// arrange surface. Mirrors the focus setting exactly.
    /// Spotlight palette: toggled with Cmd/Ctrl+K or the project-bar button.
    show_spotlight: State<bool>,
    spotlight_query: State<String>,
    focus_mode: State<bool>,
    /// System diagnostics panel toggle (diagnostics page).
    show_diagnostics: State<bool>,
    /// Accumulated AU watchdog trips (engine polls since connect).
    watchdog_trips: State<u32>,
    /// Moufu Hub current address typed in the link card; empty until
    /// configured. Persisted via ui_shared::moufu_connection.
    moufu_address: State<String>,
    moufu_status: State<String>,
    moufu_entity_labels: State<Vec<String>>,
    moufu_entity_ids: State<Vec<String>>,
    moufu_entity_selected: State<u32>,
    moufu_link_labels: State<Vec<String>>,
    moufu_link_ids: State<Vec<String>>,
    moufu_link_selected: State<u32>,
    moufu_preview: State<String>,
    moufu_remote_source: State<String>,
    /// Moufu connector; None in smoke/headless mode.
    moufu_publisher: Option<hirari_moufu_adapter::MoufuPublisher>,
    moufu_outbound_link_ids: std::sync::Mutex<std::collections::HashSet<String>>,
    moufu_last_revision: std::sync::Mutex<Option<u64>>,
    moufu_remote_versions: std::sync::Mutex<std::collections::HashMap<String, (u64, bool)>>,
    /// Outbound link gating the cadenced snapshot publication.
    /// Pending video load; set by the LOAD VIDEO dialog, drained by the
    /// poll via `core.load_video`.
    pending_video_load: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    moufu_has_outbound: std::sync::atomic::AtomicBool,
    /// Moufu card toggle.
    show_moufu: State<bool>,
    /// Selected tempo-event index in the engine's tempo map.
    tempo_event_selected: State<u32>,
    /// Ramp mode for the next ADD EVT: true = linear ramp, false = step.
    tempo_event_ramp: State<bool>,
    /// Tap-tempo samples, newest last, capped at five.
    tempo_taps: std::rc::Rc<std::cell::RefCell<std::collections::VecDeque<std::time::Instant>>>,
    /// Time signature selector: 0=4/4 1=3/4 2=6/8 3=7/8.
    tempo_time_signature: State<u32>,
    /// Loudness snapshots polled from the engine (mastering readouts).
    integrated_lufs: State<f32>,
    true_peak_l: State<f32>,
    true_peak_r: State<f32>,
    /// Selected clip as `(track_id, clip_id)`, mirroring the dock
    /// selection the later drag/trim slice will mutate through.
    selected_clip: State<Option<(u32, i32)>>,
    /// Decoded peak vectors per clip, keyed by `(track_id, clip_id)`.
    /// Filled asynchronously; see `poll_waveforms`.
    clip_peaks: State<HashMap<(u32, i32), Vec<f32>>>,
    /// Piano roll track (0 = unset; defaults to the first track with
    /// notes) and its parsed notes.
    piano_track: State<u32>,
    piano_notes: State<Vec<PianoNote>>,
    /// Drum mode: the roll shows GM drum lanes instead of pitch rows.
    drum_mode: State<bool>,
    /// Chord-insert settings (piano roll chord controls).
    chord_root: State<u32>,
    chord_quality: State<u32>,
    chord_length: State<f32>,
    chord_inversion: State<u32>,
    progression_preset: State<u32>,
    /// Articulation-map id being edited (1..255).
    map_articulation_id: State<u32>,
    /// Plugin type the ADD button installs (built-in chain 0..=10).
    add_plugin_type: State<u32>,
    /// Plugin whose parameters are shown (index into the chain).
    selected_plugin: State<u32>,
    /// Selected note index into `piano_notes` (for delete).
    selected_note: State<Option<usize>>,
    status: State<String>,
    waveform_image: Option<CanvasTexture>,
    waveform_reusable: Option<wgpu::Texture>,
    /// MusicXML export pending path; the poll writes it via the engine
    /// and refreshes the status.
    pending_musicxml: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// OpenUtau vocal score orchestration (P13). Source and render are
    /// picked independently because the engine's import flow requires both
    /// the score and the rendered stem; importing runs through the poll.
    openutau_caption: State<String>,
    openutau_source: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    openutau_render: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    pending_openutau:
        std::sync::Arc<std::sync::Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>>>,
    /// Send matrix panel (aux sends / audio routing parity):
    /// source/bus selectors, gain, pre/post, enable.
    show_sends: State<bool>,
    send_source: State<u32>,
    send_dest: State<u32>,
    send_gain: State<f32>,
    send_pre: State<bool>,
    send_enabled: State<bool>,
    /// Video monitor (Z_VideoMonitor parity): raw RGBA frames from
    /// the engine, uploaded to a reusable canvas texture on the draw path
    /// (not on the poll), revision-tracked like the waveform upload.
    video_image: Option<CanvasTexture>,
    video_reusable: Option<wgpu::Texture>,
    video_frame: Option<Vec<u8>>,
    video_size: Option<(u32, u32)>,
    video_revision: std::sync::Mutex<u64>,
    show_video: State<bool>,
    /// Last drawn arrange layout, used to map pointer x back to a beat.
    arrange_geometry: ArrangeCanvasGeometry,
    piano_geometry: PianoRollGeometry,
    piano_edit: Option<PianoEdit>,
    auto_edit: Option<AutoEdit>,
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
    /// Paths chosen in async native dialogs, delivered to the UI thread
    /// by the poll (the engine is not `Send`, so dialogs never touch it).
    pending_open: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    pending_save: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Chosen preset file plus its target track/plugin for load/save.
    pending_preset: std::sync::Arc<std::sync::Mutex<Option<(std::path::PathBuf, bool, u32, u32)>>>,
    /// Chosen library folder awaiting the UI-thread scan.
    pending_library: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// MIDI export path; a .mid save dialog drops its path here and the
    /// poll executes the engine export.
    pending_midi: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Directory output for a stem export, set by the folder picker; the
    /// poll arms core's batch on arrival.
    pending_stems: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    library_assets: State<Vec<LibraryAsset>>,
    library_query: State<String>,
    /// Last arrange canvas bounds, to map a file drop to lane/beat.
    arrange_bounds: (f32, f32, f32, f32),
    /// Last drawn inputs for the dirty-canvas check (see
    /// `canvas_scene_dirty`). Bumped on layout refreshes and piano edits.
    dirty_playhead: Cell<f64>,
    dirty_zoom: Cell<f32>,
    dirty_scroll: Cell<f32>,
    dirty_selected: Cell<Option<(u32, i32)>>,
    dirty_piano: Cell<(u32, u64)>,
    dirty_layout: Cell<u64>,
    dirty_layout_seen: Cell<u64>,
    /// In-flight dropped-file audio import: (ticket, path, track).
    pending_import: Option<(u64, String, u32)>,
    /// Library import ticket slot (dialog/async path shares the poll).
    library_ticket: std::sync::Arc<std::sync::Mutex<Option<(u64, String, u32)>>>,
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
        audio_buffer: context.state(256u32),
        audio_ready: context.state(false),
        io_latency_ms: context.state(0.0f32),
        device_count: context.state(0u32),
        audio_devices: context.state(Vec::<AudioDevice>::new()),
        device_index: context.state(0u32),
        master_peak: context.state(0.0f32),
        master_gain: context.state(1.0f32),
        show_genesis: context.state(true),
        tracks: context.state(Vec::<ArrangeTrack>::new()),
        markers: context.state(Vec::<ArrangeMarker>::new()),
        measure_starts: context.state(Vec::<MeasureStart>::new()),
        timeline_end_beats: context.state(128.0f32),
        arrange_zoom: context.state(1.0f32),
        arrange_scroll_beats: context.state(0.0f32),
        arrange_snap_division: context.state(16.0f32),
        arrange_locked: context.state(false),
        auto_mode: context.state(false),
        auto_lane: context.state(0u32),
        auto_point: context.state(None),
        auto_track: context.state(0u32),
        export_format: context.state(0u32),
        export_tail_index: context.state(2u32),
        stem_export_active: context.state(false),
        render_caption: context.state(String::new()),
        stack_selected_id: context.state(0u32),
        stack_gain: context.state(1.0f32),
        stack_collapsed: context.state(false),
        stack_name: context.state(String::new()),
        flex_sync_group_text: context.state("0".to_owned()),
        flex_quantize_strength: context.state(1.0f32),
        flex_quantize_grid: context.state(2u32),
        flex_quantize_swing: context.state(0.0f32),
        comping_takes: context.state(Vec::new()),
        comp_crossfade: context.state(0.01f32),
        show_spotlight: context.state(false),
        spotlight_query: context.state(String::new()),
        focus_mode: context.state(crate::ui_shared::preferences::load_focus_mode()),
        synth_engine: context.state(1u32),
        synth_macros: context.state([0.5f32; 8]),
        show_diagnostics: context.state(false),
        watchdog_trips: context.state(0u32),
        moufu_address: context.state(load_moufu_address().unwrap_or_default()),
        moufu_status: context.state("MOUFU NOT CONFIGURED".to_owned()),
        moufu_entity_labels: context.state(Vec::new()),
        moufu_entity_ids: context.state(Vec::new()),
        moufu_entity_selected: context.state(0u32),
        moufu_link_labels: context.state(Vec::new()),
        moufu_link_ids: context.state(Vec::new()),
        moufu_link_selected: context.state(0u32),
        moufu_preview: context.state("No linked project state received.".to_owned()),
        moufu_remote_source: context.state(String::new()),
        // The connector is started with the same env var+settings the dock
        // app honors (HIRARI_MOUFU_ADDR, else the persisted address).
        moufu_publisher: hirari_moufu_adapter::MoufuPublisher::start_from_env().map(|publisher| {
            let address = load_moufu_address().unwrap_or_default();
            if !address.trim().is_empty() {
                let _ = publisher.set_address(Some(address.trim()));
            }
            publisher
        }),
        moufu_outbound_link_ids: std::sync::Mutex::new(std::collections::HashSet::new()),
        moufu_last_revision: std::sync::Mutex::new(None),
        moufu_remote_versions: std::sync::Mutex::new(std::collections::HashMap::new()),
        pending_video_load: std::sync::Arc::new(std::sync::Mutex::new(None)),
        moufu_has_outbound: std::sync::atomic::AtomicBool::new(false),
        show_moufu: context.state(false),
        tempo_event_selected: context.state(0u32),
        tempo_event_ramp: context.state(true),
        tempo_taps: std::rc::Rc::new(std::cell::RefCell::new(std::collections::VecDeque::new())),
        tempo_time_signature: context.state(0u32),
        integrated_lufs: context.state(-120.0f32),
        true_peak_l: context.state(-120.0f32),
        true_peak_r: context.state(-120.0f32),
        selected_clip: context.state(None),
        clip_peaks: context.state(HashMap::new()),
        piano_track: context.state(0u32),
        piano_notes: context.state(Vec::<PianoNote>::new()),
        drum_mode: context.state(false),
        chord_root: context.state(60u32),
        chord_quality: context.state(0u32),
        chord_length: context.state(4.0f32),
        chord_inversion: context.state(0u32),
        progression_preset: context.state(0u32),
        map_articulation_id: context.state(1u32),
        add_plugin_type: context.state(0u32),
        selected_plugin: context.state(0u32),
        selected_note: context.state(None),
        status: context.state(status),
        waveform_image: None,
        waveform_reusable: None,
        video_image: None,
        video_reusable: None,
        video_frame: None,
        video_size: None,
        video_revision: std::sync::Mutex::new(0),
        openutau_caption: context.state(String::new()),
        openutau_source: std::sync::Arc::new(std::sync::Mutex::new(None)),
        openutau_render: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_openutau: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_musicxml: std::sync::Arc::new(std::sync::Mutex::new(None)),
        show_sends: context.state(false),
        send_source: context.state(0u32),
        send_dest: context.state(0u32),
        send_gain: context.state(0.5f32),
        send_pre: context.state(false),
        send_enabled: context.state(false),
        show_video: context.state(false),
        arrange_geometry: arrange_canvas_geometry(0.0, 0.0, 128.0, 0, 1.0, 0.0),
        piano_geometry: piano_roll_geometry(0.0, 0.0, 128.0, &[]),
        piano_edit: None,
        auto_edit: None,
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
        pending_open: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_save: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_preset: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_library: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_midi: std::sync::Arc::new(std::sync::Mutex::new(None)),
        pending_stems: std::sync::Arc::new(std::sync::Mutex::new(None)),
        library_assets: context.state(Vec::<LibraryAsset>::new()),
        library_query: context.state(String::new()),
        arrange_bounds: (0.0, 0.0, 0.0, 0.0),
        library_ticket: std::sync::Arc::new(std::sync::Mutex::new(None)),
        dirty_playhead: Cell::new(f64::NAN),
        dirty_zoom: Cell::new(-1.0),
        dirty_scroll: Cell::new(-1.0),
        dirty_selected: Cell::new(Some((u32::MAX, 0))),
        dirty_piano: Cell::new((0, 424_242)),
        dirty_layout: Cell::new(424_242),
        dirty_layout_seen: Cell::new(424_243),
        pending_import: None,
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
    /// Drives the offline-render flow while a render or a stems batch is
    /// in flight, mirroring the `telemetry_render` cadence: the
    /// engine reports bounce progress/completion and the stems queue is
    /// advanced here, never on the UI event path.
    fn poll_render(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let Some((state, progress)) = core.get_bounce_status() else {
            return;
        };
        match state {
            3 => {
                if !self.stem_export_active.get() {
                    self.render_caption.set("RENDER COMPLETE".to_owned());
                    return;
                }
                match stem_step(&core.stem_batch_settle(true)) {
                    StemStep::StemDone { rendered, total } => {
                        match stem_step(&core.stem_batch_next()) {
                            StemStep::Ready { output, format, .. } => {
                                if kick_stem(&core, &output, format) {
                                    self.render_caption.set(format!(
                                        "STEM {rendered} OF {total} COMPLETE · NEXT QUEUED"
                                    ));
                                } else {
                                    core.stem_batch_abort();
                                    self.stem_export_active.set(false);
                                    self.render_caption.set(format!(
                                        "could not queue stem {} of {total}",
                                        rendered + 1
                                    ));
                                }
                            }
                            StemStep::Failed(reason) => {
                                core.stem_batch_abort();
                                self.stem_export_active.set(false);
                                self.render_caption.set(reason);
                            }
                            _ => {
                                core.stem_batch_abort();
                                self.stem_export_active.set(false);
                                self.render_caption
                                    .set("stem export queue is invalid".to_owned());
                            }
                        }
                    }
                    StemStep::Complete { outputs, .. } => {
                        self.stem_export_active.set(false);
                        self.render_caption
                            .set(format!("STEM EXPORT COMPLETE · {} FILES", outputs.len()));
                    }
                    StemStep::Failed(reason) => {
                        self.stem_export_active.set(false);
                        self.render_caption.set(reason);
                    }
                    StemStep::Ready { .. } => {
                        core.stem_batch_abort();
                        self.stem_export_active.set(false);
                        self.render_caption
                            .set("stem export reported an unexpected state".to_owned());
                    }
                }
            }
            4 => {
                if self.stem_export_active.get() {
                    let reason = match stem_step(&core.stem_batch_settle(false)) {
                        StemStep::Failed(reason) => reason,
                        _ => "native stem render failed".to_owned(),
                    };
                    self.render_caption.set(reason);
                } else {
                    self.render_caption.set("RENDER FAILED".to_owned());
                }
                self.stem_export_active.set(false);
            }
            5 => {
                if self.stem_export_active.get() {
                    core.stem_batch_abort();
                }
                self.stem_export_active.set(false);
                self.render_caption.set("RENDER CANCELLED".to_owned());
            }
            1 | 2 => {
                self.render_caption.set(format!(
                    "RENDERING {:.0}%",
                    progress.clamp(0.0, 1.0) * 100.0
                ));
            }
            _ => {}
        }
    }

    /// Video frame poll (control rate, follows playhead): requests the
    /// frame for the playhead position and stashes it in `self.video_frame`
    /// when a new revision lands. The actual texture upload happens on the
    /// draw path, like the waveform.
    fn poll_video(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        if !self.show_video.get() {
            return;
        }
        let sample_rate = core.get_sample_rate().max(1.0);
        let seconds = core.get_playhead() as f64 / sample_rate;
        let _ = core.request_video_frame(seconds);
        let revision = core.get_video_frame_revision();
        let last = self.video_revision.lock().map(|last| *last).unwrap_or(0);
        if revision == 0 || revision == last {
            return;
        }
        let frame = core.get_video_frame();
        if frame.len() < 8 {
            return;
        }
        let width = u32::from_le_bytes(frame[0..4].try_into().unwrap_or([0; 4]));
        let height = u32::from_le_bytes(frame[4..8].try_into().unwrap_or([0; 4]));
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if width == 0 || height == 0 || width > 8192 || height > 8192 || frame.len() != expected + 8
        {
            return;
        }
        self.video_frame = Some(frame[8..].to_vec());
        self.video_size = Some((width, height));
        if let Ok(mut slot) = self.video_revision.lock() {
            *slot = revision;
        }
    }

    /// Moufu connector poll. Same event discipline as the dock's live
    /// link timer: publish the snapshot when it changes while an outbound
    /// link is registered, and mirror the queue effects into the card.
    /// Runs every 8th tick (~250ms) so the drain does not contend with the
    /// 30 Hz engine poll.
    fn poll_moufu(&mut self) {
        let Some(publisher) = self.moufu_publisher.clone() else {
            return;
        };
        if self.tick % 8 != 0 {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        if self.moufu_has_outbound.load(Ordering::Acquire) {
            let revision = core.project_layout_revision();
            let should_publish = self
                .moufu_last_revision
                .lock()
                .map(|last| *last != Some(revision))
                .unwrap_or(true);
            if should_publish {
                crate::ui_shared::moufu_layout::publish_project_snapshot(
                    &publisher,
                    &core,
                    Vec::new(),
                    true,
                );
                if let Ok(mut last) = self.moufu_last_revision.lock() {
                    *last = Some(revision);
                }
            }
        }
        for event in publisher.drain_events() {
            match event {
                MoufuEvent::ConnectionConfigured { enabled } => {
                    self.moufu_outbound_link_ids
                        .lock()
                        .map(|mut set| set.clear())
                        .unwrap_or(());
                    self.moufu_has_outbound.store(false, Ordering::Release);
                    self.moufu_last_revision
                        .lock()
                        .map(|mut rev| *rev = None)
                        .unwrap_or(());
                    self.moufu_remote_versions
                        .lock()
                        .map(|mut remote| remote.clear())
                        .unwrap_or(());
                    self.moufu_entity_labels.set(Vec::new());
                    self.moufu_entity_ids.set(Vec::new());
                    self.moufu_link_labels.set(Vec::new());
                    self.moufu_link_ids.set(Vec::new());
                    self.moufu_entity_selected.set(0);
                    self.moufu_link_selected.set(0);
                    self.moufu_remote_source.set(String::new());
                    self.moufu_preview
                        .set("No linked project state received.".to_owned());
                    self.moufu_status.set(if enabled {
                        "MOUFU CONNECTING".to_owned()
                    } else {
                        "MOUFU NOT CONFIGURED".to_owned()
                    });
                }
                MoufuEvent::Connected { server_version } => {
                    log::info!("Connected to Moufu server {server_version}");
                    self.moufu_status.set("MOUFU CONNECTED".to_owned());
                    let _ = publisher.list_entities();
                    let _ = publisher.list_links();
                }
                MoufuEvent::Disconnected { reason } => {
                    log::debug!("Moufu reconnect scheduled: {reason}");
                    self.moufu_outbound_link_ids
                        .lock()
                        .map(|mut set| set.clear())
                        .unwrap_or(());
                    self.moufu_has_outbound.store(false, Ordering::Release);
                    self.moufu_last_revision
                        .lock()
                        .map(|mut rev| *rev = None)
                        .unwrap_or(());
                    self.moufu_status.set("MOUFU RECONNECTING".to_owned());
                    self.status
                        .set(format!("MOUFU INTERRUPTED; RETRYING: {reason}"));
                }
                MoufuEvent::EntityList(entities) => {
                    let project_entities: Vec<_> = entities
                        .iter()
                        .filter(|entity| {
                            entity.data_type == HIRARI_PROJECT_DATA_TYPE
                                && entity.id.0 != HIRARI_PROJECT_ENTITY
                        })
                        .collect();
                    let labels: Vec<String> = project_entities
                        .iter()
                        .map(|entity| {
                            format!("{} · {} · {}", entity.source_app, entity.name, entity.id.0)
                        })
                        .collect();
                    let ids: Vec<String> = project_entities
                        .iter()
                        .map(|entity| entity.id.0.clone())
                        .collect();
                    let count = labels.len();
                    if !labels.is_empty() {
                        self.moufu_entity_selected.set(0);
                    }
                    self.moufu_entity_labels.set(labels);
                    self.moufu_entity_ids.set(ids);
                    self.moufu_status
                        .set(format!("MOUFU CONNECTED · {count} PROJECT ENTITIES"));
                }
                MoufuEvent::LinkList(links) => {
                    let outbound: HashSet<String> = links
                        .iter()
                        .filter(|link| {
                            link.source_entity.0 == HIRARI_PROJECT_ENTITY
                                && link.target_entity.0 != HIRARI_PROJECT_ENTITY
                                && link.status == LinkStatus::Active
                        })
                        .map(|link| link.link_id.to_string())
                        .collect();
                    let had_new = outbound.iter().any(|id| {
                        self.moufu_outbound_link_ids
                            .lock()
                            .map(|set| !set.contains(id))
                            .unwrap_or(true)
                    });
                    if had_new {
                        // A freshly linked peer needs the current snapshot
                        // even when the project has not moved.
                        let _ = publisher.republish_latest_layout();
                        if let Ok(mut rev) = self.moufu_last_revision.lock() {
                            *rev = Some(core.project_layout_revision());
                        }
                    }
                    if outbound.is_empty() {
                        self.moufu_last_revision
                            .lock()
                            .map(|mut rev| *rev = None)
                            .unwrap_or(());
                    }
                    if let Ok(mut set) = self.moufu_outbound_link_ids.lock() {
                        *set = outbound.clone();
                    }
                    self.moufu_has_outbound
                        .store(!outbound.is_empty(), Ordering::Release);
                    let relevant: Vec<_> = links
                        .iter()
                        .filter(|link| {
                            link.source_entity.0 == HIRARI_PROJECT_ENTITY
                                || link.target_entity.0 == HIRARI_PROJECT_ENTITY
                        })
                        .collect();
                    let labels: Vec<String> = relevant
                        .iter()
                        .map(|link| {
                            format!(
                                "{} → {} · {:?}",
                                link.source_entity.0, link.target_entity.0, link.status
                            )
                        })
                        .collect();
                    let ids: Vec<String> = relevant
                        .iter()
                        .map(|link| link.link_id.to_string())
                        .collect();
                    let count = labels.len();
                    if !labels.is_empty() {
                        self.moufu_link_selected.set(0);
                    }
                    // Forget snapshots for sources whose links went away.
                    let inbound_sources: HashSet<&str> = links
                        .iter()
                        .filter(|link| {
                            link.target_entity.0 == HIRARI_PROJECT_ENTITY
                                && link.status == LinkStatus::Active
                        })
                        .map(|link| link.source_entity.0.as_str())
                        .collect();
                    if let Ok(mut versions) = self.moufu_remote_versions.lock() {
                        versions.retain(|source, _| inbound_sources.contains(source.as_str()));
                    }
                    self.moufu_entity_ids.get();
                    let remote_source = self.moufu_remote_source.get();
                    if !remote_source.is_empty()
                        && !inbound_sources.contains(remote_source.as_str())
                    {
                        self.moufu_remote_source.set(String::new());
                        self.moufu_preview
                            .set("No active inbound project link.".to_owned());
                    }
                    self.moufu_link_labels.set(labels);
                    self.moufu_link_ids.set(ids);
                    self.moufu_status
                        .set(format!("MOUFU CONNECTED · {count} PROJECT LINKS"));
                }
                MoufuEvent::EntityUpdated(change) => {
                    let accepted = self
                        .moufu_remote_versions
                        .lock()
                        .map(|mut versions| {
                            accept_moufu_version(
                                &mut versions,
                                &change.entity_id.0,
                                change.version,
                                change.is_transient,
                            )
                        })
                        .unwrap_or(false);
                    if accepted {
                        let (source, preview) = linked_project_preview(
                            &change.entity_id.0,
                            change.version,
                            change.is_transient,
                            &change.payload,
                        );
                        self.moufu_remote_source.set(source);
                        self.moufu_preview.set(preview);
                    }
                }
                MoufuEvent::EntityState {
                    entity_id,
                    version,
                    payload,
                } => {
                    let accepted = self
                        .moufu_remote_versions
                        .lock()
                        .map(|mut versions| {
                            accept_moufu_version(&mut versions, &entity_id.0, version, false)
                        })
                        .unwrap_or(false);
                    if accepted {
                        let (source, preview) =
                            linked_project_preview(&entity_id.0, version, false, &payload);
                        self.moufu_remote_source.set(source);
                        self.moufu_preview.set(preview);
                    }
                }
                _ => {}
            }
        }
    }

    /// Refresh the persisted track-stack view once per tune; the engine
    /// owns the truth, so any local toggles are re-read from JSON.
    fn poll_track_stacks(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let persisted = core.track_stacks_json();
        let stacks = parse_track_stacks(&persisted);
        let selected = self.stack_selected_id.get();
        match stacks.iter().find(|stack| stack.id == selected) {
            Some(stack) => {
                self.stack_gain.set(stack.master_gain.clamp(0.0, 2.0));
                self.stack_collapsed.set(stack.collapsed);
                self.stack_name.set(stack.name.clone());
            }
            None => {
                if selected != 0 {
                    self.stack_selected_id.set(0);
                    self.stack_gain.set(1.0);
                    self.stack_collapsed.set(false);
                    self.stack_name.set(String::new());
                }
            }
        }
    }

    fn poll_engine(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        self.poll_render();
        self.poll_moufu();
        self.poll_track_stacks();
        let snapshot = core.comping_snapshot_json();
        self.comping_takes.set(parse_comping_takes(&snapshot));
        if let Some(path) = self
            .pending_video_load
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            if core.load_video(&path.to_string_lossy()) {
                self.status.set(format!(
                    "VIDEO LOADED: {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned())
                ));
            } else {
                self.status.set("VIDEO LOAD FAILED".to_owned());
            }
        }
        self.poll_video();
        // Diagnostics counters accumulate from the engine every tick.
        self.watchdog_trips.set(
            self.watchdog_trips
                .get()
                .saturating_add(core.take_watchdog_trips()),
        );
        self.playing.set(core.is_playing());
        self.audio_ready.set(core.is_audio_device_ready());
        self.io_latency_ms
            .set((core.io_latency_seconds() * 1000.0) as f32);
        self.loop_enabled.set(core.is_loop_enabled());
        self.metronome_enabled.set(core.is_metronome_enabled());
        self.tempo.set(core.get_tempo());
        self.sample_rate.set(core.get_sample_rate());
        self.playhead_beats
            .set(core.samples_to_beats(core.get_playhead()));

        let loudness = core.get_master_loudness();
        self.integrated_lufs.set(loudness.integrated);
        self.true_peak_l.set(loudness.true_peak_l);
        self.true_peak_r.set(loudness.true_peak_r);
        let peak = 10.0f32
            .powf(loudness.true_peak_l / 20.0)
            .max(10.0f32.powf(loudness.true_peak_r / 20.0))
            .clamp(0.0, 1.0);
        self.master_peak.set(peak);
        self.master_gain.set(core.master_gain());
        self.plots.publish_meters(&[peak, peak]);

        // The layout walk is comparatively costly; refresh it a few times a
        // second rather than on every 30 Hz tick.
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 10 == 1 {
            self.refresh_arrange_state(&core);
            self.poll_waveforms(&core);
            let devices = parse_audio_devices(&core.list_audio_devices_json());
            self.device_count.set(devices.len() as u32);
            self.audio_devices.set(devices);
        }
        // Library imports complete asynchronously; same ticket poll.
        if self.show_sends.get() {
            if let Ok(routes) = serde_json::from_str::<serde_json::Value>(&core.audio_routes_json())
            {
                if let Some(routes) = routes.as_array() {
                    let source_id = self
                        .tracks
                        .get()
                        .get(self.send_source.get() as usize)
                        .map(|t| t.id)
                        .unwrap_or(0);
                    let dest_id = self
                        .tracks
                        .get()
                        .iter()
                        .filter(|t| t.kind == "Bus")
                        .nth(self.send_dest.get() as usize)
                        .map(|t| t.id)
                        .unwrap_or(0);
                    let route = routes.iter().find(|route| {
                        route
                            .get("send")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                            && route.get("source_id").and_then(serde_json::Value::as_u64)
                                == Some(u64::from(source_id))
                            && route
                                .get("destination_id")
                                .and_then(serde_json::Value::as_u64)
                                == Some(u64::from(dest_id))
                    });
                    if let Some(route) = route {
                        self.send_enabled.set(true);
                        if let Some(gain) = route.get("gain").and_then(serde_json::Value::as_f64) {
                            self.send_gain.set(gain.clamp(0.0, 1.0) as f32);
                        }
                        self.send_pre.set(
                            route
                                .get("pre_fader")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        );
                    } else {
                        self.send_enabled.set(false);
                    }
                }
            }
        }
        if let Some(ticket) = self
            .library_ticket
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            if self.pending_import.is_none() {
                self.pending_import = Some(ticket);
            } else {
                let _ = core.complete_region_import(ticket.0);
            }
        }
        // Dropped-file imports complete asynchronously; poll the ticket
        // each tick like the import poller.
        if let Some((ticket, path, imported_track)) = self.pending_import.take() {
            match core.complete_region_import(ticket) {
                0 => self.pending_import = Some((ticket, path, imported_track)),
                1 => {
                    let _ = core.register_preview_audio(&path);
                    self.status.set(format!(
                        "IMPORTED: {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or(path)
                    ));
                    self.refresh_arrange_state(&core);
                }
                _ => self.status.set("IMPORT FAILED".to_owned()),
            }
        }
        // Library folder scans execute here (scan_preview_audio indexes
        // the filesystem, so it runs on the poll, not the dialog thread).
        if let Some(path) = self
            .pending_library
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.scan_preview_audio(&path.to_string_lossy()) {
                Ok(count) => {
                    self.library_assets
                        .set(parse_preview_catalog(&core.preview_audio_catalog_json()));
                    self.status.set(format!("LIBRARY SCANNED: {} items", count));
                }
                Err(_) => self.status.set("LIBRARY SCAN FAILED".to_owned()),
            }
        }
        // Preset dialog results (path + save flag + target) execute here.
        if let Some((path, save, track_id, plugin_index)) = self
            .pending_preset
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let accepted = if save {
                core.save_plugin_preset(track_id, plugin_index, &path.to_string_lossy())
            } else {
                core.load_plugin_preset(track_id, plugin_index, &path.to_string_lossy())
            };
            self.status.set(if accepted {
                format!(
                    "PRESET {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            } else {
                "PRESET REJECTED".to_owned()
            });
        }
        // File-dialog results arrive from the async native dialog on a
        // background thread; the engine work happens here on the UI thread.
        if let Some(path) = self
            .pending_open
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let path = path.to_string_lossy().into_owned();
            if core.load_project(&path) {
                *self.project_path.borrow_mut() = Some(path.clone());
                self.selected_clip.set(None);
                self.show_genesis.set(false);
                self.status.set(format!(
                    "OPENED {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or(path)
                ));
            } else {
                self.status.set("LOAD FAILED".to_owned());
            }
        }
        if let Some(path) = self
            .pending_save
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let path = path.to_string_lossy().into_owned();
            if core.save_project(&path) {
                *self.project_path.borrow_mut() = Some(path.clone());
                self.status.set(format!(
                    "SAVED {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or(path)
                ));
            } else {
                self.status.set("SAVE FAILED".to_owned());
            }
        }
        // OpenUtau import: source is the UST/USTX path, render is the
        // rendered stem; both are required by the engine's import path.
        if let Some((source, render)) = self
            .pending_openutau
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let diagnostic = core.openutau_import_diagnostic_json(
                &source.to_string_lossy(),
                &render.to_string_lossy(),
            );
            let ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                == Some(true);
            if !ok {
                self.openutau_caption
                    .set(format!("IMPORT REJECTED: {diagnostic}"));
            } else {
                let track_id = core.add_track(4);
                if track_id == 0 || !core.add_region(track_id, &render.to_string_lossy(), 0.0) {
                    self.openutau_caption
                        .set("OPENUTAU IMPORT FAILED: COULD NOT CREATE VOCAL REGION".to_owned());
                } else {
                    self.tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                    self.openutau_caption.set(format!(
                        "VOCAL IMPORTED: {} · SOURCE KEPT: {}",
                        render
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("render"),
                        source
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("score"),
                    ));
                }
            }
        }
        // MusicXML export: engine takes care of layout + instrument names.
        if let Some(path) = self
            .pending_musicxml
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.export_musicxml(&path.to_string_lossy()) {
                Ok(count) => self
                    .status
                    .set(format!("MUSICXML EXPORTED · {count} NOTES")),
                Err(error) => self.status.set(format!("MUSICXML EXPORT FAILED: {error}")),
            }
        }
        // MIDI export dialog result: write the .mid through the engine.
        if let Some(path) = self
            .pending_midi
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.export_midi_file(&path.to_string_lossy()) {
                Ok(count) => self.status.set(format!("MIDI EXPORTED · {count} NOTES")),
                Err(_) => self.status.set("MIDI EXPORT FAILED".to_owned()),
            }
        }
        // Stem export folder selected: arm the batch; the poll advances it
        // once the engine reports bounce state 3 (see poll_render).
        if let Some(dir) = self
            .pending_stems
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let track_ids: Vec<u32> = self.tracks.get().iter().map(|t| t.id).collect();
            if track_ids.is_empty() {
                self.render_caption
                    .set("STEM EXPORT NEEDS AT LEAST ONE TRACK".to_owned());
            } else {
                let prepared = core.stem_batch_prepare(
                    dir.to_string_lossy().as_ref(),
                    self.export_format.get(),
                    &track_ids,
                    render_tail_seconds(self.export_tail_index.get() as i32),
                    false,
                    true,
                );
                match prepare_rejected(&prepared) {
                    Some(reason) => self.render_caption.set(reason),
                    None => match stem_step(&core.stem_batch_next()) {
                        StemStep::Ready { output, format, .. } => {
                            if kick_stem(&core, &output, format) {
                                self.stem_export_active.set(true);
                                self.render_caption.set("STEM EXPORT STARTED".to_owned());
                            } else {
                                core.stem_batch_abort();
                                self.render_caption
                                    .set("could not queue the first stem".to_owned());
                            }
                        }
                        StemStep::Failed(reason) => self.render_caption.set(reason),
                        _ => self
                            .render_caption
                            .set("stem export queue is invalid".to_owned()),
                    },
                }
            }
        }
    }

    /// Consumes finished waveform decodes and queues missing clip peaks,
    /// mirroring the telemetry queue: at most 8 detached decodes in
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
        // surfaces, so this shell and the arranger reduce the
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
        self.dirty_layout
            .set(self.dirty_layout.get().wrapping_add(1));
    }

    /// Pointer travel since the lane press, in logical pixels. Clicks
    /// within this distance select; beyond it the press becomes a seek drag.
    fn click_distance(&self, event: &CanvasPointerEvent) -> f64 {
        (event.x - self.down_x).hypot(event.y - self.down_y)
    }

    /// Clamps a trim beat delta through the dock fraction math, so
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
    /// `on_trim_clip` path: one undo transaction, abort on failure,
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
    /// mirroring the `sync_midi_notes_to_core` path: every note
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
    /// `on_clip_moved` path: one undo transaction, sync-group aware,
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

    /// Draws the arrangement (ruler, lanes, clips, markers, playhead) as
    /// vector shapes. This is the read-only parity step: geometry matches
    /// the dock, but there is no zoom/scroll, selection or drag yet,
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
    /// Commits the automation gesture: one undo transaction around
    /// `set_automation_data` with beat-sorted native triples, mirroring
    /// the automation commit (it commits live during the drag
    /// inside the gesture's transaction; we commit once on release).
    fn commit_auto_edit(&mut self) {
        let Some(edit) = self.auto_edit.take() else {
            return;
        };
        let Some(core) = self.core.clone() else {
            return;
        };
        let tracks = self.tracks.get();
        let Some(track) = tracks.iter().find(|track| track.id == edit.track_id) else {
            return;
        };
        let mut points: Vec<crate::ui_shared::arrange_model::AutomationPoint> = match edit.lane {
            0 => track.volume_automation.clone(),
            _ => track.pan_automation.clone(),
        };
        match edit.point_index {
            Some(index) => {
                let Some(point) = points.get_mut(index) else {
                    return;
                };
                point.beat = (edit.orig_beat + edit.delta_beats).max(0.0);
                point.value = (point.value + edit.delta_value).clamp(
                    if edit.lane == 0 { 0.0 } else { -1.0 },
                    if edit.lane == 0 { 2.0 } else { 1.0 },
                );
            }
            None => {
                points.push(crate::ui_shared::arrange_model::AutomationPoint {
                    beat: edit.orig_beat.max(0.0),
                    value: (edit.orig_value + edit.delta_value).clamp(
                        if edit.lane == 0 { 0.0 } else { -1.0 },
                        if edit.lane == 0 { 2.0 } else { 1.0 },
                    ),
                });
            }
        }
        points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let native: Vec<f64> = points
            .iter()
            .flat_map(|point| {
                let time = core.beats_to_samples(f64::from(point.beat.max(0.0))) as f64;
                [time, f64::from(point.value), 0.0]
            })
            .collect();
        core.begin_undo_transaction("Edit Automation");
        let accepted = core.set_automation_data(edit.track_id, edit.lane, native);
        if accepted {
            let _ = core.end_undo_transaction();
            self.refresh_arrange_state(&core);
        } else {
            let _ = core.abort_undo_transaction();
        }
        self.auto_point.set(None);
    }

    /// Piano-roll gestures: press a note to move it (right edge resizes),
    /// press empty space to add one, drag to preview, release to commit.
    /// The arrange edit lock holds edits here too (selection only).
    fn on_piano_pointer(&mut self, event: CanvasPointerEvent) -> bool {
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
    /// A dropped audio file becomes a clip on the lane under the drop
    /// point (falling back to the piano track and the playhead), via the
    /// engine's queued import, like the audio import.
    fn on_file_dropped(&mut self, path: &str, x: Option<f64>, y: Option<f64>) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let (bounds_x, bounds_y, _, _) = self.arrange_bounds;
        let geometry = self.arrange_geometry;
        let tracks = self.tracks.get();
        let (track_id, beat) = match (x, y) {
            (Some(drop_x), Some(drop_y)) => {
                let local_x = drop_x as f32 - bounds_x;
                let local_y = drop_y as f32 - bounds_y;
                if geometry.lane_height <= 0.0 || local_y < geometry.ruler_height {
                    return;
                }
                let lane = ((local_y - geometry.ruler_height) / geometry.lane_height) as usize;
                let Some(track) = tracks.get(lane) else {
                    return;
                };
                (track.id, f64::from(geometry.beat_for_x(local_x)))
            }
            _ => {
                let track_id = self.piano_track.get();
                (track_id, self.playhead_beats.get())
            }
        };
        if track_id == 0 {
            return;
        }
        let ticket = core.queue_region_import_at_beat(track_id, path, beat);
        if ticket == 0 {
            self.status.set("IMPORT FAILED".to_owned());
            return;
        }
        self.status.set(format!(
            "IMPORTING: {}",
            std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_owned())
        ));
        self.pending_import = Some((ticket, path.to_owned(), track_id));
    }
    /// Zooms the arrange canvas by `factor`, anchored at canvas x so the
    /// beat under the pointer stays put (wheel and pinch share this).
    fn zoom_arrange_at(&self, x: f32, factor: f32) {
        let geometry = self.arrange_geometry;
        if geometry.px_per_beat <= 0.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let zoom_old = self.arrange_zoom.get();
        let zoom_new = (zoom_old * factor).clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX);
        let timeline_end = self.timeline_end_beats.get();
        let timeline_width = geometry.px_per_beat * timeline_end / zoom_old;
        let px_new = timeline_width / timeline_end * zoom_new;
        let beat = geometry.beat_for_x(x);
        let scroll_new = beat - (x - geometry.header_width) / px_new;
        self.arrange_zoom.set(zoom_new);
        self.arrange_scroll_beats.set(scroll_new.max(0.0));
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
        match event {
            AmeEvent::AppMessage(message) => {
                if message == "tick" {
                    self.poll_engine();
                }
            }
            AmeEvent::FileDropped { path, x, y } => {
                self.on_file_dropped(path, *x, *y);
            }
            _ => {}
        }
    }

    /// App-wide shortcuts (parity: Space plays, L loops). The
    /// framework only calls this with no text editor focused, so the
    /// lyric field keeps Space/L as text input.
    fn on_shortcut(&mut self, key: &str, modifiers: KeyModifiers) -> bool {
        if (modifiers.control || modifiers.super_key) && matches!(key, "k" | "K") {
            self.show_spotlight.set(!self.show_spotlight.get());
            return true;
        }
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
            // Zoom anchored at the pointer beat, same factor curve as the
            // timeline ruler's wheel handling.
            let factor = (1.0 - event.delta_y / 400.0).clamp(0.25, 4.0) as f32;
            self.zoom_arrange_at(event.x as f32, factor);
        } else {
            // Plain wheel scrolls the timeline (DAW convention), matching
            // the timeline ruler's scroll direction and units.
            let timeline_end = self.timeline_end_beats.get();
            let slot_width = (self.arrange_bounds.2 - 104.0).max(0.0);
            let visible = if geometry.px_per_beat > 0.0 {
                f64::from(slot_width / geometry.px_per_beat)
            } else {
                f64::from(timeline_end)
            };
            let max_offset = (f64::from(timeline_end) - visible).max(0.0);
            let delta = if event.delta_x != 0.0 {
                event.delta_x
            } else {
                event.delta_y
            };
            let offset = f64::from(self.arrange_scroll_beats.get())
                - delta / f64::from(geometry.px_per_beat);
            self.arrange_scroll_beats
                .set(offset.clamp(0.0, max_offset) as f32);
        }
        true
    }

    fn on_canvas_gesture(&mut self, canvas_id: &str, event: CanvasGestureEvent) -> bool {
        if canvas_id != "arrange" {
            return false;
        }
        match event.gesture {
            CanvasGestureKind::Pinch { delta, .. } => {
                self.zoom_arrange_at(event.x as f32, (1.0 + delta) as f32);
                true
            }
            CanvasGestureKind::Pan { delta_x, .. } => {
                let px_per_beat = self.arrange_geometry.px_per_beat;
                if px_per_beat > 0.0 {
                    let delta_beats = (delta_x / f64::from(px_per_beat)) as f32;
                    self.arrange_scroll_beats
                        .set((self.arrange_scroll_beats.get() - delta_beats).max(0.0));
                }
                true
            }
            CanvasGestureKind::Rotate { .. } => false,
        }
    }

    /// Redraws canvases only when their inputs changed, using the landed
    /// dirty-canvas API: the waveform always animates (meters), while
    /// arrange/piano replay their cached scenes when idle. Geometry and
    /// scale changes re-record regardless (framework behavior).
    /// Position-aware cursor: resize over Audio/Vocal trim edges, pointer
    /// over clips/notes, default elsewhere. Uses the landed per-region
    /// cursor API (previously a static per-slot icon).
    fn canvas_cursor(&self, canvas_id: &str, x: f64, y: f64) -> Option<ame_ui::CursorIcon> {
        match canvas_id {
            "arrange" => {
                let geometry = self.arrange_geometry;
                if geometry.lane_height <= 0.0 || (y as f32) < geometry.ruler_height {
                    return None;
                }
                let lane = ((y as f32 - geometry.ruler_height) / geometry.lane_height) as usize;
                let tracks = self.tracks.get();
                let track = tracks.get(lane)?.clone();
                let hit = clip_at(&tracks, &geometry, x as f32, y as f32);
                let Some((_, clip_id)) = hit else {
                    return None;
                };
                let clip = track.clips.iter().find(|clip| clip.id == clip_id)?;
                if clip.midi {
                    return Some(ame_ui::CursorIcon::Pointer);
                }
                if (track.kind == "Audio" || track.kind == "Vocal")
                    && trim_edge_at(&geometry, clip, x as f32).is_some()
                {
                    return Some(ame_ui::CursorIcon::EwResize);
                }
                Some(ame_ui::CursorIcon::Pointer)
            }
            "piano" => {
                let geometry = self.piano_geometry;
                if geometry.px_per_pitch <= 0.0 {
                    return None;
                }
                let track_id = self.piano_track.get();
                let notes = self.piano_notes.get();
                let visible: Vec<PianoNote> = notes
                    .iter()
                    .filter(|note| note.track_id == track_id)
                    .cloned()
                    .collect();
                if note_at(&visible, &geometry, x as f32, y as f32).is_some() {
                    Some(ame_ui::CursorIcon::Pointer)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Declares the arrange/piano elements as screen-reader regions,
    /// using the landed canvas accessibility API (bounds are slot-local,
    /// matching pointer coordinates).
    fn canvas_accessibility_items(&self, canvas_id: &str) -> Vec<CanvasAccessibilityItem> {
        const MAX_ITEMS: usize = 512;
        match canvas_id {
            "arrange" => {
                let geometry = self.arrange_geometry;
                let tracks = self.tracks.get();
                let selected = self.selected_clip.get();
                let mut items = Vec::new();
                for (index, track) in tracks.iter().enumerate() {
                    let top = geometry.y_for_lane(index);
                    items.push(CanvasAccessibilityItem {
                        label: track.name.clone(),
                        role: CanvasItemRole::Label,
                        x: 0.0,
                        y: top,
                        width: geometry.header_width,
                        height: geometry.lane_height,
                        value: Some(format!("{} · {} clips", track.kind, track.clips.len())),
                        selected: None,
                    });
                    for clip in &track.clips {
                        if items.len() >= MAX_ITEMS {
                            return items;
                        }
                        let start = geometry.x_for_beat(clip.start_beat);
                        let width = (clip.length_beats * geometry.px_per_beat).max(4.0) - 2.0;
                        let height = (geometry.lane_height - 4.0).clamp(2.0, 42.0);
                        let y = top + (geometry.lane_height - height) / 2.0;
                        items.push(CanvasAccessibilityItem {
                            label: clip.name.clone(),
                            role: CanvasItemRole::Button,
                            x: start,
                            y,
                            width,
                            height,
                            value: None,
                            selected: Some(selected == Some((track.id, clip.id))),
                        });
                    }
                }
                items
            }
            "piano" => {
                let geometry = self.piano_geometry;
                let track_id = self.piano_track.get();
                let selected = self.selected_note.get();
                const PITCH_NAMES: [&str; 12] = [
                    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
                ];
                self.piano_notes
                    .get()
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| note.track_id == track_id)
                    .take(MAX_ITEMS)
                    .map(|(index, note)| CanvasAccessibilityItem {
                        label: format!(
                            "Note {}{} velocity {}",
                            PITCH_NAMES[(note.pitch % 12) as usize],
                            note.pitch / 12 - 1,
                            note.velocity
                        ),
                        role: CanvasItemRole::Button,
                        x: geometry.x_for_beat(note.start_beat),
                        y: geometry.y_for_pitch(note.pitch),
                        width: (note.length_beats * geometry.px_per_beat).max(2.0),
                        height: geometry.px_per_pitch.max(1.0),
                        value: None,
                        selected: Some(selected == Some(index)),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn canvas_scene_dirty(&self, canvas_id: &str) -> bool {
        match canvas_id {
            "waveform" => true,
            "arrange" | "piano" => {
                let playhead = self.playhead_beats.get();
                let zoom = self.arrange_zoom.get();
                let scroll = self.arrange_scroll_beats.get();
                let selected = self.selected_clip.get();
                let piano = (self.piano_track.get(), self.dirty_piano.get().1);
                let dirty = playhead != self.dirty_playhead.get()
                    || zoom != self.dirty_zoom.get()
                    || scroll != self.dirty_scroll.get()
                    || selected != self.dirty_selected.get()
                    || piano != self.dirty_piano.get()
                    || self.dirty_layout.get() != self.dirty_layout_seen.get();
                self.dirty_playhead.set(playhead);
                self.dirty_zoom.set(zoom);
                self.dirty_scroll.set(scroll);
                self.dirty_selected.set(selected);
                self.dirty_piano.set(piano);
                self.dirty_layout_seen.set(self.dirty_layout.get());
                dirty
            }
            _ => true,
        }
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
        // while it is open, same as the project actions.
        let open_path = self.project_path.clone();
        let open_pending = self.pending_open.clone();
        let open_button = ButtonView::new("Open")
            .action_id("project.open")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Hirari project", &["hirari"]);
                let dialog = match open_path.borrow().as_deref() {
                    Some(path) => dialog.set_directory(
                        std::path::Path::new(path)
                            .parent()
                            .map(|parent| parent.to_path_buf())
                            .unwrap_or_default(),
                    ),
                    None => dialog,
                };
                let pending = open_pending.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let save_core = self.core.clone();
        let save_path = self.project_path.clone();
        let save_pending = self.pending_save.clone();
        let save_status = self.status.clone();
        let save_button = ButtonView::new("Save")
            .action_id("project.save")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &save_core else { return };
                match save_path.borrow().clone() {
                    Some(path) => {
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
                    }
                    None => {
                        let dialog = ame_ui::platform::NativeFileDialog::new()
                            .set_file_name("Untitled.hirari")
                            .add_filter("Hirari project", &["hirari"]);
                        let pending = save_pending.clone();
                        dialog.save_file_async(move |path| {
                            if let Some(path) = path {
                                if let Ok(mut slot) = pending.lock() {
                                    *slot = Some(path);
                                }
                            }
                        });
                    }
                }
            });
        let new_core = self.core.clone();
        let new_path = self.project_path.clone();
        let new_selected = self.selected_clip.clone();
        let new_status = self.status.clone();
        let new_show_genesis = self.show_genesis.clone();
        let new_button = ButtonView::new("New")
            .action_id("project.new")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &new_core {
                    core.new_project();
                    *new_path.borrow_mut() = None;
                    new_selected.set(None);
                    new_show_genesis.set(true);
                    new_status.set("NEW PROJECT".to_owned());
                }
            });
        let diagnostics_open_for_button = self.show_diagnostics.clone();
        let spotlight_state = self.show_spotlight.clone();
        let spotlight_button = ButtonView::new("CMD+K")
            .action_id("spotlight.toggle")
            .secondary()
            .on_press(move |_| {
                spotlight_state.set(!spotlight_state.get());
            });
        let focus_state = self.focus_mode.clone();
        let focus_button = ButtonView::new(if self.focus_mode.get() {
            "FOCUS ON"
        } else {
            "FOCUS"
        })
        .action_id("focus.toggle")
        .secondary()
        .on_press(move |_| {
            let next = !focus_state.get();
            focus_state.set(next);
            let _ = crate::ui_shared::preferences::store_focus_mode(next);
        });
        let diagnostics_button = ButtonView::new(if self.show_diagnostics.get() {
            "SYSTEMS CLOSE"
        } else {
            "SYSTEM"
        })
        .action_id("diagnostics.toggle")
        .secondary()
        .on_press(move |_| {
            diagnostics_open_for_button.set(!diagnostics_open_for_button.get());
        });
        let sends_open_for_button = self.show_sends.clone();
        let sends_button = ButtonView::new(if self.show_sends.get() {
            "SENDS CLOSE"
        } else {
            "SENDS"
        })
        .action_id("sends.toggle")
        .secondary()
        .on_press(move |_| {
            sends_open_for_button.set(!sends_open_for_button.get());
        });
        let moufu_open_for_button = self.show_moufu.clone();
        let moufu_button = ButtonView::new(if self.show_moufu.get() {
            "MOUFU CLOSE"
        } else {
            "MOUFU"
        })
        .action_id("moufu.toggle")
        .secondary()
        .on_press(move |_| {
            moufu_open_for_button.set(!moufu_open_for_button.get());
        });
        let project_bar = View::Card(vec![View::HStack(vec![
            View::Text("PROJECT".to_owned()),
            open_button.into(),
            save_button.into(),
            new_button.into(),
            diagnostics_button.into(),
            moufu_button.into(),
            sends_button.into(),
            focus_button.into(),
            spotlight_button.into(),
            View::Caption(status.clone()),
        ])]);

        // Export card (export dialog parity): format and tail
        // cyclers, mix render, stem batch, MIDI export, cancel. Stems
        // are armed by the folder-picker poll; the render poll advances
        // the batch while the engine reports bounce state 3.
        let export_format_state = self.export_format.clone();
        let format_label = render_format_label(self.export_format.get());
        let export_format_button = ButtonView::new(format!("FORMAT: {format_label}"))
            .action_id("export.format_cycle")
            .secondary()
            .on_press(move |_| {
                const FORMATS: [u32; 4] = [0, 3, 2, 1]; // PCM16, PCM24, Float32, Wave64
                let current = export_format_state.get();
                let index = FORMATS
                    .iter()
                    .position(|code| *code == current)
                    .unwrap_or(0);
                export_format_state.set(FORMATS[(index + 1) % FORMATS.len()]);
            });
        let export_tail_state = self.export_tail_index.clone();
        let tail_label = match self.export_tail_index.get() {
            0 => "NONE",
            1 => "1s",
            2 => "2s",
            3 => "5s",
            4 => "10s",
            _ => "30s",
        };
        let export_tail_button = ButtonView::new(format!("TAIL: {tail_label}"))
            .action_id("export.tail_cycle")
            .secondary()
            .on_press(move |_| {
                export_tail_state.set((export_tail_state.get() + 1) % 6);
            });
        let render_core = self.core.clone();
        let render_format = self.export_format.clone();
        let render_tail = self.export_tail_index.clone();
        let render_status = self.render_caption.clone();
        let render_button = ButtonView::new("RENDER")
            .action_id("export.render")
            .primary()
            .on_press(move |_| {
                let Some(core) = &render_core else { return };
                let path = default_render_output_path();
                core.set_offline_render_tail_seconds(render_tail_seconds(render_tail.get() as i32));
                if core.start_render_async_to_format(&path.to_string_lossy(), render_format.get()) {
                    render_status.set(format!(
                        "RENDER QUEUED: {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ));
                } else {
                    render_status.set("RENDER COULD NOT BE QUEUED".to_owned());
                }
            });
        let stems_pending = self.pending_stems.clone();
        let stems_button = ButtonView::new("STEMS")
            .action_id("export.stems")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new();
                let pending = stems_pending.clone();
                dialog.pick_folder_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let musicxml_pending = self.pending_musicxml.clone();
        let musicxml_button = ButtonView::new("MUSICXML")
            .action_id("export.musicxml")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .set_file_name("project.musicxml")
                    .add_filter("MusicXML", &["musicxml", "xml"]);
                let pending = musicxml_pending.clone();
                dialog.save_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let midi_pending = self.pending_midi.clone();
        let midi_button = ButtonView::new("MIDI")
            .action_id("export.midi")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .set_file_name("project.mid")
                    .add_filter("MIDI file", &["mid", "midi"]);
                let pending = midi_pending.clone();
                dialog.save_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let cancel_core = self.core.clone();
        let cancel_status = self.render_caption.clone();
        let cancel_button = ButtonView::new("CANCEL")
            .action_id("export.cancel")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &cancel_core {
                    let (_, message, _) = cancellation_status(core.cancel_render());
                    cancel_status.set(message.to_owned());
                }
            });
        let export_card = View::Card(vec![View::HStack(vec![
            View::Text("EXPORT".to_owned()),
            export_format_button.into(),
            export_tail_button.into(),
            render_button.into(),
            stems_button.into(),
            midi_button.into(),
            musicxml_button.into(),
            cancel_button.into(),
            View::Caption(self.render_caption.get()),
        ])]);

        // Diagnostics card (diagnostics page parity): engine, audio
        // drops/overruns, plugin sandbox health and range overflow.
        // Aboutattribution is intentionally not ported.
        let diagnostics_card = if let Some(core) = &self.core {
            let health = core.runtime_health_snapshot();
            let state = core.runtime_health_status_text();
            let overruns: u32 = core
                .sandbox_snapshots()
                .iter()
                .map(|snapshot| snapshot.mailbox_overruns)
                .sum();
            let drops = core.get_dropped_input_blocks();
            let failures = health.sandbox_failures;
            let watchdog = self.watchdog_trips.get();
            let recover_core = self.core.clone();
            let recover_status = self.status.clone();
            let recover_button = ButtonView::new("RECOVER")
                .action_id("diagnostics.recover")
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &recover_core {
                        let recovered = core.recover_sandboxed_plugins();
                        recover_status.set(format!(
                            "RECOVERY REQUESTED · {recovered} PLUGIN(S) RECOVERED"
                        ));
                    }
                });
            let cpu = health.dsp_load.clamp(0.0, 4.0) * 25.0;
            let range_label = if health.audio_range_overflow {
                "RANGE OVERFLOW"
            } else {
                "RANGE OK"
            };
            let mut rows = vec![
                View::Text(format!("SYSTEM · {state}")),
                View::HStack(vec![
                    View::Caption(format!("CPU {cpu:.0}%")),
                    View::Caption(format!("DROP {drops}")),
                    View::Caption(format!("OVERRUN {overruns}")),
                    View::Caption(format!("FAIL {failures}")),
                    View::Caption(format!("WATCHDOG {watchdog}")),
                    View::Caption(range_label.to_owned()),
                    recover_button.into(),
                ]),
            ];
            if health.audio_silent_fallback
                || matches!(health.audio_driver_status, "start-failed" | "unavailable")
            {
                rows.push(View::Caption(
                    "No audio path — check device, sample rate, buffer, cable.".to_owned(),
                ));
            }
            View::Card(rows)
        } else {
            View::Card(vec![View::Text("SYSTEM · no engine".to_owned())])
        };

        // Moufu card (moufu_link_page parity): Hub address,
        // connect/disconnect, entity share/receive, links list and
        // read-only linked-project preview.
        let moufu_card = {
            let address_state = self.moufu_address.clone();
            let address_change = address_state.clone();
            let address_field = TextFieldView::new(
                self.moufu_address.get(),
                "Moufu Hub address (e.g. 127.0.0.1:9478)",
            )
            .change_action_id("moufu.address_change")
            .submit_action_id("moufu.address_change")
            .on_change(move |value| address_change.set(value))
            .on_submit(|| {});
            let connect_state = self.moufu_address.clone();
            let connect_publisher = self.moufu_publisher.clone();
            let connect_status = self.moufu_status.clone();
            let connect_button = ButtonView::new("CONNECT")
                .action_id("moufu.connect")
                .primary()
                .on_press(move |_| {
                    let address = connect_state.get().trim().to_owned();
                    if address.len() > 1024 {
                        connect_status.set("MOUFU ADDRESS IS TOO LONG".to_owned());
                        return;
                    }
                    match &connect_publisher {
                        Some(publisher) => {
                            let target = (!address.is_empty()).then_some(address.as_str());
                            match publisher.set_address(target) {
                                Ok(()) => {
                                    let _ = save_moufu_address(&address);
                                    connect_status.set(if address.is_empty() {
                                        "MOUFU NOT CONFIGURED".to_owned()
                                    } else {
                                        "MOUFU CONNECTING".to_owned()
                                    });
                                }
                                Err(error) => {
                                    connect_status
                                        .set(format!("MOUFU CONFIGURATION FAILED: {error}"));
                                }
                            }
                        }
                        None => connect_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let disconnect_state = self.moufu_address.clone();
            let disconnect_publisher = self.moufu_publisher.clone();
            let disconnect_status = self.moufu_status.clone();
            let disconnect_button =
                ButtonView::new("DISCONNECT")
                    .action_id("moufu.disconnect")
                    .secondary()
                    .on_press(move |_| {
                        disconnect_state.set(String::new());
                        let _ = save_moufu_address("");
                        match &disconnect_publisher {
                            Some(publisher) => match publisher.set_address(None) {
                                Ok(()) => disconnect_status.set("MOUFU NOT CONFIGURED".to_owned()),
                                Err(error) => disconnect_status
                                    .set(format!("MOUFU DISCONNECT FAILED: {error}")),
                            },
                            None => disconnect_status
                                .set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                        }
                    });
            let refresh_publisher = self.moufu_publisher.clone();
            let refresh_status = self.moufu_status.clone();
            let refresh_button = ButtonView::new("REFRESH")
                .action_id("moufu.refresh")
                .secondary()
                .on_press(move |_| match &refresh_publisher {
                    Some(publisher) => {
                        let entities = publisher.list_entities();
                        let links = publisher.list_links();
                        if let Err(error) = entities.and(links) {
                            refresh_status.set(format!("MOUFU REFRESH FAILED: {error}"));
                        }
                    }
                    None => refresh_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                });
            let entity_labels = self.moufu_entity_labels.get();
            let entity_count = entity_labels.len() as u32;
            let entity_index = self
                .moufu_entity_selected
                .get()
                .min(entity_count.saturating_sub(1));
            let entity_name = entity_labels
                .get(entity_index as usize)
                .cloned()
                .unwrap_or_else(|| "no entities".to_owned());
            let cycle_label = format!("ENT {entity_index}/{}", entity_count.max(1));
            let entity_selected_state = self.moufu_entity_selected.clone();
            let entity_cycle_button = ButtonView::new(cycle_label)
                .action_id("moufu.entity_cycle")
                .secondary()
                .on_press(move |_| {
                    if entity_count > 0 {
                        entity_selected_state.set((entity_selected_state.get() + 1) % entity_count);
                    }
                });
            let share_publisher = self.moufu_publisher.clone();
            let share_ids = self.moufu_entity_ids.clone();
            let share_selected = self.moufu_entity_selected.clone();
            let share_status = self.moufu_status.clone();
            let share_button = ButtonView::new("SHARE")
                .action_id("moufu.share")
                .secondary()
                .on_press(move |_| {
                    let ids = share_ids.get();
                    let target = ids.get(share_selected.get() as usize).cloned();
                    let Some(target) = target else { return };
                    match &share_publisher {
                        Some(publisher) => {
                            if let Err(error) =
                                publisher.create_link(HIRARI_PROJECT_ENTITY, &target)
                            {
                                share_status.set(format!("MOUFU SHARE FAILED: {error}"));
                            }
                        }
                        None => share_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let receive_publisher = self.moufu_publisher.clone();
            let receive_ids = self.moufu_entity_ids.clone();
            let receive_selected = self.moufu_entity_selected.clone();
            let receive_status = self.moufu_status.clone();
            let receive_button = ButtonView::new("RECEIVE")
                .action_id("moufu.receive")
                .secondary()
                .on_press(move |_| {
                    let ids = receive_ids.get();
                    let source = ids.get(receive_selected.get() as usize).cloned();
                    let Some(source) = source else { return };
                    match &receive_publisher {
                        Some(publisher) => {
                            if let Err(error) =
                                publisher.create_link(&source, HIRARI_PROJECT_ENTITY)
                            {
                                receive_status.set(format!("MOUFU RECEIVE FAILED: {error}"));
                            }
                        }
                        None => receive_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let link_labels = self.moufu_link_labels.get();
            let link_count = link_labels.len() as u32;
            let link_index = self
                .moufu_link_selected
                .get()
                .min(link_count.saturating_sub(1));
            let link_name = link_labels
                .get(link_index as usize)
                .cloned()
                .unwrap_or_else(|| "no links".to_owned());
            let link_cycle = format!("LINK {link_index}/{}", link_count.max(1));
            let link_selected_state = self.moufu_link_selected.clone();
            let link_cycle_button = ButtonView::new(link_cycle)
                .action_id("moufu.link_cycle")
                .secondary()
                .on_press(move |_| {
                    if link_count > 0 {
                        link_selected_state.set((link_selected_state.get() + 1) % link_count);
                    }
                });
            let unlink_publisher = self.moufu_publisher.clone();
            let unlink_ids = self.moufu_link_ids.clone();
            let unlink_selected = self.moufu_link_selected.clone();
            let unlink_status = self.moufu_status.clone();
            let unlink_button = ButtonView::new("REMOVE LINK")
                .action_id("moufu.unlink")
                .secondary()
                .on_press(move |_| {
                    let ids = unlink_ids.get();
                    let link_id = ids.get(unlink_selected.get() as usize).cloned();
                    let Some(link_id) = link_id else { return };
                    match &unlink_publisher {
                        Some(publisher) => {
                            if let Err(error) = publisher.destroy_link(&link_id) {
                                unlink_status.set(format!("MOUFU REMOVE LINK FAILED: {error}"));
                            }
                        }
                        None => unlink_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            View::Card(vec![
                View::Text("MOUFU HUB".to_owned()),
                View::HStack(vec![
                    address_field.into(),
                    connect_button.into(),
                    disconnect_button.into(),
                    refresh_button.into(),
                ]),
                View::Caption(self.moufu_status.get()),
                View::HStack(vec![
                    View::Caption(format!("ENT: {entity_name}")),
                    entity_cycle_button.into(),
                    share_button.into(),
                    receive_button.into(),
                ]),
                View::HStack(vec![
                    View::Caption(format!("LINK: {link_name}")),
                    link_cycle_button.into(),
                    unlink_button.into(),
                ]),
                View::Caption(if self.moufu_remote_source.get().is_empty() {
                    self.moufu_preview.get()
                } else {
                    format!("{}
{}", self.moufu_remote_source.get(), self.moufu_preview.get())
                }),
                View::Caption(
                    "Project names/notes/markers sync both ways; file paths and plugin state stay local. Connect only to a trusted Hub."
                        .to_owned(),
                ),
            ])
        };

        // Video card (media/video_monitor parity): toggle and load.
        // Frame display fans out through the video canvas; revision-tracked.
        let video_open = self.show_video.clone();
        let video_toggle = ButtonView::new(if self.show_video.get() {
            "HIDE VIDEO"
        } else {
            "SHOW VIDEO"
        })
        .action_id("video.toggle")
        .secondary()
        .on_press(move |_| {
            video_open.set(!video_open.get());
        });
        let video_pending = self.pending_video_load.clone();
        let video_load = ButtonView::new("LOAD VIDEO")
            .action_id("video.load")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Video file", &["mov", "mp4", "mkv", "webm", "avi"]);
                let pending = video_pending.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        // TEMPO card (global tempo editor parity): base tempo ±,
        // tap tempo, time-signature cycle (4/4 3/4 6/8 7/8), tempo-map
        // event cycler with add/remove and ramp/step mode.
        let tempo_current = self.tempo.clone();
        let tempo_core = self.core.clone();
        let tempo_down = ButtonView::new("BPM-")
            .action_id("tempo.decrement")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &tempo_core {
                    let current = tempo_current.get();
                    let _ = core.set_tempo((current - 1.0).clamp(20.0, 300.0));
                }
            });
        let tempo_current = self.tempo.clone();
        let tempo_core = self.core.clone();
        let tempo_up = ButtonView::new("BPM+")
            .action_id("tempo.increment")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &tempo_core {
                    let current = tempo_current.get();
                    let _ = core.set_tempo((current + 1.0).clamp(20.0, 300.0));
                }
            });
        let tap_core = self.core.clone();
        let tap_taps = self.tempo_taps.clone();
        let tap_status = self.status.clone();
        let tempo_tap = ButtonView::new("TAP TEMPO")
            .action_id("tempo.tap")
            .secondary()
            .on_press(move |_| {
                let now = std::time::Instant::now();
                let mut taps = tap_taps.borrow_mut();
                if taps.back().is_some_and(|previous| {
                    let elapsed = now.duration_since(*previous).as_secs_f64();
                    !(0.2..=3.0).contains(&elapsed)
                }) {
                    taps.clear();
                }
                taps.push_back(now);
                while taps.len() > 5 {
                    taps.pop_front();
                }
                if taps.len() >= 2 {
                    let intervals: Vec<f64> = taps
                        .make_contiguous()
                        .windows(2)
                        .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64())
                        .collect();
                    let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
                    let bpm = (60.0 / average).clamp(20.0, 300.0) as f32;
                    if let Some(core) = &tap_core {
                        let _ = core.set_tempo(bpm);
                        tap_status.set(format!("TAP TEMPO: {bpm:.2} BPM"));
                    }
                } else {
                    tap_status.set("TAP TEMPO: TAP AGAIN".to_owned());
                }
            });
        let signature_state = self.tempo_time_signature.clone();
        let signature_core = self.core.clone();
        let signature_playhead = self.playhead_beats.clone();
        let signature_button = ButtonView::new(format!(
            "TS {}",
            ["4/4", "3/4", "6/8", "7/8"][self.tempo_time_signature.get() as usize]
        ))
        .action_id("tempo.signature_cycle")
        .secondary()
        .on_press(move |_| {
            const TABLE: [(u8, u8); 4] = [(4, 4), (3, 4), (6, 8), (7, 8)];
            let next = (signature_state.get() + 1) % 4;
            signature_state.set(next);
            let Some(core) = &signature_core else { return };
            let (numerator, denominator) = TABLE[next as usize];
            let beat = signature_playhead.get().max(0.0);
            let _ = core.set_time_signature_event(beat, numerator, denominator);
        });
        let tempo_events: Vec<(f64, f64, bool)> = self
            .core
            .as_ref()
            .map(|core| {
                core.get_tempo_events()
                    .chunks_exact(3)
                    .map(|chunk| (chunk[0], chunk[1], chunk[2] != 0.0))
                    .collect()
            })
            .unwrap_or_default();
        let event_total = tempo_events.len();
        let event_selected = self.tempo_event_selected.get() as usize;
        let event_summary = tempo_events
            .get(event_selected)
            .map(|(beat, bpm, ramp)| {
                format!(
                    "EVT {}/{} @ {:.2}B {:.1}{}",
                    event_selected + 1,
                    event_total,
                    beat,
                    bpm,
                    if *ramp { " RAMP" } else { " STEP" }
                )
            })
            .unwrap_or_else(|| "NO EVENTS".to_owned());
        let event_selected_state = self.tempo_event_selected.clone();
        let event_prev = ButtonView::new("EV-")
            .action_id("tempo.event_prev")
            .secondary()
            .on_press(move |_| {
                let selected = event_selected_state.get();
                if selected > 0 {
                    event_selected_state.set(selected - 1);
                }
            });
        let event_vector = tempo_events.clone();
        let event_selected_state = self.tempo_event_selected.clone();
        let event_next = ButtonView::new("EV+")
            .action_id("tempo.event_next")
            .secondary()
            .on_press(move |_| {
                let selected = event_selected_state.get();
                if (selected as usize) + 1 < event_vector.len() {
                    event_selected_state.set(selected + 1);
                }
            });
        let event_ramp_state = self.tempo_event_ramp.clone();
        let event_ramp = ButtonView::new(if self.tempo_event_ramp.get() {
            "RAMP ON"
        } else {
            "STEP ON"
        })
        .action_id("tempo.ramp_toggle")
        .secondary()
        .on_press(move |_| {
            event_ramp_state.set(!event_ramp_state.get());
        });
        let event_core = self.core.clone();
        let event_add_playhead = self.playhead_beats.clone();
        let event_add_tempo = self.tempo.clone();
        let event_add_ramp = self.tempo_event_ramp.clone();
        let event_add_selected = self.tempo_event_selected.clone();
        let event_add_count = event_total as u32;
        let event_add = ButtonView::new("ADD EVT")
            .action_id("tempo.event_add")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &event_core else { return };
                let beat = event_add_playhead.get().max(0.0);
                let bpm = f64::from(event_add_tempo.get());
                let ramp = event_add_ramp.get();
                if core.set_tempo_event(beat, bpm, ramp) {
                    event_add_selected.set(event_add_count);
                }
            });
        let event_del_core = self.core.clone();
        let event_del_target = tempo_events.get(event_selected).map(|(beat, _, _)| *beat);
        let event_del_button = ButtonView::new("DEL EVT")
            .action_id("tempo.event_delete")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &event_del_core else { return };
                if let Some(beat) = event_del_target {
                    let _ = core.remove_tempo_event(beat);
                }
            });
        let tempo_card = View::Card(vec![
            View::HStack(vec![
                View::Text("TEMPO".to_owned()),
                tempo_down.into(),
                tempo_up.into(),
                tempo_tap.into(),
                signature_button.into(),
                event_prev.into(),
                View::Caption(event_summary),
                event_next.into(),
                event_ramp.into(),
                event_add.into(),
                event_del_button.into(),
            ]),
            View::Caption(format!("BPM {:.1}", self.tempo.get())),
        ]);

        // OpenUtau vocal card (OpenUtau editor parity): score pick
        // + launch + render pick + import into a new Vocal track.
        let open_score_source = self.openutau_source.clone();
        let open_score_caption = self.openutau_caption.clone();
        let open_score_button = ButtonView::new("OPEN USTX")
            .action_id("openutau.open")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("OpenUtau score", &["ustx", "ust"]);
                let pending = open_score_source.clone();
                let caption = open_score_caption.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        let name = path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                        caption.set(format!("SCORE: {name}"));
                    }
                });
            });
        let open_score_source = self.openutau_source.clone();
        let open_score_caption = self.openutau_caption.clone();
        let launch_openutau = ButtonView::new("LAUNCH OPENUTAU")
            .action_id("openutau.launch")
            .secondary()
            .on_press(move |_| {
                let source = open_score_source
                    .lock()
                    .map(|mut slot| slot.take())
                    .unwrap_or_default();
                let Some(source) = source else {
                    open_score_caption.set("PICK A SCORE FIRST".to_owned());
                    return;
                };
                if let Err(error) =
                    hirari_core_bridge::openutau::validate_source(&source.to_string_lossy())
                {
                    open_score_caption.set(format!("INVALID SCORE · {error}"));
                    return;
                }
                let app = hirari_core_bridge::openutau::application_path();
                if !app.is_dir() {
                    open_score_caption.set(format!("OPENUTAU NOT FOUND · {}", app.display()));
                    return;
                }
                #[cfg(target_os = "macos")]
                {
                    match std::process::Command::new("open")
                        .arg("-a")
                        .arg(&app)
                        .arg(&source)
                        .spawn()
                    {
                        Ok(_) => {
                            open_score_caption.set(format!("OPENUTAU LAUNCHED · {}", app.display()))
                        }
                        Err(error) => {
                            open_score_caption.set(format!("OPENUTAU LAUNCH FAILED: {error}"))
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    open_score_caption
                        .set("OPENUTAU LAUNCH IS MAC-ONLY · RENDER MUST BE LINKED BY HAND".into());
                }
            });
        let open_render_source = self.openutau_render.clone();
        let open_render_caption = self.openutau_caption.clone();
        let pick_render_button = ButtonView::new("PICK RENDER")
            .action_id("openutau.pick_render")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Rendered vocal", &["wav", "aif", "aiff"]);
                let pending = open_render_source.clone();
                let caption = open_render_caption.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        let name = path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                        caption.set(format!("RENDER: {name}"));
                    }
                });
            });
        let import_source = self.openutau_source.clone();
        let import_render = self.openutau_render.clone();
        let import_pending = self.pending_openutau.clone();
        let import_caption = self.openutau_caption.clone();
        let import_button = ButtonView::new("IMPORT VOCAL")
            .action_id("openutau.import")
            .primary()
            .on_press(move |_| {
                let source = import_source
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let render = import_render
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let (Some(source), Some(render)) = (source, render) else {
                    import_caption.set("PICK BOTH A SCORE AND A RENDER FIRST".to_owned());
                    return;
                };
                if let Ok(mut slot) = import_pending.lock() {
                    *slot = Some((source, render));
                }
                import_caption.set("VOCAL IMPORT QUEUED".to_owned());
            });
        let openutau_card = View::Card(vec![View::HStack(vec![
            View::Text("VOCAL SCORE".to_owned()),
            open_score_button.into(),
            launch_openutau.into(),
            pick_render_button.into(),
            import_button.into(),
            View::Caption(self.openutau_caption.get()),
        ])]);

        // SYNTH card (synth panel parity): engine picker and the
        // eight engine macros via set_macro_value. Local echo mirrors what
        // was sent; no engine getter exists for these values yet.
        let synth_engine_state = self.synth_engine.clone();
        let engine_label = match self.synth_engine.get() {
            0 => "SYNTH",
            2 => "SAMPLER",
            _ => "WAVETABLE",
        };
        let synth_engine_core = self.core.clone();
        let synth_engine_button = ButtonView::new(format!("ENGINE: {engine_label}"))
            .action_id("synth.engine_cycle")
            .secondary()
            .on_press(move |_| {
                let next = (synth_engine_state.get() + 1) % 3;
                synth_engine_state.set(next);
                if let Some(core) = &synth_engine_core {
                    let _ = core.set_preview_synth_engine(next);
                }
            });
        let mut synth_macros: Vec<View> = vec![synth_engine_button.into()];
        for index in 0..8u32 {
            let macros_state = self.synth_macros.clone();
            let macro_core = self.core.clone();
            let current = self.synth_macros.get()[index as usize];
            let slider = SliderView::new(current, 0.0, 1.0)
                .action_id(format!("synth.macro.{index}"))
                .on_change(move |value| {
                    let mut macros = macros_state.get();
                    macros[index as usize] = value.clamp(0.0, 1.0);
                    macros_state.set(macros);
                    if let Some(core) = &macro_core {
                        let _ = core.set_macro_value(index, value.clamp(0.0, 1.0));
                    }
                });
            synth_macros.push(View::HStack(vec![
                View::Caption(format!("M{}", index + 1)),
                slider.into(),
            ]));
        }
        let synth_card = View::Card(vec![View::VStack(synth_macros)]);

        // SENDS card (audio routing / aux sends parity): source
        // cycler, destination bus cycler, gain slider with live apply,
        // pre/post toggle and send enable.
        let sends_card = {
            let tracks_for_sends = self.tracks.get();
            let source_count = tracks_for_sends.len() as u32;
            let bus_count = tracks_for_sends
                .iter()
                .filter(|track| track.kind == "Bus")
                .count() as u32;
            let source_index = if source_count == 0 {
                0
            } else {
                (self.send_source.get() as usize) % source_count as usize
            };
            let dest_index = if bus_count == 0 {
                0
            } else {
                (self.send_dest.get() as usize) % bus_count as usize
            };
            let source_label = tracks_for_sends
                .get(source_index)
                .map(|track| track.name.clone())
                .unwrap_or_else(|| "NO SOURCE".to_owned());
            let source_id = tracks_for_sends
                .get(source_index)
                .map(|track| track.id)
                .unwrap_or(0);
            let dest_track = tracks_for_sends
                .iter()
                .filter(|track| track.kind == "Bus")
                .nth(dest_index);
            let dest_label = dest_track
                .map(|track| track.name.clone())
                .unwrap_or_else(|| "NO BUS".to_owned());
            let dest_id = dest_track.map(|track| track.id).unwrap_or(0);
            let s_idx = self.send_source.clone();
            let send_source_button = ButtonView::new(format!("SRC: {source_label}"))
                .action_id("sends.source_cycle")
                .secondary()
                .on_press(move |_| {
                    if source_count > 0 {
                        s_idx.set((s_idx.get() + 1) % source_count);
                    }
                });
            let d_idx = self.send_dest.clone();
            let send_dest_button = ButtonView::new(format!("-> {dest_label}"))
                .action_id("sends.dest_cycle")
                .secondary()
                .on_press(move |_| {
                    if bus_count > 0 {
                        d_idx.set((d_idx.get() + 1) % bus_count);
                    }
                });
            let gain_state = self.send_gain.clone();
            let gain_apply_core = self.core.clone();
            let gain_source = source_id;
            let gain_dest = dest_id;
            let gain_enabled = self.send_enabled.clone();
            let gain_pre = self.send_pre.clone();
            let gain_slider = SliderView::new(self.send_gain.get(), 0.0, 1.0)
                .action_id("sends.gain")
                .on_change(move |value| {
                    let value = value.clamp(0.0, 1.0);
                    gain_state.set(value);
                    // Live-apply on change, like the aux-sends panel.
                    if let Some(core) = &gain_apply_core {
                        let _ = core.set_send_route(
                            gain_source,
                            gain_dest,
                            value,
                            gain_pre.get(),
                            gain_enabled.get(),
                        );
                    }
                });
            let pre_state = self.send_pre.clone();
            let pre_apply_core = self.core.clone();
            let pre_source = source_id;
            let pre_dest = dest_id;
            let pre_gain = self.send_gain.clone();
            let pre_enabled = self.send_enabled.clone();
            let send_pre_button = ButtonView::new(if self.send_pre.get() { "PRE" } else { "POST" })
                .action_id("sends.pre_toggle")
                .secondary()
                .on_press(move |_| {
                    let next = !pre_state.get();
                    pre_state.set(next);
                    if let Some(core) = &pre_apply_core {
                        let _ = core.set_send_route(
                            pre_source,
                            pre_dest,
                            pre_gain.get(),
                            next,
                            pre_enabled.get(),
                        );
                    }
                });
            let enable_core = self.core.clone();
            let enable_source = source_id;
            let enable_dest = dest_id;
            let enable_gain = self.send_gain.clone();
            let enable_pre = self.send_pre.clone();
            let enable_state = self.send_enabled.clone();
            let enable_button = ButtonView::new(if self.send_enabled.get() {
                "SEND ON"
            } else {
                "SEND OFF"
            })
            .action_id("sends.enable_toggle")
            .primary()
            .on_press(move |_| {
                let next = !enable_state.get();
                enable_state.set(next);
                if let Some(core) = &enable_core {
                    let _ = core.set_send_route(
                        enable_source,
                        enable_dest,
                        enable_gain.get(),
                        enable_pre.get(),
                        next,
                    );
                }
            });
            View::Card(vec![View::HStack(vec![
                View::Text("SENDS".to_owned()),
                send_source_button.into(),
                send_dest_button.into(),
                View::Caption("GAIN".to_owned()),
                gain_slider.into(),
                send_pre_button.into(),
                enable_button.into(),
            ])])
        };

        let video_card = View::Card(vec![View::HStack(vec![
            View::Text("VIDEO".to_owned()),
            video_load.into(),
            video_toggle.into(),
        ])]);

        let spotlight_card = {
            let query_state = self.spotlight_query.clone();
            let query_field = TextFieldView::new(
                self.spotlight_query.get(),
                "Filter commands (play, save, render...)",
            )
            .change_action_id("spotlight.query")
            .submit_action_id("spotlight.query")
            .on_change(move |value| query_state.set(value))
            .on_submit(|| {});
            let spotlight_core = self.core.clone();
            let play_toggle = self.playing.clone();
            let loop_toggle = self.loop_enabled.clone();
            let metro_toggle = self.metronome_enabled.clone();
            let filter = self.spotlight_query.get().to_lowercase();
            let commands: [(&str, SpotlightCommand); 13] = [
                ("TOGGLE PLAY", SpotlightCommand::TogglePlay),
                ("TOGGLE LOOP", SpotlightCommand::ToggleLoop),
                ("TOGGLE METRONOME", SpotlightCommand::ToggleMetronome),
                ("SAVE PROJECT", SpotlightCommand::Save),
                ("NEW PROJECT", SpotlightCommand::New),
                ("CYCLE SNAP", SpotlightCommand::SnapCycle),
                ("ZOOM IN", SpotlightCommand::ZoomIn),
                ("ZOOM OUT", SpotlightCommand::ZoomOut),
                ("FIT ARRANGEMENT", SpotlightCommand::Fit),
                ("RENDER MASTER", SpotlightCommand::RenderMaster),
                ("EXPORT MIDI", SpotlightCommand::ExportMidi),
                ("TOGGLE AUTO-LOCK", SpotlightCommand::LockToggle),
                ("UNDO", SpotlightCommand::Undo),
            ];
            let matches: Vec<(String, SpotlightCommand)> = if filter.is_empty() {
                commands
                    .into_iter()
                    .map(|(label, command)| (label.to_owned(), command))
                    .collect()
            } else {
                commands
                    .into_iter()
                    .filter(|(label, _)| label.to_lowercase().contains(&filter))
                    .map(|(label, command)| (label.to_owned(), command))
                    .collect()
            };
            let mut buttons: Vec<View> = Vec::new();
            for (label, command) in matches {
                let core = spotlight_core.clone();
                let playing = play_toggle.clone();
                let loop_on = loop_toggle.clone();
                let metro = metro_toggle.clone();
                let snap_state = self.arrange_snap_division.clone();
                let zoom_state = self.arrange_zoom.clone();
                let lock_state = self.arrange_locked.clone();
                let button = ButtonView::new(label.to_owned())
                    .action_id(format!("spotlight.{}", label.to_lowercase()))
                    .secondary()
                    .on_press(move |_| {
                        let Some(core) = &core else { return };
                        match command {
                            SpotlightCommand::TogglePlay => {
                                let next = !playing.get();
                                core.set_playing(next);
                                playing.set(next);
                            }
                            SpotlightCommand::ToggleLoop => {
                                let next = !loop_on.get();
                                core.set_loop(next);
                                loop_on.set(next);
                            }
                            SpotlightCommand::ToggleMetronome => {
                                let next = !metro.get();
                                core.set_metronome_enabled(next);
                                metro.set(next);
                            }
                            SpotlightCommand::Save => {
                                let _ = core.save_project(&core_default_project_path());
                            }
                            SpotlightCommand::New => {
                                core.new_project();
                            }
                            SpotlightCommand::SnapCycle => {
                                const DIVISIONS: [u32; 5] = [1, 2, 4, 8, 16];
                                let current = snap_state.get() as u32;
                                let next = DIVISIONS
                                    .iter()
                                    .find(|division| **division > current)
                                    .copied()
                                    .unwrap_or(DIVISIONS[0]);
                                snap_state.set(next as f32);
                            }
                            SpotlightCommand::ZoomIn => {
                                zoom_state.set((zoom_state.get() * 1.25).min(ARRANGE_ZOOM_MAX));
                            }
                            SpotlightCommand::ZoomOut => {
                                zoom_state.set((zoom_state.get() / 1.25).max(ARRANGE_ZOOM_MIN));
                            }
                            SpotlightCommand::Fit => {
                                zoom_state.set(1.0);
                            }
                            SpotlightCommand::RenderMaster => {
                                let path = default_render_output_path();
                                core.set_offline_render_tail_seconds(0.0);
                                let _ =
                                    core.start_render_async_to_format(&path.to_string_lossy(), 0);
                            }
                            SpotlightCommand::ExportMidi => {
                                let _ = core;
                            }
                            SpotlightCommand::LockToggle => {
                                lock_state.set(!lock_state.get());
                            }
                            SpotlightCommand::Undo => {
                                let _ = core.undo();
                            }
                        }
                    });
                buttons.push(button.into());
            }
            View::Card(vec![
                View::Text("CMD+K · QUICK".to_owned()),
                query_field.into(),
                View::HStack(buttons),
            ])
        };

        // AUDIO EDIT card (P14 partial): operate on the selected clip.
        let audio_edit_card = if let Some((track_id, clip_id)) = self.selected_clip.get() {
            let clips: Vec<_> = self
                .tracks
                .get()
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| track.clips.clone())
                .unwrap_or_default();
            let selected_clip = clips.iter().find(|clip| clip.id == clip_id).cloned();
            let edit_core = self.core.clone();
            let edit_refresh = self.tracks.clone();
            let reverse_current = selected_clip.as_ref().map(|c| c.reverse).unwrap_or(false);
            let reverse_button = ButtonView::new(if reverse_current {
                "UNREVERSE"
            } else {
                "REVERSE"
            })
            .action_id("audioedit.reverse")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &edit_core else { return };
                let _ = core.set_region_reverse(track_id, clip_id as u32, !reverse_current);
                edit_refresh.set(parse_arrange_tracks(
                    &core.get_project_layout_json(),
                    |samples| core.samples_to_beats(samples),
                ));
            });
            let pitch_label = format!(
                "PITCH: {}",
                selected_clip
                    .as_ref()
                    .map(|clip| format!("{:+.1} st", clip.pitch_semitones))
                    .unwrap_or_else(|| "?".to_owned())
            );
            let pitch_down_core = self.core.clone();
            let pitch_down_current = selected_clip
                .as_ref()
                .map(|c| c.pitch_semitones)
                .unwrap_or(0.0);
            let pitch_down_refresh = self.tracks.clone();
            let pitch_down = ButtonView::new("PITCH-")
                .action_id("audioedit.pitch_down")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &pitch_down_core else { return };
                    let _ = core.set_region_pitch_semitones(
                        track_id,
                        clip_id as u32,
                        pitch_down_current - 1.0,
                    );
                    pitch_down_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let pitch_up_core = self.core.clone();
            let pitch_up_current = selected_clip
                .as_ref()
                .map(|c| c.pitch_semitones)
                .unwrap_or(0.0);
            let pitch_up_refresh = self.tracks.clone();
            let pitch_up = ButtonView::new("PITCH+")
                .action_id("audioedit.pitch_up")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &pitch_up_core else { return };
                    let _ = core.set_region_pitch_semitones(
                        track_id,
                        clip_id as u32,
                        pitch_up_current + 1.0,
                    );
                    pitch_up_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let split_core = self.core.clone();
            let split_refresh = self.tracks.clone();
            let split_playhead = self.playhead_beats.clone();
            let split_button = ButtonView::new("SPLIT")
                .action_id("audioedit.split")
                .primary()
                .on_press(move |_| {
                    let Some(core) = &split_core else { return };
                    let beat = split_playhead.get();
                    let _ = core.split_region(track_id, clip_id as u32, beat);
                    split_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            // Fades: one slider each for fade-in and fade-out, applied against
            // the layout-tracked current value on every change.
            let clip_fade_in = selected_clip
                .as_ref()
                .map(|clip| clip.fade_in.clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let clip_fade_out = selected_clip
                .as_ref()
                .map(|clip| clip.fade_out.clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let fade_in_core = self.core.clone();
            let fade_in_refresh = self.tracks.clone();
            let fade_out_current = clip_fade_out;
            let fade_in_slider = SliderView::new(clip_fade_in, 0.0, 1.0)
                .action_id("audioedit.fade_in")
                .on_change(move |value| {
                    let clamped = value.clamp(0.0, 1.0);
                    if let Some(core) = &fade_in_core {
                        let _ = core.set_region_fades(
                            track_id,
                            clip_id as u32,
                            clamped,
                            fade_out_current,
                        );
                        fade_in_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    }
                });
            let fade_out_core = self.core.clone();
            let fade_out_refresh = self.tracks.clone();
            let fade_in_current = clip_fade_in;
            let fade_out_slider = SliderView::new(clip_fade_out, 0.0, 1.0)
                .action_id("audioedit.fade_out")
                .on_change(move |value| {
                    let clamped = value.clamp(0.0, 1.0);
                    if let Some(core) = &fade_out_core {
                        let _ = core.set_region_fades(
                            track_id,
                            clip_id as u32,
                            fade_in_current,
                            clamped,
                        );
                        fade_out_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    }
                });
            let loop_core = self.core.clone();
            let loop_refresh = self.tracks.clone();
            let loop_current = selected_clip
                .as_ref()
                .map(|clip| clip.loop_count)
                .unwrap_or(1);
            let loop_button = ButtonView::new(format!("LOOP {loop_current}×"))
                .action_id("audioedit.loop")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &loop_core else { return };
                    let next = match loop_current {
                        0 | 1 => 2,
                        2 => 4,
                        _ => 1,
                    };
                    let _ = core.set_region_loop_count(track_id, clip_id as u32, next);
                    loop_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let duplicate_core = self.core.clone();
            let duplicate_refresh = self.tracks.clone();
            let duplicate_playhead = self.playhead_beats.clone();
            let duplicate_button = ButtonView::new("DUPLICATE")
                .action_id("audioedit.duplicate")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &duplicate_core else { return };
                    let beat = duplicate_playhead.get();
                    let _ = core.duplicate_region(track_id, clip_id as u32, beat);
                    duplicate_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let delete_core = self.core.clone();
            let delete_refresh = self.tracks.clone();
            let delete_button = ButtonView::new("DELETE")
                .action_id("audioedit.delete")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &delete_core else { return };
                    let _ = core.remove_region(track_id, clip_id as u32);
                    delete_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            View::Card(vec![View::HStack(vec![
                View::Text("AUDIO EDIT".to_owned()),
                View::Caption(pitch_label),
                reverse_button.into(),
                pitch_down.into(),
                pitch_up.into(),
                split_button.into(),
                View::Caption("FADE IN".to_owned()),
                fade_in_slider.into(),
                View::Caption("FADE OUT".to_owned()),
                fade_out_slider.into(),
                loop_button.into(),
                duplicate_button.into(),
                delete_button.into(),
            ])])
        } else {
            View::Card(vec![View::HStack(vec![View::Caption(
                "Select a clip in the arrangement to enable audio edits (REVERSE, PITCH±, SPLIT)."
                    .to_owned(),
            )])])
        };

        // Flex & Pitch card (P8): warp/pitch preservation + sync group + sync-group quantize.
        let flex_card = if let Some((track_id, clip_id)) = self.selected_clip.get() {
            let clips = self
                .tracks
                .get()
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| track.clips.clone())
                .unwrap_or_default();
            let selected_clip = clips.iter().find(|clip| clip.id == clip_id).cloned();
            let clip_is_audio = selected_clip
                .as_ref()
                .map(|clip| !clip.midi)
                .unwrap_or(false);
            if clip_is_audio {
                let preserve_current = selected_clip
                    .as_ref()
                    .map(|clip| clip.pitch_preserve_warp)
                    .unwrap_or(false);
                let preserve_core = self.core.clone();
                let preserve_refresh = self.tracks.clone();
                let preserve_button = CheckboxView::new("Preserve pitch (WSOLA)", preserve_current)
                    .action_id("flex.preserve_pitch")
                    .on_toggle(move |next| {
                        if let Some(core) = &preserve_core {
                            let _ =
                                core.set_region_pitch_preserve_warp(track_id, clip_id as u32, next);
                            preserve_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let warp_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.warp_ratio.clamp(0.5, 2.0))
                    .unwrap_or(1.0);
                let warp_core = self.core.clone();
                let warp_refresh = self.tracks.clone();
                let warp_slider = SliderView::new(warp_value as f32, 0.5, 2.0)
                    .action_id("flex.warp_ratio")
                    .on_change(move |value| {
                        if let Some(core) = &warp_core {
                            let clamped = value.clamp(0.5, 2.0) as f64;
                            let _ = core.set_region_warp_ratio(track_id, clip_id as u32, clamped);
                            warp_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let pitch_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.pitch_semitones.clamp(-24.0, 24.0))
                    .unwrap_or(0.0);
                let pitch_core = self.core.clone();
                let pitch_refresh = self.tracks.clone();
                let pitch_slider = SliderView::new(pitch_value, -24.0, 24.0)
                    .action_id("flex.pitch_semitones")
                    .on_change(move |value| {
                        if let Some(core) = &pitch_core {
                            let _ =
                                core.set_region_pitch_semitones(track_id, clip_id as u32, value);
                            pitch_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let sync_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.sync_group.to_string())
                    .unwrap_or_else(|| "0".to_owned());
                let sync_text = self.flex_sync_group_text.clone();
                let sync_text_submit = self.flex_sync_group_text.clone();
                let sync_apply_core = self.core.clone();
                let sync_refresh = self.tracks.clone();
                let sync_field = TextFieldView::new(sync_value, "sync group id (0=unlinked)")
                    .change_action_id("flex.sync.text")
                    .submit_action_id("flex.sync.apply")
                    .on_change(move |value| {
                        sync_text.set(value);
                    })
                    .on_submit(move || {
                        let parsed = sync_text_submit.get().trim().parse::<u32>().ok();
                        let Some(group) = parsed else { return };
                        let Some(core) = &sync_apply_core else { return };
                        let _ = core.set_region_sync_group(track_id, clip_id as u32, group);
                        sync_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    });
                const GRID_NAMES: [&str; 13] = [
                    "1/4", "1/8", "1/16", "1/32", "1/64", "1/4T", "1/8T", "1/16T", "1/32T", "1/4D",
                    "1/8D", "1/16D", "1/32D",
                ];
                let grid_state = self.flex_quantize_grid.clone();
                let grid_label = GRID_NAMES
                    .get(self.flex_quantize_grid.get() as usize)
                    .copied()
                    .unwrap_or("1/16");
                let grid_button = ButtonView::new(format!("GRID {grid_label}"))
                    .action_id("flex.grid.cycle")
                    .secondary()
                    .on_press(move |_| {
                        grid_state.set((grid_state.get() + 1) % GRID_NAMES.len() as u32);
                    });
                let strength_state = self.flex_quantize_strength.clone();
                let strength_slider = SliderView::new(strength_state.get(), 0.0, 1.0)
                    .action_id("flex.quant.strength")
                    .on_change(move |value| {
                        strength_state.set(value.clamp(0.0, 1.0));
                    });
                let swing_state = self.flex_quantize_swing.clone();
                let swing_slider = SliderView::new(swing_state.get(), -1.0, 1.0)
                    .action_id("flex.quant.swing")
                    .on_change(move |value| {
                        swing_state.set(value.clamp(-1.0, 1.0));
                    });
                let quant_core = self.core.clone();
                let quant_refresh = self.tracks.clone();
                let quant_strength = self.flex_quantize_strength.clone();
                let quant_grid = self.flex_quantize_grid.clone();
                let quant_swing = self.flex_quantize_swing.clone();
                let quantize_button = ButtonView::new("QUANTIZE SYNC GROUP")
                    .action_id("flex.quantize")
                    .primary()
                    .on_press(move |_| {
                        if let Some(core) = &quant_core {
                            let _ = core.quantize_region_sync_group(
                                track_id,
                                clip_id as u32,
                                quant_strength.get(),
                                quant_grid.get() as u8,
                                quant_swing.get(),
                            );
                            quant_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                View::Card(vec![View::VStack(vec![
                    View::Text("FLEX & PITCH".to_owned()),
                    View::HStack(vec![
                        View::Caption("MODE".to_owned()),
                        preserve_button.into(),
                    ]),
                    View::HStack(vec![View::Caption("SPEED".to_owned()), warp_slider.into()]),
                    View::HStack(vec![View::Caption("PITCH".to_owned()), pitch_slider.into()]),
                    View::HStack(vec![
                        View::Caption("SYNC GROUP".to_owned()),
                        sync_field.into(),
                    ]),
                    View::HStack(vec![
                        View::Caption("QUANT STR".to_owned()),
                        strength_slider.into(),
                    ]),
                    View::HStack(vec![View::Caption("GRID".to_owned()), grid_button.into()]),
                    View::HStack(vec![View::Caption("SWING".to_owned()), swing_slider.into()]),
                    View::HStack(vec![quantize_button.into()]),
                ])])
            } else {
                View::Card(vec![View::HStack(vec![View::Caption(
                    "Flex & Pitch edits require an Audio or Vocal region selected.".to_owned(),
                )])])
            }
        } else {
            View::Card(vec![View::HStack(vec![View::Caption(
                "Select an Audio or Vocal clip in Arrange to edit flex-pitch.".to_owned(),
            )])])
        };

        // TRACK STACKS card (P10): Create/add/gain/collapse/delete stack
        // driven by the engine's track_stacks API.
        let stacks_card = {
            let create_core = self.core.clone();
            let create_selection = self.piano_track.clone();
            let create_selected_id = self.stack_selected_id.clone();
            let create_button = ButtonView::new("CREATE FROM SELECTED")
                .action_id("stacks.create")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &create_core else { return };
                    let member = create_selection.get();
                    if member == 0 {
                        return;
                    }
                    let json = core.track_stacks_json();
                    let stacks = parse_track_stacks(&json);
                    let next_id = stacks.iter().map(|stack| stack.id).max().unwrap_or(0) + 1;
                    let created_name = format!("Stack {next_id}");
                    let _ = core.upsert_track_stack(next_id, &created_name, &[member], 1.0, false);
                    create_selected_id.set(next_id);
                });
            let add_core = self.core.clone();
            let add_selection = self.piano_track.clone();
            let add_selected_id = self.stack_selected_id.clone();
            let add_button = ButtonView::new("ADD SELECTED")
                .action_id("stacks.add")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &add_core else { return };
                    let member = add_selection.get();
                    let stack_id = add_selected_id.get();
                    if stack_id == 0 || member == 0 {
                        return;
                    }
                    let _ = core.add_track_stack_member(stack_id, member);
                });
            let gains_core = self.core.clone();
            let gains_id = self.stack_selected_id.clone();
            let gain_slider = SliderView::new(self.stack_gain.get(), 0.0, 2.0)
                .action_id("stacks.gain")
                .on_change(move |value| {
                    let clamp = value.clamp(0.0, 2.0);
                    if let Some(core) = &gains_core {
                        let _ = core.set_track_stack_gain(gains_id.get(), clamp);
                    }
                });
            let collapse_state = self.stack_collapsed.clone();
            let collapse_id = self.stack_selected_id.clone();
            let collapse_core = self.core.clone();
            let collapse_button = CheckboxView::new("COLLAPSE", self.stack_collapsed.get())
                .action_id("stacks.collapse")
                .on_toggle(move |next| {
                    let id = collapse_id.get();
                    if id == 0 {
                        return;
                    }
                    if let Some(core) = &collapse_core {
                        let _ = core.set_track_stack_collapsed(id, next);
                    }
                    collapse_state.set(next);
                });
            let delete_core = self.core.clone();
            let delete_id = self.stack_selected_id.clone();
            let delete_button = ButtonView::new("DELETE")
                .action_id("stacks.delete")
                .secondary()
                .on_press(move |_| {
                    let id = delete_id.get();
                    if id == 0 {
                        return;
                    }
                    let Some(core) = &delete_core else { return };
                    let _ = core.delete_track_stack(id);
                    delete_id.set(0);
                });
            let caption = if self.stack_selected_id.get() == 0 {
                "No stack selected — CREATE FROM SELECTED to make one".to_owned()
            } else {
                format!(
                    "#{} · {}",
                    self.stack_selected_id.get(),
                    self.stack_name.get()
                )
            };
            View::Card(vec![View::HStack(vec![
                View::Text("TRACK STACKS".to_owned()),
                create_button.into(),
                add_button.into(),
                View::Caption(caption),
                gain_slider.into(),
                collapse_button.into(),
                delete_button.into(),
            ])])
        };

        // COMPING card (P10): take list from comping_snapshot_json, crossfade
        // slider wired to set_comp_crossfade_normalized, and per-take SELECT
        // (select_comp_take) wiring into the engine's undo/notification path.
        let comp_crossfade = self.comp_crossfade.clone();
        let comp_crossfade_ref = self.core.clone();
        let comp_crossfade_slider = SliderView::new(self.comp_crossfade.get(), 0.0, 1.0)
            .action_id("comping.crossfade")
            .on_change(move |value| {
                let clamped = value.clamp(0.0, 1.0);
                comp_crossfade.set(clamped);
                if let Some(core) = &comp_crossfade_ref {
                    let _ = core.set_comp_crossfade_normalized(clamped);
                }
            });
        let comp_takes = self.comping_takes.get();
        let mut comp_rows: Vec<View> = Vec::with_capacity(comp_takes.len() + 2);
        comp_rows.push(View::Text("COMPING".to_owned()));
        comp_rows.push(View::HStack(vec![
            View::Caption("CROSSFADE".to_owned()),
            comp_crossfade_slider.into(),
        ]));
        if comp_takes.is_empty() {
            comp_rows.push(View::Caption(
                "No takes yet. Record audio on a track to create a take lane.".to_owned(),
            ));
        } else {
            for take in comp_takes {
                let take_id = take.id;
                let take_name = take.name.clone();
                let take_span = take.end_sample.saturating_sub(take.start_sample) as f64
                    / take.start_sample.max(1) as f64;
                let _ = take_span;
                let select_core = self.core.clone();
                let select_button = ButtonView::new(format!("SELECT #{take_id} {take_name}"))
                    .action_id(format!("comping.select.{take_id}"))
                    .secondary()
                    .on_press(move |_| {
                        if let Some(core) = &select_core {
                            let _ = core.select_comp_take(take_id);
                        }
                    });
                comp_rows.push(View::HStack(vec![select_button.into()]));
            }
        }
        let comping_card = View::Card(vec![View::VStack(comp_rows)]);

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

        let master_gain_core = self.core.clone();
        let master_gain_state = self.master_gain.clone();
        let master_gain = SliderView::new(self.master_gain.get(), 0.0, 2.0)
            .action_id("mixer.master_gain")
            .on_change(move |value| {
                if let Some(core) = &master_gain_core {
                    if core.set_master_gain(value.clamp(0.0, 2.0)) {
                        master_gain_state.set(value.clamp(0.0, 2.0));
                    }
                }
            });
        let meter = View::Card(vec![View::HStack(vec![
            View::Text("Master".to_owned()),
            View::ProgressBar((peak * 100.0) as u8).grow(1),
            View::Caption(format!("{:.0}%", peak * 100.0)),
            View::Caption("Gain".to_owned()),
            master_gain.into(),
            View::Caption(status),
        ])]);

        // Audio settings (the audio settings parity): sample-rate and
        // buffer cyclers plus reconnect, read from the engine each poll.
        let audio_ready = self.audio_ready.get();
        let audio_devices = self.device_count.get();
        let audio_buffer = self.audio_buffer.get();
        let rate_core = self.core.clone();
        let rate_state = self.audio_buffer.clone();
        let rate_sample_rate = self.sample_rate.clone();
        let rate_status = self.status.clone();
        let rate_button = ButtonView::new(format!("{:.0} kHz", sample_rate / 1000.0))
            .action_id("audio.sample_rate_cycle")
            .secondary()
            .on_press(move |_| {
                const RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];
                let Some(core) = &rate_core else { return };
                let current = rate_sample_rate.get() as u32;
                let next = RATES
                    .iter()
                    .copied()
                    .find(|rate| *rate > current)
                    .unwrap_or(RATES[0]);
                if core.apply_audio_config(next, rate_state.get()) {
                    rate_status.set(format!("AUDIO {} Hz", next));
                } else {
                    rate_status.set("AUDIO CONFIG FAILED".to_owned());
                }
            });
        let buffer_core = self.core.clone();
        let buffer_state = self.audio_buffer.clone();
        let buffer_sample_rate = self.sample_rate.clone();
        let buffer_status = self.status.clone();
        let buffer_button = ButtonView::new(format!("{audio_buffer} spl"))
            .action_id("audio.buffer_cycle")
            .secondary()
            .on_press(move |_| {
                const SIZES: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];
                let Some(core) = &buffer_core else { return };
                let next = SIZES
                    .iter()
                    .copied()
                    .find(|size| *size > buffer_state.get())
                    .unwrap_or(SIZES[0]);
                buffer_state.set(next);
                let rate = buffer_sample_rate.get().max(1.0) as u32;
                if core.apply_audio_config(rate, next) {
                    buffer_status.set(format!("AUDIO BUFFER {}", next));
                } else {
                    buffer_status.set("AUDIO CONFIG FAILED".to_owned());
                }
            });
        let reconnect_core = self.core.clone();
        let reconnect_button = ButtonView::new("RECONNECT")
            .action_id("audio.reconnect")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &reconnect_core {
                    core.try_reconnect_audio_device();
                }
            });
        // Device cycle + select (on_select_device): the cycle picks
        // a pending index, USE applies it at the current config values.
        let device_list = self.audio_devices.get();
        let pending_index = self
            .device_index
            .get()
            .min(device_list.len().saturating_sub(1) as u32);
        let device_name = device_list
            .get(pending_index as usize)
            .map(|device| device.name.clone())
            .unwrap_or_else(|| "no device".to_owned());
        let cycle_state = self.device_index.clone();
        let device_count_for_cycle = device_list.len() as u32;
        let device_button = ButtonView::new(format!("DEVICE: {device_name}"))
            .action_id("audio.device_cycle")
            .secondary()
            .on_press(move |_| {
                if device_count_for_cycle > 0 {
                    cycle_state.set((cycle_state.get() + 1) % device_count_for_cycle);
                }
            });
        let use_core = self.core.clone();
        let use_devices = self.audio_devices.clone();
        let use_index = self.device_index.clone();
        let use_buffer = self.audio_buffer.clone();
        let use_sample_rate = self.sample_rate.clone();
        let use_status = self.status.clone();
        let use_button = ButtonView::new("USE")
            .action_id("audio.device_select")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &use_core else { return };
                let devices = use_devices.get();
                let index = use_index.get().min(devices.len().saturating_sub(1) as u32) as usize;
                let Some(device) = devices.get(index) else {
                    return;
                };
                let sample_rate = match use_sample_rate.get().round() as u32 {
                    44_100 | 48_000 | 88_200 | 96_000 | 192_000 => {
                        use_sample_rate.get().round() as u32
                    }
                    _ => 48_000,
                };
                let buffer_size = match use_buffer.get() {
                    32 | 64 | 128 | 256 | 512 | 1024 | 2048 => use_buffer.get(),
                    _ => 256,
                };
                if core.select_audio_device(device.id, sample_rate, buffer_size) {
                    use_status.set(format!("AUDIO DEVICE SELECTED: {}", device.name));
                } else {
                    use_status.set("DEVICE SELECTION FAILED".to_owned());
                }
            });
        let audio_card = View::Card(vec![View::HStack(vec![
            View::Text("AUDIO".to_owned()),
            device_button.into(),
            use_button.into(),
            rate_button.into(),
            buffer_button.into(),
            reconnect_button.into(),
            View::Caption(format!(
                "{} devices · {} · {:.1}ms",
                audio_devices,
                if audio_ready { "READY" } else { "NO DEVICE" },
                self.io_latency_ms.get()
            )),
        ])]);

        // Arrange toolbar (dock header): zoom steps match the dock
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
        // Automation edit mode (auto lanes): AUTO toggles lane
        // editing, the lane button cycles volume/pan, DEL removes the
        // selected automation point.
        let auto_state = self.auto_mode.clone();
        let auto_on = self.auto_mode.get();
        let auto_button = ButtonView::new(if auto_on { "AUTO ON" } else { "AUTO" })
            .action_id("arrange.auto_mode")
            .secondary()
            .on_press(move |_| {
                auto_state.set(!auto_on);
            });
        let auto_lane_state = self.auto_lane.clone();
        let auto_lane_label = if self.auto_lane.get() == 0 {
            "VOL"
        } else {
            "PAN"
        };
        let auto_lane_button = ButtonView::new(auto_lane_label)
            .action_id("arrange.auto_lane")
            .secondary()
            .on_press(move |_| {
                auto_lane_state.set((auto_lane_state.get() + 1) % 2);
            });
        let auto_del_core = self.core.clone();
        let auto_del_tracks = self.tracks.clone();
        let auto_del_point = self.auto_point.clone();
        let auto_del_track = self.auto_track.clone();
        let auto_del_lane = self.auto_lane.clone();
        let auto_del_locked = self.arrange_locked.clone();
        let auto_del_button = ButtonView::new("DEL PT")
            .action_id("arrange.auto_delete_point")
            .secondary()
            .on_press(move |_| {
                if auto_del_locked.get() {
                    return;
                }
                let Some(core) = &auto_del_core else { return };
                let Some(index) = auto_del_point.get() else {
                    return;
                };
                let tracks = auto_del_tracks.get();
                let track_id = auto_del_track.get();
                let lane = auto_del_lane.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut points: Vec<crate::ui_shared::arrange_model::AutomationPoint> = match lane {
                    0 => track.volume_automation.clone(),
                    _ => track.pan_automation.clone(),
                };
                if index >= points.len() {
                    return;
                }
                points.remove(index);
                let native: Vec<f64> = points
                    .iter()
                    .flat_map(|point| {
                        let time = core.beats_to_samples(f64::from(point.beat.max(0.0))) as f64;
                        [time, f64::from(point.value), 0.0]
                    })
                    .collect();
                core.begin_undo_transaction("Edit Automation");
                if core.set_automation_data(track_id, lane, native) {
                    let _ = core.end_undo_transaction();
                    auto_del_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                } else {
                    let _ = core.abort_undo_transaction();
                }
                auto_del_point.set(None);
            });
        let arrange_toolbar = View::Card(vec![View::HStack(vec![
            View::Text("ARRANGEMENT".to_owned()),
            zoom_out.into(),
            zoom_in.into(),
            zoom_fit.into(),
            snap_button.into(),
            lock_button.into(),
            auto_button.into(),
            auto_lane_button.into(),
            auto_del_button.into(),
            View::Caption(format!("ZOOM {}%", (zoom * 100.0).round() as u32)),
            View::Caption("ruler: pan · empty lane: seek · clip: move · edges: trim".to_owned()),
        ])]);

        // Clip inspector for the selected audio clip: fade in/out and
        // gain sliders, committing through the engine undo transactions
        // like the clip editor. MIDI clips have no region to edit.
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
                    .on_release(move |value| {
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
                    .on_release(move |value| {
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
                    .on_release(move |value| {
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
        // and an add stepper over the built-in chain (fx list).
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
                    let select_state = self.selected_plugin.clone();
                    let select_active = self.selected_plugin.get() == index as u32;
                    let select_button =
                        ButtonView::new(if select_active { "Selected" } else { "Params" })
                            .action_id(format!("plugin.select.{index}"))
                            .secondary()
                            .on_press(move |_| {
                                select_state.set(index as u32);
                            });
                    rows.push(View::HStack(vec![
                        View::Text(plugin_name(*plugin_type, index)),
                        select_button.into(),
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
        // controls, wired to the same engine setters as the mixer.
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
        let drum_state = self.drum_mode.clone();
        let drum_on = self.drum_mode.get();
        let drum_button = ButtonView::new(if drum_on { "PIANO" } else { "DRUM" })
            .action_id("piano.drum_mode")
            .secondary()
            .on_press(move |_| {
                drum_state.set(!drum_on);
            });
        piano_buttons.push(drum_button.into());
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
        // Chord controls (piano roll): root/quality/length/inversion
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
        // Progression insert: four chords
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
        // Articulation map: MAP cycles the
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
        // Articulation name field for the MAP id.
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
        // Lyric field. The text buffer is shared between the change
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
        // Articulation stepper for the selected note.
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

        // Timeline ruler above the arrange canvas: same zoom/scroll as the
        // canvas (px/beat units in beats-domain mode), 104px lane-header
        // indent, markers synced to the engine, scrub seeks the engine
        // playhead. The canvas keeps drawing lanes/clips below it.
        let last_timeline_width = (self.arrange_bounds.2 - 104.0).max(1.0);
        let timeline_end_beats = self.timeline_end_beats.get();
        let arrange_zoom = self.arrange_zoom.get();
        let base_px_per_beat = if timeline_end_beats > 0.0 {
            f64::from(last_timeline_width / timeline_end_beats)
        } else {
            60.0
        };
        let zoom_px_per_beat = base_px_per_beat * f64::from(arrange_zoom);
        let marker_list = self.markers.get();
        let mut sorted_markers = marker_list.clone();
        sorted_markers.sort_by(|a, b| {
            a.beat
                .partial_cmp(&b.beat)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        let timeline_zoom = self.arrange_zoom.clone();
        let timeline_scroll = self.arrange_scroll_beats.clone();
        let timeline_playhead = self.playhead_beats.clone();
        let timeline_seek_core = self.core.clone();
        let timeline_delete_core = self.core.clone();
        let timeline_move_core = self.core.clone();
        let sorted_marker_ids: Vec<u32> = sorted_markers.iter().map(|marker| marker.id).collect();
        let sorted_marker_delete_ids = sorted_marker_ids.clone();
        let sorted_marker_move_ids = sorted_marker_ids;
        let marker_add_core = self.core.clone();
        let marker_add_refresh = self.markers.clone();
        let marker_refresh = self.markers.clone();
        let arrange_timeline = TimelineView::new(
            self.playhead_beats.get(),
            f64::from(timeline_end_beats.max(1.0)),
            24,
        )
        .ruler_height(16)
        .header_indent(104)
        .beats_domain(4, 4)
        .zoom_if(Some(zoom_px_per_beat as f32))
        .offset_if(Some(f64::from(self.arrange_scroll_beats.get())))
        .markers(
            sorted_markers
                .iter()
                .map(|marker| TimelineMarker::new(marker.beat, marker.label.clone())),
        )
        .on_scrub(move |beat| {
            if (beat - timeline_playhead.get()).abs() < f64::EPSILON {
                return;
            }
            if let Some(core) = &timeline_seek_core {
                core.set_playhead(core.beats_to_samples(beat));
            }
        })
        .on_navigation(move |zoom, offset| {
            let clamped_zoom = (zoom as f64 / base_px_per_beat) as f32;
            timeline_zoom.set(clamped_zoom.clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX));
            timeline_scroll.set((offset as f32).max(0.0));
        })
        .on_marker_add(move |beat| {
            let Some(core) = &marker_add_core else {
                return;
            };
            let markers = marker_add_refresh.get();
            let next_id = markers.iter().map(|marker| marker.id).max().unwrap_or(0) + 1;
            let _ = core.upsert_marker(next_id, &format!("Mark {next_id}"), beat, "#646496");
        })
        .on_marker_delete(move |index| {
            let Some(core) = &timeline_delete_core else {
                return;
            };
            if let Some(id) = sorted_marker_delete_ids.get(index) {
                let _ = core.delete_marker(*id);
            }
        })
        .on_marker_move(move |index, beat| {
            let Some(core) = &timeline_move_core else {
                return;
            };
            let markers = marker_refresh.get();
            let Some(marker) = sorted_marker_move_ids
                .get(index)
                .and_then(|id| markers.iter().find(|marker| marker.id == *id))
            else {
                return;
            };
            let _ = core.upsert_marker(marker.id, &marker.label, beat, "#646496");
        });
        let arrange_timeline: View = arrange_timeline.into();

        if self.show_genesis.get() {
            let mut template_buttons = Vec::new();
            for (label, template) in [
                ("ELECTRONIC", "ELECTRONIC"),
                ("CINEMATIC SCORE", "CINEMATIC SCORE"),
                ("ATMOS MASTERING", "ATMOS MASTERING"),
                ("VOICE-OVER PRO", "VOICE-OVER PRO"),
            ] {
                let template_core = self.core.clone();
                let template_selected = self.selected_clip.clone();
                let template_status = self.status.clone();
                let template_show = self.show_genesis.clone();
                let template_name = template.to_string();
                let button = ButtonView::new(label)
                    .action_id(format!("genesis.template.{label}"))
                    .secondary()
                    .on_press(move |_| {
                        let Some(core) = &template_core else { return };
                        match create_template_project(core, &template_name) {
                            Ok(()) => {
                                template_selected.set(None);
                                template_show.set(false);
                                template_status.set(format!("STARTED {template_name}"));
                            }
                            Err(error) => {
                                template_status.set(format!("TEMPLATE FAILED: {error}"));
                            }
                        }
                    });
                template_buttons.push(button.into());
            }
            let blank_core = self.core.clone();
            let blank_selected = self.selected_clip.clone();
            let blank_status = self.status.clone();
            let blank_show = self.show_genesis.clone();
            let blank_button = ButtonView::new("BLANK")
                .action_id("genesis.template.BLANK")
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &blank_core {
                        core.new_project();
                        blank_selected.set(None);
                        blank_show.set(false);
                        blank_status.set("NEW PROJECT".to_owned());
                    }
                });
            template_buttons.push(blank_button.into());
            let mut recent_buttons: Vec<View> = Vec::new();
            for project in crate::ui_shared::recent_projects::load_recent_projects() {
                let open_pending = self.pending_open.clone();
                let path = project.path.to_string();
                let file_name = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Project")
                    .to_owned();
                let button = ButtonView::new(file_name)
                    .action_id("genesis.recent")
                    .secondary()
                    .on_press(move |_| {
                        if let Ok(mut slot) = open_pending.lock() {
                            *slot = Some(std::path::PathBuf::from(path.clone()));
                        }
                    });
                recent_buttons.push(button.into());
            }
            let genesis = View::Card(vec![
                View::Text("HIRARI".to_owned()),
                View::Caption("Pick a starting point or open a recent project".to_owned()),
                View::HStack(template_buttons),
                View::Caption("RECENT PROJECTS".to_owned()),
                View::HStack(recent_buttons),
            ]);
            return View::VStack(vec![project_bar, genesis]);
        }

        let integrated_lufs = self.integrated_lufs.get();
        let true_peak_l = self.true_peak_l.get();
        let true_peak_r = self.true_peak_r.get();
        let integrated_label = if integrated_lufs <= -119.0 {
            "— LUFS".to_owned()
        } else {
            format!("{integrated_lufs:.1} LUFS")
        };
        let master_card = View::Card(vec![View::HStack(vec![
            View::Text("MASTER".to_owned()),
            View::Caption(format!("GAIN {:.2}", self.master_gain.get())),
            View::Caption(integrated_label),
            View::Caption(format!(
                "TP L {true_peak_l:.1} dBTP · R {true_peak_r:.1} dBTP"
            )),
            View::Caption("Target −14 LUFS / −1 dBTP".to_owned()),
        ])]);
        let mut views = vec![project_bar];
        if self.show_diagnostics.get() {
            views.push(diagnostics_card);
        }
        if self.show_moufu.get() {
            views.push(moufu_card);
        }
        if self.show_sends.get() {
            views.push(sends_card);
        }
        views.push(synth_card);
        views.push(openutau_card);
        views.push(audio_edit_card);
        if self.show_spotlight.get() {
            views.insert(1, spotlight_card);
        }
        if self.focus_mode.get() {
            views.extend([
                transport,
                meter,
                View::canvas("waveform").frame(None, Some(168)),
                arrange_timeline,
                View::canvas("arrange").grow(3),
            ]);
        } else {
            views.extend([
                export_card,
                tempo_card,
                flex_card,
                stacks_card,
                comping_card,
                transport,
                meter,
                master_card,
                audio_card,
                video_card,
                arrange_toolbar,
                View::canvas("waveform").frame(None, Some(168)),
                if self.show_video.get() {
                    View::canvas("video").frame(None, Some(200))
                } else {
                    View::Caption(String::new())
                },
                arrange_timeline,
                View::canvas("arrange").grow(3),
            ]);
        }
        if let Some(inspector) = inspector {
            views.push(inspector);
        }
        if let Some(plugin_strip) = plugin_strip {
            views.push(plugin_strip);
        }
        // Plugin parameter strip for the selected plugin of the selected
        // track (normalized 0..1 sliders, as in the plugin UI).
        if let Some(core) = &self.core {
            let params_track = tracks
                .iter()
                .find(|track| track.id == self.piano_track.get());
            if let Some(track) = params_track {
                let params_track_id = track.id;
                let plugin_count = track.plugin_types.len() as u32;
                if plugin_count > 0 {
                    let plugin_index = self.selected_plugin.get().min(plugin_count - 1);
                    let count = core
                        .get_plugin_parameter_count(params_track_id, plugin_index)
                        .min(32);
                    if count > 0 {
                        let param_locked = self.arrange_locked.get();
                        let mut param_rows: Vec<View> = vec![View::Text(format!(
                            "PARAMS · {}",
                            plugin_name(
                                track.plugin_types[plugin_index as usize],
                                plugin_index as usize
                            )
                        ))];
                        for parameter_id in 0..count {
                            let name = core.get_plugin_parameter_name(
                                params_track_id,
                                plugin_index,
                                parameter_id,
                            );
                            let label = if name.trim().is_empty() {
                                format!("Parameter {parameter_id}")
                            } else {
                                name
                            };
                            let value = core
                                .get_plugin_parameter(params_track_id, plugin_index, parameter_id)
                                .clamp(0.0, 1.0);
                            let param_core = self.core.clone();
                            let slider = SliderView::new(value, 0.0, 1.0)
                                .action_id(format!("plugin.param.{plugin_index}.{parameter_id}"))
                                .disabled(param_locked)
                                .on_change(move |new_value| {
                                    if param_locked {
                                        return;
                                    }
                                    if let Some(core) = &param_core {
                                        let _ = core.set_plugin_parameter(
                                            params_track_id,
                                            plugin_index,
                                            parameter_id,
                                            new_value.clamp(0.0, 1.0),
                                        );
                                    }
                                });
                            param_rows
                                .push(View::HStack(vec![View::Caption(label), slider.into()]));
                        }
                        // Preset save/load for this plugin
                        // on_save_preset/on_load_preset via native dialogs).
                        let preset_pending = self.pending_preset.clone();
                        let preset_track = params_track_id;
                        let preset_plugin = plugin_index;
                        for (label, is_save) in [("SAVE PRESET", true), ("LOAD PRESET", false)] {
                            let pending = preset_pending.clone();
                            let preset_button = ButtonView::new(label)
                                .action_id(format!(
                                    "plugin.preset.{}",
                                    if is_save { "save" } else { "load" }
                                ))
                                .secondary()
                                .on_press(move |_| {
                                    let dialog = ame_ui::platform::NativeFileDialog::new()
                                        .add_filter("Hirari Plugin Preset", &["aupreset"]);
                                    let pending = pending.clone();
                                    if is_save {
                                        dialog.save_file_async(move |path| {
                                            if let Some(path) = path {
                                                if let Ok(mut slot) = pending.lock() {
                                                    *slot = Some((
                                                        path,
                                                        true,
                                                        preset_track,
                                                        preset_plugin,
                                                    ));
                                                }
                                            }
                                        });
                                    } else {
                                        dialog.pick_file_async(move |path| {
                                            if let Some(path) = path {
                                                if let Ok(mut slot) = pending.lock() {
                                                    *slot = Some((
                                                        path,
                                                        false,
                                                        preset_track,
                                                        preset_plugin,
                                                    ));
                                                }
                                            }
                                        });
                                    }
                                });
                            param_rows.push(preset_button.into());
                        }
                        views.push(View::Card(vec![View::VStack(param_rows)]));
                    }
                }
            }
        }
        views.extend([piano_selector, View::canvas("piano").grow(2), track_list]);
        // Audio library: folder scan, search
        // filter, per-asset preview and import to the piano track.
        let library_assets = self.library_assets.get();
        let library_filter = self.library_query.get().to_lowercase();
        let scan_pending = self.pending_library.clone();
        let scan_button = ButtonView::new("SCAN")
            .action_id("library.scan")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new();
                let pending = scan_pending.clone();
                dialog.pick_folder_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let search_state = self.library_query.clone();
        let library_search = TextFieldView::new(library_filter.clone(), "Search sounds")
            .change_action_id("library.search")
            .submit_action_id("library.search")
            .on_change(move |value| {
                search_state.set(value);
            });
        let mut library_rows: Vec<(ame_ui::view::StableKey, View)> = Vec::new();
        for asset in library_assets.iter().filter(|asset| {
            library_filter.is_empty() || asset.name.to_lowercase().contains(&library_filter)
        }) {
            let play_core = self.core.clone();
            let play_path = asset.path.clone();
            let play_button = ButtonView::new("PLAY")
                .action_id(format!("library.preview.{}", asset.id))
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &play_core {
                        let _ = core.register_preview_audio(&play_path);
                    }
                });
            let import_core = self.core.clone();
            let import_path = asset.path.clone();
            let import_track = self.piano_track.clone();
            let import_playhead = self.playhead_beats.clone();
            let import_status = self.status.clone();
            let import_ticket_slot = self.library_ticket.clone();
            let import_button = ButtonView::new("IMPORT")
                .action_id(format!("library.import.{}", asset.id))
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &import_core {
                        let track_id = import_track.get();
                        if track_id == 0 {
                            return;
                        }
                        let ticket = core.queue_region_import_at_beat(
                            track_id,
                            &import_path,
                            import_playhead.get(),
                        );
                        if ticket == 0 {
                            import_status.set("IMPORT FAILED".to_owned());
                        } else {
                            if let Ok(mut slot) = import_ticket_slot.lock() {
                                *slot = Some((ticket, import_path.clone(), track_id));
                            }
                            import_status.set("IMPORTING: Audio".to_owned());
                        }
                    }
                });
            library_rows.push((
                ame_ui::view::StableKey::from(asset.id),
                View::HStack(vec![
                    View::Text(asset.name.clone()),
                    View::Caption(format!(
                        "{} Hz · {} bytes",
                        asset.sample_rate as u32, asset.size_bytes
                    )),
                    play_button.into(),
                    import_button.into(),
                ]),
            ));
        }
        views.push(View::Card(vec![
            View::HStack(vec![
                View::Text("AUDIO LIBRARY".to_owned()),
                scan_button.into(),
                library_search.into(),
            ]),
            // Variable-height virtual list: large sound folders stay
            // scrollable without laying out every row up front.
            View::virtual_list_variable_height(140, 32, 4, library_rows),
        ]));
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
