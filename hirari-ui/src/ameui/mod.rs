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
use std::collections::{HashMap, HashSet, VecDeque};
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
use hirari_core_bridge::project_contracts::MidiLearnMappingContract;
use hirari_core_bridge::project_contracts::MidiNoteContract;
use hirari_moufu_adapter::{
    LinkStatus, MoufuEvent, HIRARI_PROJECT_DATA_TYPE, HIRARI_PROJECT_ENTITY,
};
use std::sync::atomic::Ordering;

mod cards;
mod midi_learn;
mod view;

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
    /// MIDI LEARN panel toggle (CoreMIDI poll + CC-to-macro bindings).
    show_midi: State<bool>,
    /// 1..=8 = the matching engine macro is armed to learn the next CC;
    /// 0 = not armed.
    midi_armed_macro: State<u32>,
    /// Status line for the MIDI LEARN card.
    midi_status: State<String>,
    /// OpenUtau tuning values mirrored from the engine's vocal contract; all normalized 0..=1.
    openutau_scoop: State<f32>,
    openutau_vibrato: State<f32>,
    openutau_dynamics: State<f32>,
    openutau_consonants: State<f32>,
    /// Notated staff view (grand staff for the selected MIDI track).
    show_notation: State<bool>,
    /// Control-room panel toggle (monitor bus only).
    show_chords: State<bool>,
    show_control_room: State<bool>,
    /// Speaker names registered in the control room this session, in order.
    control_room_speakers: State<Vec<String>>,
    /// Selected speaker index within the session list.
    control_room_speaker_index: State<u32>,
    /// Dim state mirrored from the engine each refresh.
    control_room_dim: State<bool>,
    /// Dim depth in dB.
    control_room_dim_db: State<f32>,
    /// Talkback state mirrored from the engine each refresh.
    talkback_enabled: State<bool>,
    /// Talkback gain (0..4).
    talkback_gain: State<f32>,
    /// Step-sequencer panel toggle.
    show_step_seq: State<bool>,
    /// Persisted step-sequencer patterns (from the engine each refresh).
    step_seq_patterns: State<Vec<hirari_core_bridge::project::StepSequencerPatternContract>>,
    /// Page index within the pattern (1..=8).
    step_page: State<u32>,
    /// Velocity for newly inserted step notes.
    step_velocity: State<f32>,
    /// Probability for newly inserted step notes (percent).
    step_probability: State<f32>,
    /// Gate percent for newly inserted step notes.
    step_gate: State<f32>,
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
    spectrum_image: Option<CanvasTexture>,
    spectrum_reusable: Option<wgpu::Texture>,
    /// Spectrum analyser toggle (FFT-derived, drawn on its own canvas).
    show_spectrum: State<bool>,
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
    /// MIDI LEARN: true once `core.start_midi_input()` has succeeded; false
    /// after we deliberately stopped input or the device was unavailable.
    midi_input_started: bool,
    /// Per-source running-status byte for the CoreMIDI decoder.
    midi_running_status: HashMap<i32, Option<u8>>,
    /// Last ~30 s of Note On/Off events on the MIDI input, used by the
    /// "Insert buffered MIDI" path.
    midi_retrospective: Rc<std::cell::RefCell<VecDeque<midi_learn::BufferedMidiEvent>>>,
    /// Track id to run a retrospective insert for, requested by the MIDI
    /// LEARN card; the poll performs the insert so it can borrow `self`.
    midi_insert_pending: Rc<std::cell::RefCell<Option<u32>>>,
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
        show_midi: context.state(false),
        midi_armed_macro: context.state(0u32),
        midi_status: context.state("MIDI INPUT OFF".to_owned()),
        show_spotlight: context.state(false),
        spotlight_query: context.state(String::new()),
        focus_mode: context.state(crate::ui_shared::preferences::load_focus_mode()),
        openutau_scoop: context.state(0.35f32),
        openutau_vibrato: context.state(0.45f32),
        openutau_dynamics: context.state(0.60f32),
        openutau_consonants: context.state(0.50f32),
        show_notation: context.state(false),
        show_chords: context.state(false),
        show_control_room: context.state(false),
        control_room_speakers: context.state(Vec::new()),
        control_room_speaker_index: context.state(0u32),
        control_room_dim: context.state(false),
        control_room_dim_db: context.state(-20.0f32),
        talkback_enabled: context.state(false),
        talkback_gain: context.state(1.0f32),
        show_step_seq: context.state(false),
        step_seq_patterns: context.state(Vec::new()),
        step_page: context.state(1u32),
        step_velocity: context.state(100.0f32),
        step_probability: context.state(100.0f32),
        step_gate: context.state(80.0f32),
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
        spectrum_image: None,
        spectrum_reusable: None,
        show_spectrum: context.state(false),
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
        midi_input_started: false,
        midi_running_status: HashMap::new(),
        midi_retrospective: Rc::new(std::cell::RefCell::new(VecDeque::new())),
        midi_insert_pending: Rc::new(std::cell::RefCell::new(None)),
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

    /// MIDI LEARN poll. Runs while the MIDI LEARN panel is open: starts
    /// CoreMIDI on entry, drains input packets, pushes Note On/Off events
    /// into the 30-second retrospective buffer, binds an armed macro to the
    /// next CC message seen, and mirrors the last event into the status.
    fn poll_midi_learn(&mut self, core: &HirariCore) {
        if !self.midi_input_started {
            if !core.start_midi_input() {
                self.midi_status.set("MIDI INPUT UNAVAILABLE".to_owned());
                return;
            }
            self.midi_input_started = true;
            self.midi_status
                .set("MIDI INPUT READY · ARM A MACRO THEN MOVE A CC".to_owned());
        }
        // A retrospective insert was requested by the card; perform it here so
        // this borrows `self` freely.
        if let Some(track_id) = self.midi_insert_pending.borrow_mut().take() {
            let is_midi = self.tracks.get().iter().any(|track| {
                track.id == track_id
                    && matches!(track.kind.as_str(), "Midi" | "MIDI" | "Instrument")
            });
            match midi_learn::insert_retrospective_notes(
                track_id,
                &self.midi_retrospective,
                core,
                is_midi,
            ) {
                Ok(count) => {
                    self.midi_retrospective.borrow_mut().clear();
                    self.midi_status
                        .set(format!("INSERTED {count} NOTES · UNDO AVAILABLE"));
                    self.refresh_arrange_state(core);
                }
                Err(message) => {
                    self.midi_status.set(message);
                    if is_midi {
                        self.midi_retrospective.borrow_mut().clear();
                    }
                }
            }
        }
        for packet in
            serde_json::from_str::<Vec<midi_learn::InputPacket>>(&core.poll_midi_input_json())
                .unwrap_or_default()
        {
            let device_id = format!("coremidi:{}:input", packet.source_unique_id);
            let running_status = self
                .midi_running_status
                .entry(packet.source_unique_id)
                .or_default();
            for message in midi_learn::decode_midi_messages(&packet.data_hex, running_status) {
                let event_host_time = if packet.host_time == 0 {
                    core.midi_host_time_now()
                } else {
                    packet.host_time
                };
                if matches!(
                    message,
                    midi_learn::MidiMessage::NoteOn { .. }
                        | midi_learn::MidiMessage::NoteOff { .. }
                ) {
                    let sample_rate = core.get_sample_rate().max(1.0);
                    let mut buffer = self.midi_retrospective.borrow_mut();
                    buffer.push_back(midi_learn::BufferedMidiEvent {
                        source_id: packet.source_unique_id,
                        host_time: event_host_time,
                        message,
                    });
                    while buffer.len() > midi_learn::MAX_RETROSPECTIVE_MIDI_EVENTS {
                        buffer.pop_front();
                    }
                    while buffer.front().is_some_and(|event| {
                        core.midi_host_time_delta_samples(
                            event.host_time,
                            event_host_time,
                            sample_rate,
                        ) > sample_rate as u64 * midi_learn::RETROSPECTIVE_MIDI_SECONDS
                    }) {
                        buffer.pop_front();
                    }
                }
                let midi_learn::MidiMessage::ControlChange {
                    channel,
                    controller,
                    value,
                } = message
                else {
                    continue;
                };
                let armed = self.midi_armed_macro.get();
                let learned = if armed > 0 {
                    let macro_index = armed - 1;
                    let mapping = MidiLearnMappingContract {
                        mapping_id: format!(
                            "midi:{}:{}:{}:macro:{}",
                            packet.source_unique_id, channel, controller, macro_index
                        ),
                        device_id: device_id.clone(),
                        channel,
                        controller: controller as u16,
                        target_instance_id: format!("macro:{macro_index}"),
                        target_parameter_id: "value".to_owned(),
                        min: 0.0,
                        max: 1.0,
                        curve: 0.0,
                        pickup: false,
                        macro_group: None,
                    };
                    match core.add_midi_learn_mapping(mapping) {
                        Ok(()) => {
                            self.midi_armed_macro.set(0);
                            Some(macro_index)
                        }
                        Err(error) => {
                            self.midi_status.set(format!("MIDI LEARN FAILED: {error}"));
                            None
                        }
                    }
                } else {
                    None
                };
                if learned.is_some() {
                    self.refresh_arrange_state(core);
                }
                core.handle_midi_cc_from_device(&device_id, channel, controller, value);
                self.midi_status.set(match learned {
                    Some(macro_index) => format!(
                        "LEARNED CH {} · CC {} → MACRO {}",
                        channel + 1,
                        controller,
                        macro_index + 1
                    ),
                    None => format!("MIDI CH {} · CC {} = {}", channel + 1, controller, value),
                });
            }
        }
    }

    fn stop_midi_learn(&mut self, core: &HirariCore) {
        if self.midi_input_started {
            core.stop_midi_input();
            self.midi_input_started = false;
        }
        self.midi_armed_macro.set(0);
        self.midi_retrospective.borrow_mut().clear();
    }

    fn poll_engine(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        self.poll_render();
        self.poll_moufu();
        self.poll_track_stacks();
        if self.show_midi.get() {
            self.poll_midi_learn(&core);
        } else if self.midi_input_started {
            self.stop_midi_learn(&core);
            self.midi_status.set("MIDI INPUT OFF".to_owned());
        }
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
        let bands = core.get_fft_bands();
        if !bands.is_empty() {
            self.plots.publish_spectrum(&bands);
        }

        // The layout walk is comparatively costly; refresh it a few times a
        // second rather than on every 30 Hz tick.
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 10 == 1 {
            self.refresh_arrange_state(&core);
            self.control_room_dim.set(core.control_room_dimmed());
            self.control_room_dim_db.set(core.control_room_dim_db());
            self.talkback_enabled
                .set(core.control_room_talkback_enabled());
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
                    let _ = core.register_openutau_vocal(
                        &source.to_string_lossy(),
                        &render.to_string_lossy(),
                    );
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
        self.step_seq_patterns
            .set(serde_json::from_str(&core.step_sequencer_patterns_json()).unwrap_or_default());
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
    /// Grand-staff renderer for the selected MIDI/Instrument track. Two
    /// staves with 4/4 barlines (from the engine's measure-starts list),
    /// diatonic noteheads with ledger lines and stem direction per rule,
    /// hollow heads for half+ notes, accidentals as ASCII `#`. Fontless
    /// glyphs only — no music font is required.
    fn draw_notation(
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
