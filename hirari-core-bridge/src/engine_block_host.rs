//! Rust owns the engine's block: the admission head, the graph walk and the
//! signal that leaves it.
//!
//! The C++ callback used to gate the block, wrap the playhead over the cycle,
//! drain the track commands, stage the timeline's MIDI into every track, render
//! the built-in preview voices, then stage each track's block, inject feedback,
//! feed the live driver input, publish the routing destination table, walk the
//! graph in process order fanning each track into its buses and the master, and
//! finally capture feedback, publish metering, render the metronome, scale and
//! limit the master, measure it, mix the Control Room cue and advance the
//! playhead -- 500 lines of realtime policy inlined in `processBlock`, plus the
//! transport atomics, the click scratch and the double-buffered telemetry
//! handoff. The engine binds its handles once into this state and calls
//! [`hirari_engine_block_begin`], [`hirari_engine_block_route`] and
//! [`hirari_engine_block_finish`] around the device buffer it is rendering.
//!
//! Tracks stay C++ objects, but every read and write the block makes on one is
//! now a Rust call on a storage handle it asks for by kind -- the runtime, the
//! channel strip, the articulation state, the MIDI input buffer and a bus's
//! stage. What stays in the published operations table is the handle selection,
//! the track's role, the armed recorder and the control-edit protocol (the
//! frozen-render drop and the layout-revision bump) that a drained command keeps
//! around its store write. Every tap buffer the walk reads is already bound in
//! the track's [`TrackRuntime`].

use crate::audio_buffer_ops::hirari_audio_add_sanitized_stereo;
use crate::audio_test_tone::hirari_audio_test_tone_render;
use crate::bounce_driver::{
    hirari_bounce_offline_active, hirari_bounce_offline_blocks, hirari_bounce_offline_engaged,
    hirari_bounce_offline_thread, hirari_bounce_include_inserts, hirari_bounce_pre_fader,
    hirari_bounce_target_active, hirari_bounce_target_id, hirari_callback_thread_token,
};
use crate::bus_audio::{
    hirari_bus_audio_accumulate, hirari_bus_audio_read, hirari_bus_audio_replace_post,
    hirari_bus_system_clear,
};
use crate::channel_strip::{
    hirari_channel_strip_is_solo, hirari_channel_strip_set_gain, hirari_channel_strip_set_pan,
    hirari_channel_strip_set_solo,
};
use crate::command_host::{
    hirari_engine_audio_command, AUDIO_COMMAND_MUTE, AUDIO_COMMAND_PAN, AUDIO_COMMAND_SOLO,
    AUDIO_COMMAND_VOLUME,
};
use crate::control_room_audio::hirari_control_room_mix_cue;
use crate::control_room_runtime::{
    hirari_control_room_active_cue_bus, hirari_control_room_active_cue_click,
    hirari_control_room_active_cue_gain, hirari_control_room_active_cue_output,
    hirari_control_room_is_cue_bus, hirari_control_room_process_monitor_state,
};
use crate::engine_input_block::{hirari_input_blocks_take_monitor, hirari_input_blocks_take_talkback};
use crate::engine_rings::{hirari_engine_command_ring_pop, CommandRecord, EngineRings};
use crate::engine_clock::{hirari_engine_chunk_plan, CHUNK_RESTART_PLAYHEAD};
use crate::engine_track_table::{hirari_engine_track_snapshot, HirariTrackSnapshot};
use crate::forensic_scratchpad::hirari_forensic_scratchpad_reset;
use crate::hot_swap_state::{
    hirari_hot_swap_default_acknowledge, hirari_hot_swap_default_quiesce_requested,
};
use crate::master_limiter::hirari_master_limiter_process;
use crate::master_meter::hirari_master_meter_process;
use crate::master_output::hirari_master_output_process;
use crate::metronome_renderer::{hirari_metronome_is_enabled, hirari_metronome_process};
use crate::midi_buffer_ops::{
    hirari_midi_buffer_add, hirari_midi_buffer_add_note_off, hirari_midi_buffer_add_note_on,
    hirari_midi_buffer_clear, hirari_midi_buffer_create, hirari_midi_buffer_destroy,
    hirari_midi_buffer_event_count, hirari_midi_buffer_event_data,
    hirari_midi_buffer_remaining_capacity, hirari_midi_buffer_sort_owned,
};
use crate::midi_note_edit::ScheduledMidiNoteFfi;
use crate::midi_orchestration::{
    hirari_midi_apply_articulations_from_state, hirari_midi_process_mpe, MpeState,
};
use crate::midi_playback_clock::{
    hirari_midi_playback_panic_pending, hirari_midi_playback_prepare_block,
    hirari_midi_playback_request_panic,
};
use crate::mixer_telemetry::hirari_mixer_track_peaks;
use crate::preview_sampler::{hirari_preview_sampler_is_active, hirari_preview_sampler_render};
use crate::preview_synth::{hirari_preview_synth_process, hirari_preview_synth_reset};
use crate::routing_engine_core::{
    hirari_routing_engine_capture_feedback, hirari_routing_engine_enter_send_pdc_read,
    hirari_routing_engine_inject_feedback, hirari_routing_engine_leave_send_pdc_read,
    hirari_routing_engine_process_track_fanout, hirari_routing_engine_resolve_process_order,
    hirari_routing_engine_should_process,
};
use crate::routing_fanout::{BusPreFxAdd, RoutingTrackDestination};
use crate::sidechain_manager::hirari_sidechain_engine_publish_source;
use crate::tempo::{
    hirari_tempo_map_beats_to_samples, hirari_tempo_map_get_current_bpm,
    hirari_tempo_map_samples_to_beats,
};
use crate::track_midi_articulation::{
    hirari_track_midi_all_notes_off, hirari_track_midi_note_off, hirari_track_midi_note_on,
    hirari_track_midi_schedule_note_off, hirari_track_midi_timed_outputs, GeneratedMidiEvent,
    TimedKeySwitchOutput,
};
use crate::track_runtime::{
    self, hirari_track_runtime_set_muted, TrackRuntime,
};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// `HirariUnifiedEngine::kMaxTracks`, and the width of one metering payload.
const MAX_TRACKS: usize = 256;
/// `HirariUnifiedEngine::kMaxTrackMap`; the width of the id-indexed track map.
const MAX_TRACK_MAP: usize = 2_048;
/// `HirariUnifiedEngine::TelemetryData::spectrum` bins.
const SPECTRUM_BINS: usize = 512;
/// `HirariUnifiedEngine::kMaxAudioBlockSize`; the click scratch holds one block.
const MAX_BLOCK_FRAMES: usize = 16_384;
/// `BusTrack::kMaxSamples`, the cap on one bus stage and on Send PDC scratch.
const MAX_BUS_FRAMES: usize = 8_192;
/// `RoutingEngine::kMaxNodes`, on top of the tracks a graph order can insert.
const MAX_GRAPH_NODES: usize = MAX_TRACKS + 128;
/// `MidiBuffer::kMaxEventsPerBlock`, the cap one track's MIDI input can hold.
const MAX_MIDI_EVENTS: usize = 1_024;
/// `Track`'s generated-event scratch, the cap on one note's expansion.
const MAX_GENERATED_EVENTS: usize = 128;
/// The preview fallback table's MIDI channels and pitches.
const FALLBACK_CHANNELS: usize = 16;
const FALLBACK_PITCHES: usize = 128;

/// Mirrors `HirariUnifiedEngine::TelemetryData` field for field. The audio
/// callback writes only `count`, the two peak arrays and `version`; every other
/// field stays at its published default, exactly as it did in C++.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariEngineTelemetry {
    pub peaks_left: [f32; MAX_TRACKS],
    pub peaks_right: [f32; MAX_TRACKS],
    pub count: u32,
    pub spectrum: [f32; SPECTRUM_BINS],
    pub correlation: f32,
    pub dsp_load: f32,
    pub active_voices: u32,
    pub forensic_audit_hash: u64,
    pub version: u64,
}

impl Default for HirariEngineTelemetry {
    fn default() -> Self {
        Self {
            peaks_left: [0.0; MAX_TRACKS],
            peaks_right: [0.0; MAX_TRACKS],
            count: 0,
            spectrum: [0.0; SPECTRUM_BINS],
            correlation: 0.0,
            dsp_load: 0.0,
            active_voices: 0,
            forensic_audit_hash: 0,
            version: 0,
        }
    }
}

/// The track storages [`HirariEngineBlockOps::storage`] reaches, mirroring
/// `HirariEngineTrackStorageKind`.
pub const TRACK_RUNTIME: u32 = 0;
pub const TRACK_STRIP: u32 = 1;
pub const TRACK_ARTICULATION: u32 = 2;
pub const TRACK_PLUGIN_MIDI: u32 = 3;
pub const TRACK_BUS_STATE: u32 = 4;

/// `Track::Type`, so the block decides a track's role from the same byte the
/// C++ model stores.
const TRACK_TYPE_MIDI: u8 = 1;
const TRACK_TYPE_INSTRUMENT: u8 = 2;
const TRACK_TYPE_BUS: u8 = 3;

/// The per-track operations the engine block needs from C++. Every read the
/// block makes on a track is now a Rust call on a handle it supplies, so what
/// stays here is the handle selection, the track's role and the control-side
/// edit protocol.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariEngineBlockOps {
    /// The track's own storage selected by one of the `TRACK_*` kinds, or null
    /// when the track has none of that kind.
    pub storage: Option<unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void>,
    /// The track's `Type` as a byte.
    pub track_type: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
    /// Drops the frozen render before a store write, as a control edit does.
    pub begin_edit: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Bumps the layout revision after a store write.
    pub end_edit: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Captures the fader and pan of one armed recorder.
    pub record: Option<unsafe extern "C" fn(*mut c_void, u64)>,
}

/// Every handle the block borrows from the engine, bound once at creation. The
/// engine owns each of them for at least as long as the returned state.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariEngineBlockHandles {
    pub routing: *mut c_void,
    pub track_table: *mut c_void,
    pub bounce: *mut c_void,
    pub input_blocks: *mut c_void,
    pub control_room: *mut c_void,
    pub mixer_telemetry: *mut c_void,
    pub metronome: *mut c_void,
    pub master_limiter: *mut c_void,
    pub meter: *mut c_void,
    pub tempo: *mut c_void,
    pub bus_system: *mut c_void,
    pub sidechain: *mut c_void,
    pub rings: *mut c_void,
    pub midi_clock: *mut c_void,
    pub midi_snapshots: *mut c_void,
    pub preview_synth: *mut c_void,
    pub preview_sampler: *mut c_void,
    pub mpe: *mut c_void,
    pub macro_control: *mut c_void,
}

/// The graph eligibility the whole block shares: an offline target isolates one
/// stem, otherwise a solo anywhere mutes every track that does not carry it.
#[derive(Clone, Copy)]
struct Gating {
    offline_target_active: bool,
    offline_target_id: u32,
    any_solo: bool,
}

/// Routes one Send edge into a bus's pre-FX stage. The destination table
/// carries the bus handle Rust already owns, so the fanout kernel needs no C++
/// callback of its own.
unsafe extern "C" fn add_bus_pre_fx(
    context: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
    gain: f32,
) -> bool {
    if context.is_null() || frames == 0 || frames > MAX_BUS_FRAMES as u32 {
        return false;
    }
    unsafe { hirari_bus_audio_accumulate(context, left, right, frames, gain) }
}

/// The graph window the callback is walking, exactly as the engine's head
/// resolved it. A hot-swap can publish a different table mid-block, so the tail
/// never re-reads it.
#[derive(Clone, Copy, Default)]
struct View {
    tracks: *const *mut c_void,
    map: *const *mut c_void,
    count: u32,
    /// The device generation this snapshot was published for.
    generation: u64,
}

impl View {
    fn active(self) -> &'static [*mut c_void] {
        if self.tracks.is_null() {
            return &[];
        }
        unsafe {
            slice::from_raw_parts(
                self.tracks,
                (self.count as usize).min(MAX_TRACKS),
            )
        }
    }

    /// The id-indexed track map the published snapshot carries, or null when the
    /// window was handed over without one.
    fn track_id(self, id: u32) -> *mut c_void {
        if self.map.is_null() || id as usize >= MAX_TRACK_MAP {
            return ptr::null_mut();
        }
        unsafe { *self.map.add(id as usize) }
    }
}

pub struct EngineBlockState {
    ops: HirariEngineBlockOps,
    routing: *mut c_void,
    track_table: *mut c_void,
    bounce: *mut c_void,
    input_blocks: *mut c_void,
    control_room: *mut c_void,
    mixer_telemetry: *mut c_void,
    metronome: *mut c_void,
    master_limiter: *mut c_void,
    meter: *mut c_void,
    tempo: *mut c_void,
    bus_system: *mut c_void,
    sidechain: *mut c_void,
    rings: *mut c_void,
    midi_clock: *mut c_void,
    midi_snapshots: *mut c_void,
    preview_synth: *mut c_void,
    preview_sampler: *mut c_void,
    mpe: *mut c_void,
    macro_control: *mut c_void,
    playhead: AtomicU64,
    playing: AtomicBool,
    loop_enabled: AtomicBool,
    cycle_start: AtomicU64,
    cycle_end: AtomicU64,
    test_tone: AtomicBool,
    generation: AtomicU64,
    config_transition: AtomicBool,
    range_overflow: AtomicBool,
    preview_voices_need_reset: AtomicBool,
    monitor_offset: AtomicU32,
    talkback_offset: AtomicU32,
    /// The callback's own event collection between timeline scheduling and the
    /// preview voices; a track's input is its own Rust MIDI buffer.
    block_midi: *mut c_void,
    /// Held preview notes per MIDI channel and pitch, so a Note Off released by
    /// a track that no longer exists still closes the fallback voice.
    fallback_notes: *mut [[u32; FALLBACK_PITCHES]; FALLBACK_CHANNELS],
    telemetry: *mut HirariEngineTelemetry,
    telemetry_active: AtomicU32,
    telemetry_lock: AtomicBool,
    telemetry_ids: *mut u32,
    telemetry_left: *mut *const f32,
    telemetry_right: *mut *const f32,
    click: *mut [[f32; MAX_BLOCK_FRAMES]; 2],
    send_pdc: *mut [[f32; MAX_BUS_FRAMES]; 2],
    destinations: *mut [RoutingTrackDestination; MAX_TRACKS],
    graph_ids: *mut [u32; MAX_TRACKS],
    process_order: *mut [u32; MAX_GRAPH_NODES],
}

// The handles belong to one engine, and only its serialized audio callback and
// control thread touch this state.
unsafe impl Send for EngineBlockState {}
unsafe impl Sync for EngineBlockState {}

fn block(state: *const c_void) -> Option<&'static EngineBlockState> {
    unsafe { state.cast::<EngineBlockState>().as_ref() }
}

impl EngineBlockState {
    fn storage(&self, track: *mut c_void, kind: u32) -> *mut c_void {
        let Some(op) = self.ops.storage else {
            return ptr::null_mut();
        };
        unsafe { op(track, kind) }
    }

    fn track_type(&self, track: *mut c_void) -> u8 {
        let Some(op) = self.ops.track_type else {
            return 0;
        };
        unsafe { op(track) }
    }

    fn runtime(&self, track: *mut c_void) -> Option<&'static TrackRuntime> {
        track_runtime::runtime(self.storage(track, TRACK_RUNTIME))
    }

    fn bus_state(&self, track: *mut c_void) -> *mut c_void {
        if self.track_type(track) != TRACK_TYPE_BUS {
            return ptr::null_mut();
        }
        self.storage(track, TRACK_BUS_STATE)
    }

    fn solo(&self, track: *mut c_void) -> bool {
        unsafe { hirari_channel_strip_is_solo(self.storage(track, TRACK_STRIP)) }
    }

    fn record(&self, track: *mut c_void, playhead: u64) {
        if let Some(op) = self.ops.record {
            unsafe { op(track, playhead) };
        }
    }

    fn click(&self, side: usize) -> *mut f32 {
        let scratch = unsafe { &mut *self.click };
        scratch[side].as_mut_ptr()
    }

    /// The stereo master window the device callback owns, or null when the
    /// declared block does not fit the buffer holding it.
    fn window(
        &self,
        output: *mut c_void,
        off: u32,
        frames: u32,
    ) -> Option<(*mut f32, *mut f32, *const *mut f32, u32)> {
        let (planes, count, capacity) = track_runtime::channels_of(output);
        if planes.is_null() || count < 2 || capacity < off.checked_add(frames)? {
            return None;
        }
        let planes = unsafe { slice::from_raw_parts(planes, count as usize) };
        if planes[0].is_null() || planes[1].is_null() {
            return None;
        }
        let offset = off as usize;
        Some((
            unsafe { planes[0].add(offset) },
            unsafe { planes[1].add(offset) },
            planes.as_ptr(),
            count,
        ))
    }

    // ---- the block head: what the callback reads from a Track ----------

    /// The graph window the callback traverses: the published snapshot the Rust
    /// track table double-buffers, read without a lock exactly as the C++
    /// `trackView()` did. Publication only fills the buffer this is not reading.
    fn view(&self) -> View {
        let mut published = HirariTrackSnapshot::default();
        unsafe { hirari_engine_track_snapshot(self.track_table, &mut published) };
        View {
            tracks: published.active_tracks,
            map: published.track_map,
            count: published.track_count,
            generation: published.audio_config_generation,
        }
    }

    /// Silences the window the device callback owns, the whole buffer's
    /// channel set, exactly as `output.clear(off, frames)` did.
    fn clear_window(planes: *const *mut f32, count: u32, off: usize, frames: usize) {
        if planes.is_null() || frames == 0 {
            return;
        }
        for index in 0..count as usize {
            let plane = unsafe { *planes.add(index) };
            if !plane.is_null() {
                unsafe { ptr::write_bytes(plane.add(off), 0, frames) };
            }
        }
    }

    fn midi_input(&self, track: *mut c_void) -> bool {
        matches!(self.track_type(track), TRACK_TYPE_MIDI | TRACK_TYPE_INSTRUMENT)
    }

    fn articulation(&self, track: *mut c_void) -> *mut c_void {
        self.storage(track, TRACK_ARTICULATION)
    }

    fn plugin_midi(&self, track: *mut c_void) -> *mut c_void {
        self.storage(track, TRACK_PLUGIN_MIDI)
    }

    /// One slot of the held preview-voice table, or null when the pair is out of
    /// range for a malformed scheduled note.
    fn held_note(&self, channel: u8, pitch: u8) -> *mut u32 {
        let channel = usize::from(channel);
        let pitch = usize::from(pitch);
        if channel >= FALLBACK_CHANNELS || pitch >= FALLBACK_PITCHES {
            return ptr::null_mut();
        }
        let table = unsafe { &mut *self.fallback_notes };
        unsafe { table[channel].as_mut_ptr().add(pitch) }
    }

    /// Copies one note's generated events into the track's own input buffer, the
    /// same expansion the C++ wrapper performed for every MIDI input.
    fn push_generated(&self, midi: *mut c_void, events: &[GeneratedMidiEvent]) {
        if midi.is_null() {
            return;
        }
        for event in events {
            unsafe {
                hirari_midi_buffer_add(
                    midi,
                    event.sample_offset,
                    event.data.as_ptr(),
                    u32::from(event.size),
                    event.articulation_id,
                )
            };
        }
    }

    fn clear_midi(&self, track: *mut c_void) {
        let midi = self.plugin_midi(track);
        if !midi.is_null() {
            unsafe { hirari_midi_buffer_clear(midi) };
        }
    }

    fn note_on(
        &self,
        track: *mut c_void,
        channel: u8,
        pitch: u8,
        velocity: u8,
        offset: u64,
        articulation: u8,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_GENERATED_EVENTS];
        let count = unsafe {
            hirari_track_midi_note_on(
                state,
                channel,
                pitch,
                velocity,
                offset,
                articulation,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        self.push_generated(self.plugin_midi(track), &events[..count]);
    }

    fn note_off(
        &self,
        track: *mut c_void,
        channel: u8,
        pitch: u8,
        offset: u64,
        articulation: u8,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_GENERATED_EVENTS];
        let count = unsafe {
            hirari_track_midi_note_off(
                state,
                channel,
                pitch,
                offset,
                articulation,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        self.push_generated(self.plugin_midi(track), &events[..count]);
    }

    fn all_notes_off(&self, track: *mut c_void, offset: u64) {
        let state = self.articulation(track);
        let midi = self.plugin_midi(track);
        if state.is_null() || midi.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_MIDI_EVENTS];
        let capacity = unsafe { hirari_midi_buffer_remaining_capacity(midi) }.min(events.len());
        let count = unsafe {
            hirari_track_midi_all_notes_off(
                state,
                offset,
                if capacity == 0 {
                    ptr::null_mut()
                } else {
                    events.as_mut_ptr()
                },
                capacity,
            )
        };
        self.push_generated(midi, &events[..count]);
    }

    /// Emits one articulation's timed key-switch note-offs, mapped through the
    /// engine's tempo map at the block's sample rate. `transition` selects the
    /// pre-Note-On set, which the instrument must release before the key switch
    /// itself sounds.
    fn schedule_key_switches(
        &self,
        track: *mut c_void,
        articulation: u8,
        transition: bool,
        start_beat: f64,
        sample_rate: f64,
        offset: u64,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut outputs = [TimedKeySwitchOutput::default(); 16];
        let count = unsafe {
            hirari_track_midi_timed_outputs(
                state,
                articulation,
                transition,
                outputs.as_mut_ptr(),
                outputs.len(),
            )
        };
        let midi = self.plugin_midi(track);
        for output in &outputs[..count] {
            let end_beat = start_beat + f64::from(output.length_ticks) / 960.0;
            let end_sample =
                unsafe { hirari_tempo_map_beats_to_samples(self.tempo, end_beat, sample_rate) };
            if !unsafe {
                hirari_track_midi_schedule_note_off(state, output.channel, output.pitch, end_sample)
            } {
                // An overloaded schedule must still release the physical key.
                if !midi.is_null() {
                    unsafe {
                        hirari_midi_buffer_add_note_off(midi, output.channel + 1, output.pitch, offset)
                    };
                }
            }
        }
    }

    /// Releases every voice the callback held: the built-in synth, the fallback
    /// counts and each instrument's held keys.
    fn reset_preview_voices(&self, view: View) {
        unsafe { hirari_preview_synth_reset(self.preview_synth) };
        let table = unsafe { &mut *self.fallback_notes };
        for channel in table.iter_mut() {
            channel.fill(0);
        }
        for &track in view.active() {
            if !track.is_null() && self.midi_input(track) {
                self.all_notes_off(track, 0);
            }
        }
    }

    /// Applies the commands that arrived while the callback was rendering. A
    /// command that waited in the queue through a device rebuild never applies
    /// to a different audio configuration.
    fn drain_commands(&self, generation: u64, view: View) {
        let rings = self.rings.cast::<EngineRings>();
        let mut record = CommandRecord::default();
        while unsafe { hirari_engine_command_ring_pop(rings, &mut record) } {
            if record.expected_audio_generation != generation {
                continue;
            }
            let track = view.track_id(record.tid);
            if track.is_null() {
                continue;
            }
            let mut value = 0.0_f32;
            let command = unsafe { hirari_engine_audio_command(record.type_, record.val, &mut value) };
            if matches!(
                command,
                AUDIO_COMMAND_VOLUME | AUDIO_COMMAND_PAN | AUDIO_COMMAND_MUTE | AUDIO_COMMAND_SOLO
            ) {
                self.apply_scalar(track, command, value);
            }
        }
    }

    /// Writes one drained command to the track's own storage under the control
    /// edit protocol the C++ setter applied around it: the fader, pan and solo
    /// live on the channel strip, mute on the runtime.
    fn apply_scalar(&self, track: *mut c_void, command: u32, value: f32) {
        let Some(op) = self.ops.begin_edit else {
            return;
        };
        unsafe { op(track) };
        let strip = self.storage(track, TRACK_STRIP);
        match command {
            AUDIO_COMMAND_VOLUME => unsafe { hirari_channel_strip_set_gain(strip, value) },
            AUDIO_COMMAND_PAN => unsafe { hirari_channel_strip_set_pan(strip, value) },
            AUDIO_COMMAND_MUTE => unsafe {
                hirari_track_runtime_set_muted(
                    self.storage(track, TRACK_RUNTIME),
                    u8::from(value >= 0.5),
                )
            },
            _ => unsafe { hirari_channel_strip_set_solo(strip, value >= 0.5) },
        }
        if let Some(op) = self.ops.end_edit {
            unsafe { op(track) };
        }
    }

    /// Hands the timeline's scheduled notes to the graph: a note addressed to a
    /// live track enters that track's input, an orphan note feeds the built-in
    /// preview, and a transport discontinuity releases every held voice.
    unsafe fn stage_midi(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
        playback_active: bool,
        view: View,
    ) {
        let mut stage = MidiStage {
            state: self,
            view,
            sample_rate,
        };
        let context = ptr::from_mut(&mut stage).cast();
        unsafe {
            hirari_midi_playback_prepare_block(
                self.midi_clock,
                self.midi_snapshots,
                playhead,
                frames,
                u8::from(playback_active),
                context,
                Some(stage_panic),
                Some(stage_note),
            )
        };
        // Preview dispatch reads the block in timestamp order, same-sample events
        // in the order they were scheduled.
        unsafe { hirari_midi_buffer_sort_owned(self.block_midi) };
    }

    /// Runs the native MIDI policy over the staged block, then the built-in
    /// voices that make playback audible without a plug-in.
    unsafe fn render_preview(
        &self,
        left: *mut f32,
        right: *mut f32,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
    ) {
        let events = unsafe { hirari_midi_buffer_event_data(self.block_midi) };
        let count = unsafe { hirari_midi_buffer_event_count(self.block_midi) };
        let mpe = self.mpe.cast::<MpeState>();
        unsafe {
            hirari_midi_process_mpe(mpe, events, count);
            hirari_midi_apply_articulations_from_state(mpe.cast_const(), events, count);
        }
        let rate = sample_rate.max(1.0);
        unsafe {
            hirari_preview_synth_process(
                self.preview_synth,
                events,
                count,
                left,
                right,
                frames,
                playhead,
                rate,
                self.macro_control,
            );
            hirari_preview_sampler_render(self.preview_sampler, rate, left, right, frames);
        }
        if self.test_tone.load(Ordering::Relaxed) {
            unsafe { hirari_audio_test_tone_render(left, right, frames, playhead, sample_rate) };
        }
    }
}

/// The one-block context the playback clock's two callbacks need.
#[derive(Clone, Copy)]
struct MidiStage {
    state: *const EngineBlockState,
    view: View,
    sample_rate: f64,
}

unsafe extern "C" fn stage_panic(opaque: *mut c_void) {
    let Some(stage) = (unsafe { opaque.cast::<MidiStage>().as_ref() }) else {
        return;
    };
    unsafe { (*stage.state).reset_preview_voices(stage.view) };
}

unsafe extern "C" fn stage_note(
    opaque: *mut c_void,
    raw: *const ScheduledMidiNoteFfi,
    offset: u64,
    note_off: u8,
) {
    let (Some(stage), Some(note)) = (
        unsafe { opaque.cast::<MidiStage>().as_ref() },
        unsafe { raw.as_ref() },
    ) else {
        return;
    };
    let state = unsafe { &*stage.state };
    let channel = note.midi_channel + 1;
    let target = stage.view.track_id(note.track_id);
    if note_off != 0 {
        if !target.is_null() {
            state.note_off(target, channel, note.pitch, offset, note.articulation_id);
        }
        let held = state.held_note(note.midi_channel, note.pitch);
        if !held.is_null() && unsafe { *held } > 0
            && unsafe { hirari_midi_buffer_remaining_capacity(state.block_midi) } > 0
        {
            unsafe { hirari_midi_buffer_add_note_off(state.block_midi, channel, note.pitch, offset) };
            unsafe { *held -= 1 };
        }
        return;
    }
    if !target.is_null() {
        let start_beat = unsafe {
            hirari_tempo_map_samples_to_beats(state.tempo, note.start_sample, stage.sample_rate)
        };
        state.schedule_key_switches(
            target,
            note.articulation_id,
            true,
            start_beat,
            stage.sample_rate,
            offset,
        );
        state.note_on(
            target,
            channel,
            note.pitch,
            note.velocity,
            offset,
            note.articulation_id,
        );
        state.schedule_key_switches(
            target,
            note.articulation_id,
            false,
            start_beat,
            stage.sample_rate,
            offset,
        );
    } else if unsafe { hirari_midi_buffer_remaining_capacity(state.block_midi) } > 0 {
        unsafe {
            hirari_midi_buffer_add_note_on(
                state.block_midi,
                channel,
                note.pitch,
                note.velocity,
                offset,
                0,
            )
        };
        let held = state.held_note(note.midi_channel, note.pitch);
        if !held.is_null() {
            unsafe { *held += 1 };
        }
    }
}

impl EngineBlockState {
    /// Feeds each track's post-graph block into its one-block delayed routing
    /// edges, in published graph order.
    unsafe fn capture_feedback(&self, frames: u32, view: View) {        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            unsafe {
                hirari_routing_engine_capture_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Publishes per-track linear peaks through the double buffer. The payload
    /// stays strictly aligned with the active track order, so a track that
    /// cannot hold the block keeps its slot at silence.
    unsafe fn publish_peaks(&self, frames: u32, view: View) {
        let flag = self.telemetry_lock.swap(true, Ordering::Acquire);
        debug_assert!(!flag, "the metering handoff must not re-enter the callback");
        let next = ((self.telemetry_active.load(Ordering::Relaxed) as usize) ^ 1) as isize;
        let telemetry = unsafe { &mut *self.telemetry.offset(next) };
        telemetry.count = view.count.min(MAX_TRACKS as u32);
        let slots = telemetry.count as usize;
        let ids = unsafe { slice::from_raw_parts_mut(self.telemetry_ids, slots) };
        let lefts = unsafe { slice::from_raw_parts_mut(self.telemetry_left, slots) };
        let rights = unsafe { slice::from_raw_parts_mut(self.telemetry_right, slots) };
        ids.fill(0);
        lefts.fill(std::ptr::null());
        rights.fill(std::ptr::null());
        for slot in 0..slots {
            let Some(&track) = view.active().get(slot) else {
                continue;
            };
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            ids[slot] = runtime.track_id();
            lefts[slot] = tap.left;
            rights[slot] = tap.right;
        }
        unsafe {
            hirari_mixer_track_peaks(
                self.mixer_telemetry,
                ids.as_ptr(),
                lefts.as_ptr(),
                rights.as_ptr(),
                telemetry.count,
                frames,
                telemetry.peaks_left.as_mut_ptr(),
                telemetry.peaks_right.as_mut_ptr(),
            )
        };
        telemetry.version += 1;
        self.telemetry_active.store(next as u32, Ordering::Release);
        self.telemetry_lock.store(false, Ordering::Release);
    }

    /// The metronome belongs to the rendered monitor signal, not to a track
    /// node, so it is injected here and left for the master limiter to trim.
    /// Returns whether the click is part of this block.
    unsafe fn render_metronome(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
        playing: bool,
    ) -> bool {
        let block = frames as usize;
        let scratch = unsafe { &mut *self.click };
        scratch[0][..block].fill(0.0);
        scratch[1][..block].fill(0.0);
        if !playing || !unsafe { hirari_metronome_is_enabled(self.metronome) } {
            return false;
        }
        unsafe {
            hirari_metronome_process(
                self.metronome,
                self.click(0),
                self.click(1),
                frames,
                playhead,
                sample_rate,
                hirari_tempo_map_get_current_bpm(self.tempo) as f64,
            )
        };
        true
    }

    fn find_track(&self, track_id: u32, view: View) -> Option<*mut c_void> {
        view.active()
            .iter()
            .copied()
            .filter(|&track| !track.is_null())
            .find(|&track| {
                self.runtime(track)
                    .is_some_and(|runtime| runtime.track_id() == track_id)
            })
    }

    /// Control Room is strictly device monitoring: speaker trim and DIM are
    /// never baked into an offline master or stem render.
    unsafe fn render_control_room(
        &self,
        planes: *const *mut f32,
        channel_count: u32,
        off: u32,
        frames: u32,
        talkback_offset: u32,
        view: View,
    ) {
        if hirari_bounce_offline_active(self.bounce) != 0 {
            return;
        }
        let planes = unsafe { slice::from_raw_parts(planes, channel_count as usize) };
        let offset = off as usize;
        let active_cue_bus = unsafe { hirari_control_room_active_cue_bus(self.control_room) };
        let mut cue = unsafe { hirari_control_room_active_cue_output(self.control_room) };
        let dedicated_pair = cue >= 2;
        let cue_available = cue + 1 < channel_count;
        if !cue_available {
            cue = 0;
        }
        let index = cue as usize;
        if active_cue_bus != 0 && (!dedicated_pair || cue_available) {
            if let Some(bus) = self.find_track(active_cue_bus, view) {
                let bus_state = self.bus_state(bus);
                if let Some(runtime) = self.runtime(bus).filter(|_| !bus_state.is_null()) {
                    if let Some(tap) = runtime.work(frames) {
                        unsafe {
                            hirari_control_room_mix_cue(
                                tap.left,
                                tap.right,
                                self.click(0) as *const f32,
                                self.click(1) as *const f32,
                                planes[index].add(offset),
                                planes[index + 1].add(offset),
                                frames,
                                hirari_control_room_active_cue_gain(self.control_room),
                                hirari_control_room_active_cue_click(self.control_room),
                            )
                        };
                    }
                }
            }
        }
        let talkback =
            unsafe { hirari_input_blocks_take_talkback(self.input_blocks, talkback_offset, frames) };
        if dedicated_pair {
            // Keep the Main control-room monitor live while a selected Cue feeds
            // its own output pair; talkback stays on that pair so it cannot leak
            // into the control-room mix.
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[0].add(offset),
                    planes[1].add(offset),
                    std::ptr::null(),
                    frames,
                );
            }
        }
        if !dedicated_pair || cue_available {
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[index].add(offset),
                    planes[index + 1].add(offset),
                    talkback,
                    frames,
                )
            };
        }
    }

    /// Graph eligibility for one track, read through the routing kernel the
    /// engine's solo and offline-target state feeds.
    fn should_process(&self, track: *mut c_void, gating: &Gating) -> bool {
        let Some(runtime) = self.runtime(track) else {
            return false;
        };
        unsafe {
            hirari_routing_engine_should_process(
                self.routing,
                runtime.track_id(),
                gating.offline_target_active,
                gating.offline_target_id,
                gating.any_solo,
                self.solo(track),
            )
        }
    }

    /// Publishes one block's tempo and playback state, then empties every work
    /// buffer so upstream tracks accumulate into a known-zero tap.
    unsafe fn stage_tracks(&self, frames: u32, view: View, playing: bool) {
        let bpm = unsafe { hirari_tempo_map_get_current_bpm(self.tempo) } as f64;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self
                .runtime(track)
                .filter(|runtime| runtime.work(frames).is_some())
            {
                runtime.stage(frames, bpm, playing);
            }
        }
    }

    /// Feedback edges are explicitly delayed by one engine block. They are
    /// injected before any graph node runs, so a cycle cannot recurse on the
    /// current block or allocate on the audio thread.
    unsafe fn inject_feedback(&self, frames: u32, view: View) {
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            unsafe {
                hirari_routing_engine_inject_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Feeds the live driver input into the selected monitor track before graph
    /// processing. The staged planes were copied by the driver callback, so this
    /// path performs no allocation or locking.
    unsafe fn feed_monitor_input(&self, frames: u32, monitor_offset: u32, view: View) {
        let monitor =
            unsafe { hirari_input_blocks_take_monitor(self.input_blocks, monitor_offset, frames) };
        if monitor.left.is_null() {
            return;
        }
        let Some(track) = self.find_track(monitor.track_id, view) else {
            return;
        };
        if let Some(runtime) = self.runtime(track) {
            runtime.add_input(monitor.left.cast_const(), monitor.right.cast_const(), frames);
        }
    }

    /// Publishes the destination table one fanout call reads, in graph order.
    /// The scratch it lives in is bound once, so no block allocates.
    unsafe fn publish_destinations(&self, frames: u32, view: View, gating: &Gating) -> usize {
        let table = unsafe { &mut *self.destinations };
        let mut count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let (channels, channel_count) = runtime.work_channels(frames);
            if channels.is_null() {
                continue;
            }
            let bus_state = self.bus_state(track);
            table[count] = RoutingTrackDestination {
                id: runtime.track_id(),
                can_process: u8::from(self.should_process(track, gating)),
                is_bus: u8::from(!bus_state.is_null()),
                channel_count,
                work_channels: channels,
                bus_context: bus_state,
            };
            count += 1;
        }
        count
    }

    /// Applies one processed track's normal routes and parallel bus sends.
    #[allow(clippy::too_many_arguments)]
    unsafe fn route_to_next_stages(
        &self,
        runtime: &TrackRuntime,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
        frames: u32,
    ) {
        // Cue buses are isolated monitor sources. Their Send inputs and insert
        // chain run normally, but their signal is selected only at the Control
        // Room boundary, never summed into the Main mix.
        if !gating.offline_target_active
            && unsafe { hirari_control_room_is_cue_bus(self.control_room, runtime.track_id()) }
        {
            return;
        }
        if gating.offline_target_active
            && gating.offline_target_id != 0
            && runtime.track_id() == gating.offline_target_id
        {
            let tap = if hirari_bounce_pre_fader(self.bounce) != 0 {
                runtime.pre_fader(frames)
            } else if hirari_bounce_include_inserts(self.bounce) == 0 {
                runtime.pre_insert(frames)
            } else {
                runtime.work(frames)
            };
            if let Some(tap) = tap {
                unsafe {
                    hirari_audio_add_sanitized_stereo(
                        master_left,
                        master_right,
                        tap.left.cast_const(),
                        tap.right.cast_const(),
                        frames,
                    )
                };
            }
            return;
        }
        let send_pre = runtime.send_pre(frames);
        let send_post = runtime.send_post(frames);
        let (send_pre_left, send_pre_right) = send_pre.map_or((ptr::null(), ptr::null()), |tap| {
            (tap.left.cast_const(), tap.right.cast_const())
        });
        let (send_post_left, send_post_right) =
            send_post.map_or((ptr::null(), ptr::null()), |tap| {
                (tap.left.cast_const(), tap.right.cast_const())
            });
        let (source_channels, source_channel_count) = runtime.work_channels(frames);
        // One state-owned scratch pair is reused for each sequential Send edge;
        // delay histories live in the routing kernel and nothing here allocates.
        let scratch = unsafe { &mut *self.send_pdc };
        unsafe {
            hirari_routing_engine_process_track_fanout(
                self.routing,
                runtime.track_id(),
                source_channels.cast(),
                source_channel_count,
                send_pre_left,
                send_pre_right,
                send_post_left,
                send_post_right,
                runtime.delay_samples(),
                destinations.as_ptr(),
                destinations.len(),
                master_left,
                master_right,
                scratch[0].as_mut_ptr(),
                scratch[1].as_mut_ptr(),
                frames,
                MAX_BUS_FRAMES as u32,
                Some(add_bus_pre_fx as BusPreFxAdd),
            );
        }
    }

    /// Walks the graph in resolved process order: upstream tracks have already
    /// accumulated into this node's work buffer, so the node renders what it was
    /// given, publishes itself as a sidechain source and fans out.
    unsafe fn walk_graph(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: u32,
        generation: u64,
        view: View,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
    ) {
        let ids = unsafe { &mut *self.graph_ids };
        let mut id_count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self.runtime(track) {
                ids[id_count] = runtime.track_id();
                id_count += 1;
            }
        }
        let order = unsafe { &mut *self.process_order };
        let mut order_count = 0u32;
        if !unsafe {
            hirari_routing_engine_resolve_process_order(
                self.routing,
                ids.as_ptr(),
                id_count,
                order.as_mut_ptr(),
                order.len(),
                &mut order_count,
            )
        } {
            // A graph the topology cannot order still renders in published
            // order rather than dropping every track's block.
            order_count = id_count as u32;
            order[..id_count].copy_from_slice(&ids[..id_count]);
        }
        for index in 0..order_count as usize {
            let Some(track) = self.find_track(order[index], view) else {
                continue;
            };
            if !self.should_process(track, gating) {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(work) = runtime.work(frames) else {
                continue;
            };
            let bus_state = self.bus_state(track);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe { hirari_bus_audio_read(bus_state, work.left, work.right, frames, false) };
            }
            // The graph publishes the node it just rendered whether or not the
            // chain ran: a muted or frozen track still owns its bus stage and
            // still feeds downstream sidechain and Send edges.
            let _ = runtime.process_accumulated(frames, playhead);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe {
                    hirari_bus_audio_replace_post(
                        bus_state,
                        work.left.cast_const(),
                        work.right.cast_const(),
                        frames,
                    )
                };
            }
            let (pre_left, pre_right) = runtime
                .pre_insert(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            let (post_left, post_right) = runtime
                .pre_fader(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            unsafe {
                hirari_sidechain_engine_publish_source(
                    self.sidechain,
                    runtime.track_id(),
                    pre_left,
                    pre_right,
                    post_left,
                    post_right,
                    work.left.cast_const(),
                    work.right.cast_const(),
                    frames,
                    sample_rate,
                    generation,
                );
            }
            self.route_to_next_stages(
                runtime,
                gating,
                master_left,
                master_right,
                destinations,
                frames,
            );
        }
    }
}

/// Runs the admission head of one engine block.
///
/// The gates that decide whether a block renders at all, the cycle wrap, the
/// snapshot-generation check, the command drain, the timeline MIDI staging into
/// every track and the built-in preview voices are all here. `frames_out`
/// receives the block length the rest of the callback must use -- the head
/// clamps it to the buffer it was handed -- and the return value is zero when
/// the head already silenced the window, so the caller neither routes nor
/// finishes.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`], `output` is that engine's
/// block storage, `sample_rate` is the device's, `shutdown` is the engine's
/// shutdown flag, and `frames_out` is writable when non-null.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_engine_block_begin(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    sample_rate: f64,
    shutdown: u8,
    frames_out: *mut u32,
) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    let (planes, channel_count, capacity) = track_runtime::channels_of(output);
    let capacity = capacity as usize;
    let off = off as usize;
    let frames = frames as usize;
    // The head silences the range the caller asked for, clamped to what the
    // buffer actually holds, before it reports the block as handled.
    let in_range = channel_count >= 2 && off < capacity;
    let prefix = frames.min(capacity.saturating_sub(off));
    if frames == 0 || !in_range {
        if frames != 0 && in_range {
            EngineBlockState::clear_window(planes, channel_count, off, prefix);
        }
        return 0;
    }
    if frames > MAX_BLOCK_FRAMES {
        EngineBlockState::clear_window(planes, channel_count, off, prefix);
        return 0;
    }
    let frames = prefix as u32;
    let offline_thread = hirari_bounce_offline_thread() != 0;
    if state.config_transition.load(Ordering::Acquire) && !offline_thread {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if hirari_bounce_offline_blocks(state.bounce, hirari_callback_thread_token()) != 0 {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if shutdown != 0 {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    // Hot-swap quiesce: never sleep or spin here. The next block sees the
    // request again while the structural transaction is still active.
    if hirari_hot_swap_default_quiesce_requested() {
        hirari_hot_swap_default_acknowledge();
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    hirari_forensic_scratchpad_reset();
    let mut playhead = state.playhead.load(Ordering::Relaxed);
    let cycle_start = state.cycle_start.load(Ordering::Acquire);
    let cycle_end = state.cycle_end.load(Ordering::Acquire);
    if state.loop_enabled.load(Ordering::Acquire) && cycle_end > cycle_start && playhead >= cycle_end
    {
        playhead = cycle_start;
        state.playhead.store(cycle_start, Ordering::Relaxed);
    }
    // The per-chunk read offset travels with the window this callback renders:
    // the live driver input is addressed at the same offset as the master.
    state.monitor_offset.store(off as u32, Ordering::Relaxed);
    state.talkback_offset.store(off as u32, Ordering::Relaxed);
    let view = state.view();
    // Structural publication is double-buffered. Never process a state prepared
    // for a different device format; the next publication carries this one.
    if view.generation != state.generation.load(Ordering::Acquire) && !offline_thread {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    if state
        .preview_voices_need_reset
        .swap(false, Ordering::AcqRel)
    {
        state.reset_preview_voices(view);
    }
    // A track's MIDI input is its own buffer: it only ever holds the events
    // addressed to it during this block, so the walk cannot replay the last.
    for &track in view.active() {
        if !track.is_null() {
            state.clear_midi(track);
        }
    }
    state.drain_commands(view.generation, view);
    // Keep the device callback primed while stopped, but do not traverse the
    // full graph unless a preview voice, a panic or an offline render owns the
    // path. Commands drain first so fader, pan, mute and solo edits stay
    // visible immediately while stopped.
    let offline_engaged =
        hirari_bounce_offline_engaged(state.bounce) != 0;
    let playing = state.playing.load(Ordering::Acquire);
    if !offline_engaged
        && !playing
        && !state.test_tone.load(Ordering::Relaxed)
        && !hirari_preview_sampler_is_active(state.preview_sampler)
        && !hirari_midi_playback_panic_pending(state.midi_clock)
    {
        EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
        return 0;
    }
    EngineBlockState::clear_window(planes, channel_count, off, frames as usize);
    unsafe { hirari_midi_buffer_clear(state.block_midi) };
    if playhead > u64::MAX - u64::from(frames) {
        // A saturated sample position is reported, never wrapped around.
        state.range_overflow.store(true, Ordering::Release);
        return 0;
    }
    state.stage_midi(
        frames,
        playhead,
        sample_rate.max(1.0),
        offline_engaged || playing,
        view,
    );
    let Some((left, right, _, _)) = state.window(output, off as u32, frames) else {
        return 0;
    };
    state.render_preview(left, right, frames, playhead, sample_rate);
    if !frames_out.is_null() {
        unsafe { *frames_out = frames };
    }
    1
}

/// Chooses the next engine chunk for a hardware callback and rewinds the
/// playhead when it sits on the cycle end. The low 32 bits of the result hold
/// the frames to process; [`CHUNK_RESTART_PLAYHEAD`] means the caller retries
/// after the rewind this function already applied.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_chunk_plan(state: *mut c_void, remaining: u32) -> u64 {
    let Some(state) = block(state) else {
        return u64::from(remaining);
    };
    let plan = hirari_engine_chunk_plan(
        remaining,
        state.playing.load(Ordering::Acquire),
        state.loop_enabled.load(Ordering::Acquire),
        state.cycle_start.load(Ordering::Acquire),
        state.cycle_end.load(Ordering::Acquire),
        state.playhead.load(Ordering::Relaxed),
    );
    if plan & CHUNK_RESTART_PLAYHEAD != 0 {
        state
            .playhead
            .store(state.cycle_start.load(Ordering::Acquire), Ordering::Relaxed);
    }
    plan
}

/// Runs the post-graph tail of one engine block and advances the playhead the
/// head resolved.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`] and `output` is that
/// engine's block storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_finish(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    sample_rate: f64,
    master_gain: f32,
    auto_record: u8,
) {
    // The click scratch holds one kMaxAudioBlockSize block; the engine's head
    // gate clamps the block length before it reaches the tail.
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((left, right, planes, channel_count)) = state.window(output, off, frames) else {
        return;
    };
    let view = state.view();
    let playhead = state.playhead.load(Ordering::Relaxed);
    let playing = state.playing.load(Ordering::Acquire);
    let talkback_offset = state.talkback_offset.load(Ordering::Relaxed);
    unsafe {
        state.capture_feedback(frames, view);
        state.publish_peaks(frames, view);
        let click = state.render_metronome(frames, playhead, sample_rate, playing);
        hirari_master_output_process(
            left,
            right,
            state.click(0),
            state.click(1),
            frames,
            master_gain,
            click,
        );
        if !state.master_limiter.is_null() {
            hirari_master_limiter_process(state.master_limiter, left, right, frames as usize, true);
        }
        hirari_master_meter_process(
            state.meter,
            left as *const f32,
            right as *const f32,
            frames as usize,
        );
        state.render_control_room(planes, channel_count, off, frames, talkback_offset, view);
        if playing && auto_record > 0 {
            for &track in view.active() {
                if !track.is_null() {
                    state.record(track, playhead);
                }
            }
        }
    }
    if !playing {
        return;
    }
    if playhead > u64::MAX - u64::from(frames) {
        state.range_overflow.store(true, Ordering::Release);
    } else {
        state.playhead.fetch_add(u64::from(frames), Ordering::Relaxed);
    }
}

/// Runs the graph half of one engine block: staging, feedback injection, the
/// live driver input, the routing destination table and the process-order walk
/// that fills the master window and every bus stage.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`] and `output` is that
/// engine's block storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_route(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    sample_rate: f64,
) {
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((master_left, master_right, _, _)) = state.window(output, off, frames) else {
        return;
    };
    let monitor_offset = state.monitor_offset.load(Ordering::Relaxed);
    let generation = state.generation.load(Ordering::Acquire);
    let playing = state.playing.load(Ordering::Acquire);
    let playhead = state.playhead.load(Ordering::Relaxed);
    let view = state.view();
    let gating = Gating {
        offline_target_active: hirari_bounce_target_active(state.bounce) != 0,
        offline_target_id: hirari_bounce_target_id(state.bounce),
        any_solo: view
            .active()
            .iter()
            .copied()
            .filter(|track| !track.is_null())
            .any(|track| state.solo(track)),
    };
    unsafe {
        hirari_bus_system_clear(state.bus_system.cast_const(), frames);
        state.stage_tracks(frames, view, playing);
        state.inject_feedback(frames, view);
        state.feed_monitor_input(frames, monitor_offset, view);
        let destination_count = state.publish_destinations(frames, view, &gating);
        let destinations = slice::from_raw_parts(state.destinations.cast(), destination_count);
        // The Send PDC delay histories are Rust-owned; this section keeps a
        // control-thread mutation from retiring them mid-walk.
        hirari_routing_engine_enter_send_pdc_read(state.routing);
        state.walk_graph(
            frames,
            playhead,
            sample_rate.max(1.0) as u32,
            generation,
            view,
            &gating,
            master_left,
            master_right,
            destinations,
        );
        hirari_routing_engine_leave_send_pdc_read(state.routing);
    }
}

/// The transport position the callback last advanced.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_playhead(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.playhead.load(Ordering::Relaxed))
}

/// Seeks the transport.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_playhead(state: *mut c_void, playhead: u64) {
    if let Some(state) = block(state) {
        state.playhead.store(playhead, Ordering::Relaxed);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_is_playing(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| u8::from(state.playing.load(Ordering::Acquire)))
}

/// Starts or stops the transport. Stopping asks the callback to release every
/// voice it holds and requests the playback panic, because the voice state is
/// owned by that thread and must not be raced from the control side.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_playing(state: *mut c_void, playing: u8) {
    let Some(state) = block(state) else {
        return;
    };
    let playing = playing != 0;
    state.playing.store(playing, Ordering::Release);
    if !playing {
        state.preview_voices_need_reset.store(true, Ordering::Release);
        hirari_midi_playback_request_panic(state.midi_clock);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_loop_enabled(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.loop_enabled.load(Ordering::Acquire))
    })
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_loop(state: *mut c_void, enabled: u8) {
    if let Some(state) = block(state) {
        state
            .loop_enabled
            .store(enabled != 0, Ordering::Release);
    }
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_cycle_start(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.cycle_start.load(Ordering::Acquire))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_cycle_end(state: *const c_void) -> u64 {
    block(state).map_or(0, |state| state.cycle_end.load(Ordering::Acquire))
}

/// Adopts a cycle, or refuses an empty one. Returns zero when the range was
/// rejected and nothing changed.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_cycle(
    state: *mut c_void,
    start_sample: u64,
    end_sample: u64,
    enabled: u8,
) -> u8 {
    let Some(state) = block(state) else {
        return 0;
    };
    if start_sample >= end_sample {
        return 0;
    }
    state.cycle_start.store(start_sample, Ordering::Release);
    state.cycle_end.store(end_sample, Ordering::Release);
    state.loop_enabled.store(enabled != 0, Ordering::Release);
    1
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_test_tone(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| u8::from(state.test_tone.load(Ordering::Relaxed)))
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_test_tone(state: *mut c_void, enabled: u8) {
    if let Some(state) = block(state) {
        state.test_tone.store(enabled != 0, Ordering::Release);
    }
}

/// The device configuration generation the callbacks and queued commands are
/// tagged with.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_generation(state: *const c_void) -> u64 {
    block(state).map_or(1, |state| state.generation.load(Ordering::Acquire))
}

/// Moves to the next device configuration generation.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_bump_generation(state: *mut c_void) -> u64 {
    block(state).map_or(0, |state| {
        state.generation.fetch_add(1, Ordering::AcqRel)
    })
}

/// Closes callback admission while the control thread replaces the buffers a
/// callback traverses.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_set_config_transition(
    state: *mut c_void,
    transition: u8,
) {
    if let Some(state) = block(state) {
        state.config_transition.store(transition != 0, Ordering::Release);
    }
}

/// Reports whether a callback observed a saturated sample position.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_range_overflowed(state: *const c_void) -> u8 {
    block(state).map_or(0, |state| {
        u8::from(state.range_overflow.load(Ordering::Acquire))
    })
}

/// Asks the next callback to release every preview voice it holds.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_request_preview_voice_reset(state: *mut c_void) {
    if let Some(state) = block(state) {
        state
            .preview_voices_need_reset
            .store(true, Ordering::Release);
    }
}

/// Empties the callback's own event collection, as a device re-preparation does.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_clear_midi_events(state: *mut c_void) {
    if let Some(state) = block(state) {
        unsafe { hirari_midi_buffer_clear(state.block_midi) };
    }
}

/// Binds an engine's block. Every handle must outlive the returned state.
///
/// # Safety
/// `ops` and `handles` are read once; the handles belong to this one engine.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_create(
    handles: HirariEngineBlockHandles,
    ops: *const HirariEngineBlockOps,
) -> *mut c_void {
    // The walk writes these from the audio callback only, so they are bound once
    // here instead of rebuilt for every block.
    let mut telemetry: Box<[HirariEngineTelemetry]> =
        vec![HirariEngineTelemetry::default(); 2].into();
    let telemetry_ptr = telemetry.as_mut_ptr();
    let mut ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let ids_ptr = ids.as_mut_ptr();
    let mut lefts: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let lefts_ptr = lefts.as_mut_ptr();
    let mut rights: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let rights_ptr = rights.as_mut_ptr();
    let click: Box<[[f32; MAX_BLOCK_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BLOCK_FRAMES], [0.0; MAX_BLOCK_FRAMES]]);
    let click_ptr = Box::into_raw(click);
    let send_pdc: Box<[[f32; MAX_BUS_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BUS_FRAMES], [0.0; MAX_BUS_FRAMES]]);
    let send_pdc_ptr = Box::into_raw(send_pdc);
    let seed = RoutingTrackDestination {
        id: 0,
        can_process: 0,
        is_bus: 0,
        channel_count: 0,
        work_channels: ptr::null(),
        bus_context: ptr::null_mut(),
    };
    let mut destinations: Box<[RoutingTrackDestination]> = vec![seed; MAX_TRACKS].into();
    let destinations_ptr = destinations.as_mut_ptr();
    let mut graph_ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let graph_ids_ptr = graph_ids.as_mut_ptr();
    let mut process_order: Box<[u32]> = vec![0u32; MAX_GRAPH_NODES].into();
    let process_order_ptr = process_order.as_mut_ptr();
    let fallback_notes: Box<[[u32; FALLBACK_PITCHES]; FALLBACK_CHANNELS]> =
        Box::new([[0u32; FALLBACK_PITCHES]; FALLBACK_CHANNELS]);
    let fallback_notes_ptr = Box::into_raw(fallback_notes);
    std::mem::forget(telemetry);
    std::mem::forget(ids);
    std::mem::forget(lefts);
    std::mem::forget(rights);
    std::mem::forget(destinations);
    std::mem::forget(graph_ids);
    std::mem::forget(process_order);
    Box::into_raw(Box::new(EngineBlockState {
        ops: if ops.is_null() {
            HirariEngineBlockOps::default()
        } else {
            unsafe { *ops }
        },
        routing: handles.routing,
        track_table: handles.track_table,
        bounce: handles.bounce,
        input_blocks: handles.input_blocks,
        control_room: handles.control_room,
        mixer_telemetry: handles.mixer_telemetry,
        metronome: handles.metronome,
        master_limiter: handles.master_limiter,
        meter: handles.meter,
        tempo: handles.tempo,
        bus_system: handles.bus_system,
        sidechain: handles.sidechain,
        rings: handles.rings,
        midi_clock: handles.midi_clock,
        midi_snapshots: handles.midi_snapshots,
        preview_synth: handles.preview_synth,
        preview_sampler: handles.preview_sampler,
        mpe: handles.mpe,
        macro_control: handles.macro_control,
        playhead: AtomicU64::new(0),
        playing: AtomicBool::new(false),
        loop_enabled: AtomicBool::new(false),
        cycle_start: AtomicU64::new(0),
        cycle_end: AtomicU64::new(0),
        test_tone: AtomicBool::new(false),
        generation: AtomicU64::new(1),
        config_transition: AtomicBool::new(false),
        range_overflow: AtomicBool::new(false),
        preview_voices_need_reset: AtomicBool::new(false),
        monitor_offset: AtomicU32::new(0),
        talkback_offset: AtomicU32::new(0),
        block_midi: hirari_midi_buffer_create(),
        fallback_notes: fallback_notes_ptr,
        telemetry: telemetry_ptr,
        telemetry_active: AtomicU32::new(0),
        telemetry_lock: AtomicBool::new(false),
        telemetry_ids: ids_ptr,
        telemetry_left: lefts_ptr,
        telemetry_right: rights_ptr,
        click: click_ptr,
        send_pdc: send_pdc_ptr,
        destinations: destinations_ptr.cast(),
        graph_ids: graph_ids_ptr.cast(),
        process_order: process_order_ptr.cast(),
    }))
    .cast()
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { Box::from_raw(state.cast::<EngineBlockState>()) };
    unsafe {
        hirari_midi_buffer_destroy(state.block_midi);
        drop(Box::from_raw(state.fallback_notes));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry,
            2,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_left,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_right,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(state.click));
        drop(Box::from_raw(state.send_pdc));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.destinations,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.graph_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.process_order,
            MAX_GRAPH_NODES,
        )));
    }
}

/// The buffer index a control reader should copy.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub extern "C" fn hirari_engine_telemetry_active_index(state: *const c_void) -> u32 {
    block(state).map_or(0, |state| state.telemetry_active.load(Ordering::Acquire))
}

/// Copies one published metering block, or reports that the audio callback
/// holds it.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`] and
/// `out` writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_telemetry_snapshot(
    state: *const c_void,
    index: u32,
    out: *mut HirariEngineTelemetry,
) -> bool {
    let Some(state) = block(state) else {
        return false;
    };
    if index >= 2 || out.is_null() {
        return false;
    }
    if state.telemetry_lock.swap(true, Ordering::Acquire) {
        return false;
    }
    unsafe { *out = *state.telemetry.offset(index as isize) };
    state.telemetry_lock.store(false, Ordering::Release);
    true
}
