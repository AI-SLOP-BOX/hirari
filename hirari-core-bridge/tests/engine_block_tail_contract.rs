//! Contract for the Rust-owned engine block tail (engine_block_host.rs).
//!
//! The tail is the last part of a device callback: it feeds the routing
//! feedback edges, publishes per-track metering, injects the metronome, scales
//! and limits the master, measures it, mixes the Control Room cue and advances
//! the playhead it holds. Tracks are the Rust objects of `engine_track.rs`, so
//! the test builds real ones and reads back what the tail wrote into them.

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
    hirari_engine_block_playhead, hirari_engine_block_range_overflowed,
    hirari_engine_block_set_auto_record_mode, hirari_engine_block_set_playhead,
    hirari_engine_block_set_playing, hirari_engine_telemetry_active_index,
    hirari_engine_telemetry_snapshot, HirariEngineBlockHandles, HirariEngineTelemetry,
};
use hirari_core_bridge::engine_input_block::{
    hirari_input_blocks_create, hirari_input_blocks_destroy,
};
use hirari_core_bridge::engine_track::{
    hirari_layout_revision_create, hirari_layout_revision_release,
    hirari_track_flush_recorded_automation, hirari_track_new, hirari_track_prepare,
    hirari_track_set_record_armed, hirari_track_storage, STORAGE_WORK_BUFFER, TRACK_TYPE_AUDIO,
};
use hirari_core_bridge::engine_track_table::{
    hirari_engine_track_insert, hirari_engine_track_publish, hirari_engine_track_table_create,
    hirari_engine_track_table_destroy,
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
use std::ffi::c_void;

const FRAMES: u32 = 4;
const RATE: f64 = 44_100.0;
/// The block length every track is prepared with, so a track that cannot hold
/// the walked block still has room for the samples the test writes.
const CAPACITY: u32 = 8;

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
    /// `[routing, bounce, input blocks, control room, mixer telemetry,
    /// metronome, limiter, meter, tempo, bus system, sidechain engine]`.
    borrowed: [*mut c_void; 11],
    revision: *mut c_void,
    /// The published graph the tail traverses, as in the engine.
    table: *mut c_void,
}

impl Engine {
    fn new() -> Self {
        Self {
            state: std::ptr::null_mut(),
            output: storage(2, 8192),
            tracks: Vec::new(),
            borrowed: [std::ptr::null_mut(); 11],
            revision: std::ptr::null_mut(),
            table: std::ptr::null_mut(),
        }
    }

    fn bind(&mut self) -> &mut Self {
        self.revision = hirari_layout_revision_create();
        self.borrowed = unsafe {
            [
                hirari_routing_engine_create(),
                hirari_bounce_session_create(),
                hirari_input_blocks_create(8192),
                hirari_control_room_state_create(),
                hirari_mixer_telemetry_create(),
                hirari_metronome_create(RATE),
                hirari_master_limiter_create(RATE),
                hirari_master_meter_create(RATE),
                hirari_tempo_map_create(),
                hirari_bus_system_create(),
                hirari_sidechain_engine_create(),
            ]
        };
        let borrowed = self.borrowed;
        self.table = unsafe { hirari_engine_track_table_create() };
        let handles = HirariEngineBlockHandles {
            routing: borrowed[0],
            track_table: self.table,
            bounce: borrowed[1],
            input_blocks: borrowed[2],
            control_room: borrowed[3],
            mixer_telemetry: borrowed[4],
            metronome: borrowed[5],
            master_limiter: borrowed[6],
            meter: borrowed[7],
            tempo: borrowed[8],
            bus_system: borrowed[9],
            sidechain: borrowed[10],
            ..HirariEngineBlockHandles::default()
        };
        self.state = unsafe { hirari_engine_block_create(handles) };
        assert!(true);
        self
    }

    /// Adds an audio track whose work block holds `frames` samples, returning
    /// its published slot index.
    fn track(&mut self, id: u32, frames: u32) -> usize {
        let name = b"test track";
        let mut track = std::ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                id,
                name.as_ptr(),
                name.len(),
                TRACK_TYPE_AUDIO,
                self.revision,
                &mut track,
            )
        };
        assert!(true);
        assert_eq!(
            unsafe { hirari_track_prepare(track, RATE, CAPACITY) },
            0,
            "the test track must prepare for the walked block"
        );
        // The length under test is the work buffer's own.
        assert!(unsafe {
            hirari_audio_buffer_storage_resize(
                hirari_track_storage(track, STORAGE_WORK_BUFFER),
                2,
                frames,
            )
        });
        self.tracks.push(track);
        let slot = self.tracks.len() - 1;
        // The table owns the handle from here, so destroying it releases the
        // track and every storage it owns.
        unsafe {
            assert!(hirari_engine_track_insert(self.table, usize::MAX, owner));
            hirari_engine_track_publish(self.table, 1, true);
        }
        slot
    }

    fn work(&self, slot: usize) -> *mut c_void {
        unsafe { hirari_track_storage(self.tracks[slot], STORAGE_WORK_BUFFER) }
    }

    /// Arms one track's recorder, exactly as the control side does.
    fn arm(&self, slot: usize) {
        assert!(unsafe { hirari_track_set_record_armed(self.tracks[slot], true) });
    }

    /// The automation points the tail captured for this track.
    fn records(&self, slot: usize) -> u32 {
        unsafe { hirari_track_flush_recorded_automation(self.tracks[slot]) }
    }

    /// Runs one tail and reports the playhead step and the sticky overflow the
    /// block ended with.
    fn finish(&self, playhead: u64, playing: bool, auto_record: bool) -> (u32, u8) {
        unsafe {
            hirari_engine_block_set_playhead(self.state, playhead);
            hirari_engine_block_set_playing(self.state, u8::from(playing));
            hirari_engine_block_set_auto_record_mode(self.state, u32::from(auto_record));
            hirari_engine_block_finish(self.state, self.output, 0, FRAMES);
            let stepped = hirari_engine_block_playhead(self.state);
            (
                stepped.saturating_sub(playhead) as u32,
                hirari_engine_block_range_overflowed(self.state),
            )
        }
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
            hirari_engine_track_table_destroy(self.table);
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
            hirari_layout_revision_release(self.revision);
            hirari_audio_buffer_storage_destroy(self.output);
        }
    }
}

#[test]
fn a_playing_block_publishes_metering_records_and_advances_the_playhead() {
    let mut engine = Engine::new();
    engine.bind();
    let slot = engine.track(3, FRAMES);
    engine.arm(slot);
    fill(
        engine.work(slot),
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
    fill(engine.work(loud), &[[0.25; 4], [0.25; 4]]);

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
    engine.arm(slot);

    assert_eq!(engine.finish(0, false, true), (0, 0));
    assert_eq!(engine.records(slot), 0);
}

#[test]
fn a_block_larger_than_the_engine_cap_writes_nothing() {
    let mut engine = Engine::new();
    engine.bind();
    engine.track(4, FRAMES);
    unsafe {
        hirari_engine_block_set_playing(engine.state, 1);
        hirari_engine_block_finish(engine.state, engine.output, 0, 16_385);
    };
    assert_eq!(
        unsafe { hirari_engine_block_playhead(engine.state) },
        0,
        "the playhead never advanced"
    );
    assert_eq!(
        unsafe { hirari_engine_block_range_overflowed(engine.state) },
        0,
        "nor was an overflow reported"
    );
    assert_eq!(engine.telemetry().version, 0, "nothing was published");
}
