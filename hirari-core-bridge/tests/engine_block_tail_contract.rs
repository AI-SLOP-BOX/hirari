//! Contract for the Rust-owned engine block tail (engine_block_host.rs).
//!
//! The tail is the last part of a device callback: it feeds the routing
//! feedback edges, publishes per-track metering, injects the metronome, scales
//! and limits the master, measures it, mixes the Control Room cue and reports
//! the playhead step. Tracks stay opaque, so the test drives the published
//! operations table instead of a C++ `Track`.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::bounce_driver::{
    hirari_bounce_session_create, hirari_bounce_session_destroy,
};
use hirari_core_bridge::bus_audio::{hirari_bus_system_create, hirari_bus_system_destroy};
use hirari_core_bridge::control_room_runtime::{
    hirari_control_room_state_create, hirari_control_room_state_destroy,
};
use hirari_core_bridge::engine_block_host::{
    hirari_engine_block_create, hirari_engine_block_destroy, hirari_engine_block_finish,
    hirari_engine_telemetry_active_index, hirari_engine_telemetry_snapshot, HirariEngineBlockOps,
    HirariEngineTelemetry,
};
use hirari_core_bridge::engine_input_block::{
    hirari_input_blocks_create, hirari_input_blocks_destroy,
};
use hirari_core_bridge::master_limiter::{
    hirari_master_limiter_create, hirari_master_limiter_destroy,
};
use hirari_core_bridge::master_meter::{hirari_master_meter_create, hirari_master_meter_destroy};
use hirari_core_bridge::metronome_renderer::{hirari_metronome_create, hirari_metronome_destroy};
use hirari_core_bridge::mixer_telemetry::{
    hirari_mixer_telemetry_create, hirari_mixer_telemetry_destroy,
};
use hirari_core_bridge::routing_engine_core::{
    hirari_routing_engine_create, hirari_routing_engine_destroy,
};
use hirari_core_bridge::sidechain_manager::{
    hirari_sidechain_engine_create, hirari_sidechain_engine_destroy,
};
use hirari_core_bridge::tempo::{hirari_tempo_map_create, hirari_tempo_map_destroy};
use hirari_core_bridge::track_runtime::{
    hirari_track_runtime_create, hirari_track_runtime_destroy,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

const FRAMES: u32 = 4;

/// Stand-in for a C++ `Track`: the ops table is the only way the block reaches it.
struct FakeTrack {
    runtime: *mut c_void,
    bus: *mut c_void,
    solo: bool,
    records: AtomicUsize,
}

unsafe extern "C" fn runtime_of(track: *mut c_void) -> *mut c_void {
    unsafe { &*track.cast::<FakeTrack>() }.runtime
}

unsafe extern "C" fn bus_state_of(track: *mut c_void) -> *mut c_void {
    unsafe { &*track.cast::<FakeTrack>() }.bus
}

unsafe extern "C" fn solo_of(track: *mut c_void) -> u8 {
    u8::from(unsafe { &*track.cast::<FakeTrack>() }.solo)
}

unsafe extern "C" fn record_of(track: *mut c_void, _playhead: u64) {
    unsafe { &*track.cast::<FakeTrack>() }
        .records
        .fetch_add(1, Ordering::Relaxed);
}

fn storage(channels: u32, frames: u32) -> *mut c_void {
    unsafe {
        let handle = hirari_audio_buffer_storage_create();
        assert!(
            hirari_audio_buffer_storage_reserve_buffer(handle, channels, frames) == 0
                && hirari_audio_buffer_storage_resize(handle, channels, frames),
            "test storage must be reservable"
        );
        handle
    }
}

fn write_planes(handle: *mut c_void) -> [*mut f32; 2] {
    unsafe { *hirari_audio_buffer_storage_array_write(handle).cast::<[*mut f32; 2]>() }
}

fn fill(handle: *mut c_void, values: &[[f32; FRAMES as usize]]) {
    let planes = write_planes(handle);
    for (channel, samples) in values.iter().enumerate() {
        for (frame, value) in samples.iter().enumerate() {
            unsafe { *planes[channel].add(frame) = *value };
        }
    }
}

/// One engine: the bound tail, the handles it borrows, and the tracks it walks.
struct Engine {
    state: *mut c_void,
    output: *mut c_void,
    tracks: Vec<*mut c_void>,
    work: Vec<*mut c_void>,
    runtime: Vec<*mut c_void>,
    /// `[routing, bounce, input blocks, control room, mixer telemetry,
    /// metronome, limiter, meter, tempo, bus system, sidechain engine]`.
    borrowed: [*mut c_void; 11],
}

impl Engine {
    fn new() -> Self {
        Self {
            state: std::ptr::null_mut(),
            output: storage(2, 8192),
            tracks: Vec::new(),
            work: Vec::new(),
            runtime: Vec::new(),
            borrowed: [std::ptr::null_mut(); 11],
        }
    }

    fn bind(&mut self) -> &mut Self {
        let ops = HirariEngineBlockOps {
            runtime: Some(runtime_of),
            bus_state: Some(bus_state_of),
            solo: Some(solo_of),
            record: Some(record_of),
        };
        self.borrowed = unsafe {
            [
                hirari_routing_engine_create(),
                hirari_bounce_session_create(),
                hirari_input_blocks_create(8192),
                hirari_control_room_state_create(),
                hirari_mixer_telemetry_create(),
                hirari_metronome_create(44_100.0),
                hirari_master_limiter_create(44_100.0),
                hirari_master_meter_create(44_100.0),
                hirari_tempo_map_create(),
                hirari_bus_system_create(),
                hirari_sidechain_engine_create(),
            ]
        };
        let borrowed = self.borrowed;
        self.state = unsafe {
            hirari_engine_block_create(
                borrowed[0],
                borrowed[1],
                borrowed[2],
                borrowed[3],
                borrowed[4],
                borrowed[5],
                borrowed[6],
                borrowed[7],
                borrowed[8],
                borrowed[9],
                borrowed[10],
                &ops,
            )
        };
        assert!(!self.state.is_null());
        self
    }

    /// Adds a track whose work buffer holds `frames` samples, returning its
    /// published slot index.
    fn track(&mut self, id: u32, frames: u32) -> usize {
        let work = storage(2, frames);
        let runtime = unsafe {
            let null = std::ptr::null_mut::<c_void>();
            hirari_track_runtime_create(
                id, null, null, null, null, null, null, work, null, null, null, null, null, None,
                null,
            )
        };
        let track = Box::into_raw(Box::new(FakeTrack {
            runtime,
            bus: std::ptr::null_mut(),
            solo: false,
            records: AtomicUsize::new(0),
        }));
        self.work.push(work);
        self.runtime.push(runtime);
        self.tracks.push(track.cast());
        self.tracks.len() - 1
    }

    fn records(&self, slot: usize) -> usize {
        unsafe { &*self.tracks[slot].cast::<FakeTrack>() }
            .records
            .load(Ordering::Relaxed)
    }

    /// Runs one tail and reports the playhead step and overflow the engine
    /// would apply.
    fn finish(&self, playhead: u64, playing: bool, auto_record: bool) -> (u32, u8) {
        let mut advance = 9_u32;
        let mut overflow = 9_u8;
        unsafe {
            hirari_engine_block_finish(
                self.state,
                self.output,
                0,
                FRAMES,
                playhead,
                44_100.0,
                1.0,
                u8::from(playing),
                u8::from(auto_record),
                0,
                self.tracks.as_ptr(),
                self.tracks.len() as u32,
                &mut advance,
                &mut overflow,
            )
        };
        (advance, overflow)
    }

    /// The block the audio callback last published.
    fn telemetry(&self) -> HirariEngineTelemetry {
        let index = hirari_engine_telemetry_active_index(self.state);
        let mut snapshot = HirariEngineTelemetry::default();
        assert!(
            unsafe { hirari_engine_telemetry_snapshot(self.state, index, &mut snapshot) },
            "the published block must be readable"
        );
        snapshot
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            // The tail borrows every other handle, so it is retired first.
            hirari_engine_block_destroy(self.state);
            for handle in self.runtime.drain(..) {
                hirari_track_runtime_destroy(handle);
            }
            for handle in self.work.drain(..) {
                hirari_audio_buffer_storage_destroy(handle);
            }
            for handle in self.tracks.drain(..) {
                drop(Box::from_raw(handle.cast::<FakeTrack>()));
            }
            // The block state borrowed all of these, so it is already retired.
            if !self.state.is_null() {
                let [routing, bounce, input, control_room, mixer, metronome, limiter, meter, tempo, bus_system, sidechain] =
                    self.borrowed;
                hirari_routing_engine_destroy(routing);
                hirari_bounce_session_destroy(bounce);
                hirari_input_blocks_destroy(input);
                hirari_control_room_state_destroy(control_room);
                hirari_mixer_telemetry_destroy(mixer);
                hirari_metronome_destroy(metronome);
                hirari_master_limiter_destroy(limiter);
                hirari_master_meter_destroy(meter);
                hirari_tempo_map_destroy(tempo);
                hirari_bus_system_destroy(bus_system);
                hirari_sidechain_engine_destroy(sidechain);
            }
            hirari_audio_buffer_storage_destroy(self.output);
        }
    }
}

#[test]
fn a_playing_block_publishes_metering_records_and_advances_the_playhead() {
    let mut engine = Engine::new();
    engine.bind();
    let slot = engine.track(3, FRAMES);
    fill(
        engine.work[slot],
        &[[0.5, -0.25, 0.75, -1.0], [0.125, 0.25, -0.5, 0.25]],
    );

    assert_eq!(engine.finish(0, true, true), (FRAMES, 0));

    let telemetry = engine.telemetry();
    assert_eq!(telemetry.count, 1);
    assert_eq!(telemetry.peaks_left[0], 1.0);
    assert_eq!(telemetry.peaks_right[0], 0.5);
    assert_eq!(telemetry.version, 1);
    assert_eq!(
        engine.records(slot),
        1,
        "the armed recorder is captured once"
    );
}

#[test]
fn metering_stays_aligned_with_the_graph_when_a_track_cannot_hold_the_block() {
    let mut engine = Engine::new();
    engine.bind();
    let loud = engine.track(5, FRAMES);
    let small = engine.track(6, 1);
    fill(engine.work[loud], &[[0.25; 4], [0.25; 4]]);

    engine.finish(0, true, false);

    let telemetry = engine.telemetry();
    assert_eq!(telemetry.count, 2, "the payload keeps every slot");
    assert_eq!(telemetry.peaks_left[loud], 0.25);
    assert_eq!(telemetry.peaks_right[loud], 0.25);
    assert_eq!(telemetry.peaks_left[small], 0.0);
    assert_eq!(engine.records(small), 0, "recording is gated by the engine");
}

#[test]
fn a_saturated_playhead_is_reported_instead_of_wrapping() {
    let mut engine = Engine::new();
    engine.bind();
    engine.track(1, FRAMES);

    assert_eq!(
        engine.finish(u64::MAX - 2, true, false),
        (0, 1),
        "the callback sets the sticky overflow and leaves the playhead"
    );
}

#[test]
fn a_stopped_engine_reports_no_step_and_captures_no_automation() {
    let mut engine = Engine::new();
    engine.bind();
    let slot = engine.track(2, FRAMES);

    assert_eq!(engine.finish(0, false, true), (0, 0));
    assert_eq!(engine.records(slot), 0);
}

#[test]
fn a_block_larger_than_the_engine_cap_writes_nothing() {
    let mut engine = Engine::new();
    engine.bind();
    engine.track(4, FRAMES);
    let mut advance = 0_u32;
    let mut overflow = 0_u8;
    unsafe {
        hirari_engine_block_finish(
            engine.state,
            engine.output,
            0,
            16_385,
            0,
            44_100.0,
            1.0,
            1,
            0,
            0,
            engine.tracks.as_ptr(),
            engine.tracks.len() as u32,
            &mut advance,
            &mut overflow,
        )
    };
    assert_eq!((advance, overflow), (0, 0));
    assert_eq!(engine.telemetry().version, 0, "nothing was published");
}
