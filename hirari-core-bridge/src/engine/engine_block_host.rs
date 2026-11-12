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
//! Tracks are Rust objects ([`crate::engine_track`]), and every read and write
//! the block makes on one is a direct call on the storage handle it asks for by
//! kind -- the runtime, the channel strip, the articulation state, the MIDI input
//! buffer and a bus's stage. The only protocol still around a store write is the
//! control-edit pair: drop the frozen render before it, bump the layout revision
//! after it. Every tap buffer the walk reads is already bound in the track's
//! [`TrackRuntime`].

use crate::audio_buffer_ops::{
    hirari_audio_add_sanitized_stereo, hirari_audio_buffer_storage_channels,
    hirari_audio_buffer_storage_read_pointer, hirari_audio_buffer_storage_samples,
};
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
use crate::engine_track::{
    hirari_track_begin_edit, hirari_track_end_edit, hirari_track_prepare,
    hirari_track_record_state, hirari_track_storage, hirari_track_type_byte,
    STORAGE_ARTICULATION, STORAGE_BUS_STATE, STORAGE_PLUGIN_MIDI, STORAGE_RUNTIME, STORAGE_STRIP,
};
use crate::engine_track_table::{
    hirari_engine_track_count, hirari_engine_track_ptrs, hirari_engine_track_snapshot,
    HirariTrackSnapshot,
};
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
use crate::sidechain_manager::{
    hirari_sidechain_engine_publish_source, hirari_sidechain_engine_refresh_source,
};
use crate::tempo::{
    hirari_tempo_map_beats_to_samples, hirari_tempo_map_get_current_bpm,
    hirari_tempo_map_samples_to_beats,
};
use crate::track_midi_articulation::{
    hirari_track_midi_all_notes_off, hirari_track_midi_note_off, hirari_track_midi_note_on,
    hirari_track_midi_schedule_note_off, hirari_track_midi_timed_outputs, GeneratedMidiEvent,
    TimedKeySwitchOutput,
};
use crate::track_runtime::{self, hirari_track_runtime_set_muted, TrackRuntime};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

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

/// The track's role, as the byte [`crate::engine_track`] reports it.
const TRACK_TYPE_MIDI: u8 = 1;
const TRACK_TYPE_INSTRUMENT: u8 = 2;
const TRACK_TYPE_BUS: u8 = 3;

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
    /// The layout revision the tracks mark and the control side polls, so a
    /// revision read needs no engine accessor.
    pub layout_revision: *mut c_void,
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
    layout_revision: *mut c_void,
    /// The master fader the tail mixes with, the normalized record-arm wire mode,
    /// the shutdown flag that closes block admission, the count of callbacks in
    /// flight the control thread joins before it replaces a buffer, the
    /// control-plane edit token and the project tonality a save writes back out.
    /// The engine facade held each of these as its own atomic and handed it to
    /// the block one argument per call; they are the block's state now.
    master_gain: AtomicU32,
    auto_record_mode: AtomicU32,
    shutdown: AtomicBool,
    callbacks_in_flight: AtomicU32,
    control_revision: AtomicU64,
    root_note: AtomicI32,
    scale_type: AtomicI32,
    playhead: AtomicU64,
    playing: AtomicBool,
    loop_enabled: AtomicBool,
    cycle_start: AtomicU64,
    cycle_end: AtomicU64,
    test_tone: AtomicBool,
    generation: AtomicU64,
    /// The device format the graph was last prepared for, recorded by
    /// [`hirari_engine_block_prepare_tracks`] — the one funnel every
    /// preparation passes through. Control-side edit hosts read it back as the
    /// registration inputs for live audio routing instead of asking C++.
    device_sample_rate: AtomicU64,
    device_block_frames: AtomicU32,
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

#[path = "engine_block_midi.rs"]
mod block_midi;

#[path = "engine_block_ffi.rs"]
mod block_ffi;
pub use block_ffi::*;

#[path = "engine_block_state_impl.rs"]
mod state_impl;
#[allow(unused_imports)]
pub(crate) use state_impl::*;
