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

mod canvas_impl;
mod cards;
mod midi_learn;
mod shell;
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

// Tests must never race the machine's physical audio device; the bridge
// honors cfg!(test) the same way for its own unit tests.
#[cfg(test)]
fn core_for_headless() -> Option<Rc<HirariCore>> {
    hirari_core_bridge::HirariCore::new_offline()
        .ok()
        .map(Rc::new)
}

/// Build the app on the event-loop (main) thread, where the engine must live.
/// In unit test builds this never opens a physical audio device.
pub fn create_app(context: &StateContext) -> HirariAmeApp {
    #[cfg(test)]
    let (core, status) = match core_for_headless() {
        Some(core) => (Some(core), String::from("ENGINE READY")),
        None => (None, String::from("ENGINE INIT FAILED")),
    };
    #[cfg(not(test))]
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

#[cfg(test)]
mod headless_harness_tests {
    use super::*;

    #[test]
    fn app_builds_and_lays_out_at_default_viewport() {
        let (app, _context) = {
            let context = ame_ui::app::StateContext::default();
            let app = create_app(&context);
            (app, context)
        };
        let viewport = ame_ui::app::Viewport {
            width: 1280,
            height: 800,
        };
        let mut harness =
            ame_ui::testing::UiTestHarness::new(app, viewport).expect("harness must build");
        let tree = harness.tree();
        assert!(
            tree.root.children.len() > 0,
            "app must render a non-empty tree"
        );
        let snapshot = harness
            .capture_snapshot()
            .expect("headless capture must work");
        assert_eq!(snapshot.width(), 1280);
        assert_eq!(snapshot.height(), 800);
    }
}
