//! Contract for the Rust-owned engine graph walk (engine_block_host.rs).
//!
//! The walk is the first half of a device callback: it stages every track,
//! injects the one-block delayed feedback edges, feeds the live driver input,
//! publishes the routing destination table and walks the graph in process order,
//! fanning each eligible node into its buses and the master window. Tracks are
//! the Rust objects of `engine_track.rs`, so the test builds real ones and
//! reaches the storages the walk reaches.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_samples,
};
use hirari_core_bridge::bounce_driver::{
    hirari_bounce_session_create, hirari_bounce_session_destroy,
};
use hirari_core_bridge::bus_audio::{
    hirari_bus_audio_read, hirari_bus_system_bus, hirari_bus_system_create,
    hirari_bus_system_destroy, hirari_bus_system_register,
};
use hirari_core_bridge::channel_strip::hirari_channel_strip_set_solo;
use hirari_core_bridge::control_room_runtime::{
    hirari_control_room_state_create, hirari_control_room_state_destroy,
};
use hirari_core_bridge::engine_block_host::{
    hirari_engine_block_create, hirari_engine_block_destroy, hirari_engine_block_finish,
    hirari_engine_block_prepare_tracks, hirari_engine_block_route,
    hirari_engine_block_set_device_format, hirari_engine_block_set_playhead,
    hirari_engine_block_set_playing, HirariEngineBlockHandles,
};
use hirari_core_bridge::engine_input_block::{
    hirari_input_blocks_create, hirari_input_blocks_destroy,
};
use hirari_core_bridge::engine_track::{
    hirari_layout_revision_create, hirari_layout_revision_current, hirari_layout_revision_release,
    hirari_track_new, hirari_track_prepare, hirari_track_resolve_bus, hirari_track_storage,
    STORAGE_BUS_STATE, STORAGE_FROZEN, STORAGE_PLUGIN_MIDI, STORAGE_RUNTIME, STORAGE_SEND_POST,
    STORAGE_SEND_PRE, STORAGE_STRIP, STORAGE_WORK_BUFFER, TRACK_TYPE_AUDIO, TRACK_TYPE_BUS,
    TRACK_TYPE_INSTRUMENT,
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
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_event_count,
};
use hirari_core_bridge::mixer_telemetry::{
    hirari_mixer_telemetry_create, hirari_mixer_telemetry_destroy,
};
use hirari_core_bridge::routing_engine_core::{
    hirari_routing_engine_add_connection, hirari_routing_engine_add_feedback,
    hirari_routing_engine_create, hirari_routing_engine_destroy,
};
use hirari_core_bridge::sidechain_manager::{
    hirari_sidechain_engine_create, hirari_sidechain_engine_destroy,
};
use hirari_core_bridge::tempo::{hirari_tempo_map_create, hirari_tempo_map_destroy};
use hirari_core_bridge::track_frozen_store::{
    hirari_track_frozen_store_is_frozen, hirari_track_frozen_store_publish,
    hirari_track_frozen_store_sample_rate,
};
use hirari_core_bridge::track_runtime::hirari_track_runtime_publish_regions;
use std::ffi::c_void;
use std::ptr;

const FRAMES: u32 = 4;
const BUS_ID: u32 = 0;
const RATE: f64 = 44_100.0;
/// The block length every track is prepared with, so each tap the walk reads has
/// room for the largest block these contracts run. The length under test is then
/// set on the work buffer alone.
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

/// A source the walk runs: its own block publishes the Send taps a contract
/// can name, which is what a real engine does and a silent track cannot.
fn sounding_source(engine: &mut Engine, id: u32) -> usize {
    let slot = engine.track(id, CAPACITY, 0.5);
    engine.publish_regions(slot);
    slot
}

/// One block's plane scaled the way a Send edge scales the tap it reads.
fn scaled(values: [f32; FRAMES as usize], gain: f32) -> [f32; FRAMES as usize] {
    let mut out = [0.0_f32; FRAMES as usize];
    for (frame, value) in out.iter_mut().zip(values) {
        *frame = value * gain;
    }
    out
}

fn samples(handle: *mut c_void) -> [f32; FRAMES as usize] {
    unsafe { *write_planes(handle)[0].cast::<[f32; FRAMES as usize]>() }
}

fn right_samples(handle: *mut c_void) -> [f32; FRAMES as usize] {
    unsafe { *write_planes(handle)[1].cast::<[f32; FRAMES as usize]>() }
}

/// The two Send taps a walked block publishes: the pre-fader copy the output
/// stage takes before the fader and the post-fader copy it leaves downstream.
fn taps(engine: &Engine, slot: usize) -> ([f32; FRAMES as usize], [f32; FRAMES as usize]) {
    (
        samples(engine.send_pre(slot)),
        samples(engine.send_post(slot)),
    )
}

/// One engine: the bound block state, the handles it borrows and the graph it
/// walks.
struct Engine {
    state: *mut c_void,
    output: *mut c_void,
    /// The object pointer of each track, in the slot the published graph gives
    /// it. The track table owns the tracks themselves.
    tracks: Vec<*mut c_void>,
    /// `[routing, bounce, input blocks, control room, mixer telemetry,
    /// metronome, limiter, meter, tempo, bus system, sidechain engine]`.
    borrowed: [*mut c_void; 11],
    /// The revision every track of this engine marks, shared as in the engine.
    revision: *mut c_void,
    bus_state: *mut c_void,
    /// The published graph the walk traverses, as in the engine.
    table: *mut c_void,
}

impl Engine {
    fn new() -> Self {
        Self {
            state: ptr::null_mut(),
            output: storage(2, 8192),
            tracks: Vec::new(),
            borrowed: [ptr::null_mut(); 11],
            revision: ptr::null_mut(),
            bus_state: ptr::null_mut(),
            table: ptr::null_mut(),
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
        // The block start clears every bus stage, so the slot a bus track
        // resolves to has to exist before the graph is walked.
        self.bus_state = unsafe {
            assert!(
                hirari_bus_system_register(self.bus_system(), BUS_ID),
                "the test bus must be present so the block start clears it"
            );
            hirari_bus_system_bus(self.bus_system(), BUS_ID)
        };
        assert!(!self.bus_state.is_null());
        let borrowed = self.borrowed;
        self.table = hirari_engine_track_table_create();
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
        assert!(!self.state.is_null());
        self
    }

    fn routing(&self) -> *mut c_void {
        self.borrowed[0]
    }

    fn bus_system(&self) -> *mut c_void {
        self.borrowed[9]
    }

    fn handle(&self, track: *mut c_void, kind: u32) -> *mut c_void {
        unsafe { hirari_track_storage(track, kind) }
    }

    fn work(&self, slot: usize) -> *mut c_void {
        self.handle(self.tracks[slot], STORAGE_WORK_BUFFER)
    }

    fn send_pre(&self, slot: usize) -> *mut c_void {
        self.handle(self.tracks[slot], STORAGE_SEND_PRE)
    }

    fn send_post(&self, slot: usize) -> *mut c_void {
        self.handle(self.tracks[slot], STORAGE_SEND_POST)
    }

    fn frozen(&self, slot: usize) -> *mut c_void {
        self.handle(self.tracks[slot], STORAGE_FROZEN)
    }

    /// Adds a track whose work block holds `frames` samples. A non-negative
    /// level makes it an instrument track, so the runtime renders its built-in
    /// voice into the block the graph then routes; a negative level publishes a
    /// track with nothing to generate, which is how an audio track with no
    /// instrument reaches the walk.
    fn track(&mut self, id: u32, frames: u32, level: f32) -> usize {
        let kind = if level >= 0.0 {
            TRACK_TYPE_INSTRUMENT
        } else {
            TRACK_TYPE_AUDIO
        };
        self.build(id, frames, kind, false)
    }

    fn staged_track(&mut self, id: u32, frames: u32, level: f32, solo: bool) -> usize {
        let kind = if level >= 0.0 {
            TRACK_TYPE_INSTRUMENT
        } else {
            TRACK_TYPE_AUDIO
        };
        self.build(id, frames, kind, solo)
    }

    /// Adds a bus track occupying the engine's one registered bus slot. The walk
    /// has no voice stage for it, so its block is the bus's pre-FX stage.
    fn bus_track(&mut self, id: u32, frames: u32) -> usize {
        self.build(id, frames, TRACK_TYPE_BUS, false)
    }

    fn build(&mut self, id: u32, frames: u32, kind: u32, solo: bool) -> usize {
        let name = b"test track";
        let mut track = ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                id,
                name.as_ptr(),
                name.len(),
                kind,
                self.revision,
                &mut track,
            )
        };
        assert!(!track.is_null());
        if kind == TRACK_TYPE_BUS {
            unsafe { hirari_track_resolve_bus(track, self.bus_state, true) };
        }
        assert_eq!(
            unsafe { hirari_track_prepare(track, RATE, CAPACITY) },
            0,
            "the test track must prepare for the walked block"
        );
        // The length under test belongs to the work buffer alone; the Send taps
        // keep the prepared size so staging cannot empty them.
        assert!(unsafe {
            hirari_audio_buffer_storage_resize(self.handle(track, STORAGE_WORK_BUFFER), 2, frames)
        });
        if solo {
            // Solo is strip state, exactly as the engine reads it. The strip is
            // set directly because the layout mark an edit raises is not what
            // these contracts measure.
            unsafe { hirari_channel_strip_set_solo(self.handle(track, STORAGE_STRIP), solo) };
        }
        // One held note gives the voice something to render, and the runtime
        // consumes the buffer when the block reaches its chain stage. That is
        // how these contracts tell a walked track from a skipped one.
        unsafe {
            hirari_midi_buffer_add_note_on(
                self.handle(track, STORAGE_PLUGIN_MIDI),
                1,
                69,
                100,
                0,
                0,
            )
        };
        self.tracks.push(track);
        let slot = self.tracks.len() - 1;
        // The engine publishes the graph after every structural edit; the walk
        // only ever sees the published snapshot. The table owns the handle from
        // here, so destroying the table is what releases the track.
        unsafe {
            assert!(hirari_engine_track_insert(self.table, usize::MAX, owner));
            hirari_engine_track_publish(self.table, 1, true);
        }
        slot
    }

    /// Opens the region gate so the runtime reaches its voice stage.
    fn publish_regions(&self, slot: usize) {
        unsafe {
            hirari_track_runtime_publish_regions(
                self.handle(self.tracks[slot], STORAGE_RUNTIME),
                ptr::null(),
                0,
                1,
            )
        };
    }

    /// True once the walk has run this track's block: the runtime hands the
    /// staged note to its chain stage, which consumes the MIDI buffer.
    fn rendered(&self, slot: usize) -> bool {
        slot < self.tracks.len()
            && unsafe {
                hirari_midi_buffer_event_count(self.handle(self.tracks[slot], STORAGE_PLUGIN_MIDI))
            } == 0
    }

    fn connect(&self, source: u32, destination: u32, gain: f32, send: bool, pre_fader: bool) {
        assert!(unsafe {
            hirari_routing_engine_add_connection(
                self.routing(),
                source,
                destination,
                gain,
                send,
                pre_fader,
            )
        });
    }

    fn add_feedback(&self, source: u32, destination: u32, gain: f32) {
        assert!(unsafe {
            hirari_routing_engine_add_feedback(self.routing(), source, destination, gain)
        });
    }

    /// Runs the graph half of one block, with the transport the head would have
    /// resolved before handing the block to the walk.
    fn route(&self, playhead: u64) {
        unsafe {
            hirari_engine_block_set_playhead(self.state, playhead);
            hirari_engine_block_set_playing(self.state, 1);
            hirari_engine_block_route(self.state, self.output, 0, FRAMES);
        };
    }

    /// Runs the tail half of one block, which is where feedback is captured.
    fn finish(&self, playhead: u64) {
        unsafe {
            hirari_engine_block_set_playhead(self.state, playhead);
            hirari_engine_block_set_playing(self.state, 1);
            hirari_engine_block_finish(self.state, self.output, 0, FRAMES);
        };
    }

    fn master(&self) -> ([f32; FRAMES as usize], [f32; FRAMES as usize]) {
        (samples(self.output), right_samples(self.output))
    }

    fn bus_stage(&self, slot: usize) -> ([f32; FRAMES as usize], [f32; FRAMES as usize]) {
        let mut left = [0.0_f32; FRAMES as usize];
        let mut right = [0.0_f32; FRAMES as usize];
        assert!(unsafe {
            hirari_bus_audio_read(
                self.handle(self.tracks[slot], STORAGE_BUS_STATE),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                FRAMES,
                false,
            )
        });
        (left, right)
    }

    /// Publishes a rendered snapshot taken at `sample_rate` on one track. The
    /// store owns the render once it accepts it, so the track releases it.
    fn publish_freeze(&self, slot: usize, sample_rate: u32) {
        let source = storage(2, FRAMES);
        fill(source, &[[1.0; FRAMES as usize], [-1.0; FRAMES as usize]]);
        assert!(
            unsafe {
                hirari_track_frozen_store_publish(
                    self.frozen(slot),
                    source,
                    2,
                    FRAMES as u64,
                    FRAMES as u64,
                    sample_rate,
                    0,
                    0,
                    ptr::null(),
                    0,
                )
            },
            "the test freeze must publish"
        );
    }

    /// How many times this engine's layout has been marked. Every track of one
    /// engine shares the revision, so the count is the engine's own.
    fn marks(&self) -> u64 {
        unsafe { hirari_layout_revision_current(self.revision) }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            // The block state borrows every other handle, so it is retired first.
            hirari_engine_block_destroy(self.state);
            // The table owns the tracks, and a track destroys the storages it
            // owns once its last handle goes.
            hirari_engine_track_table_destroy(self.table);
            if !self.borrowed[0].is_null() {
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
            hirari_layout_revision_release(self.revision);
            hirari_audio_buffer_storage_destroy(self.output);
        }
    }
}

#[test]
fn the_walk_clears_each_track_and_fans_its_block_into_the_master() {
    let mut engine = Engine::new();
    engine.bind();
    let slot = engine.track(3, CAPACITY, 0.5);
    engine.publish_regions(slot);
    // Stale audio from a previous block must not reach the master.
    fill(engine.work(slot), &[[9.0; 4], [9.0; 4]]);

    engine.route(0);

    assert!(engine.rendered(slot), "the node ran once");
    assert_eq!(engine.master().0, samples(engine.work(slot)));
    assert_eq!(engine.master().1, right_samples(engine.work(slot)));
    assert_ne!(
        samples(engine.work(slot)),
        [9.0; 4],
        "the tap was staged first"
    );
}

#[test]
fn a_solo_track_is_the_only_node_the_walk_reaches() {
    let mut engine = Engine::new();
    engine.bind();
    let soloed = engine.staged_track(1, CAPACITY, 0.25, true);
    let silent = engine.track(2, CAPACITY, -0.75);
    engine.publish_regions(soloed);
    engine.publish_regions(silent);

    engine.route(0);

    assert!(engine.rendered(soloed));
    assert!(
        !engine.rendered(silent),
        "a muted-by-solo node never renders"
    );
    assert_eq!(samples(engine.work(silent)), [0.0; 4]);
    assert_eq!(engine.master().0, samples(engine.work(soloed)));
}

#[test]
fn a_track_that_cannot_hold_the_block_is_never_staged_or_walked() {
    let mut engine = Engine::new();
    engine.bind();
    let short = engine.track(4, 1, 0.5);
    engine.publish_regions(short);
    fill(engine.work(short), &[[7.0; 4], [7.0; 4]]);

    engine.route(0);

    assert!(!engine.rendered(short), "the destination table skipped it");
    assert_eq!(samples(engine.work(short)), [7.0; 4]);
    assert_eq!(engine.master().0, [0.0; 4]);
}

#[test]
fn a_pre_fader_send_reaches_the_bus_stage_from_its_own_tap() {
    let mut engine = Engine::new();
    engine.bind();
    let bus_slot = engine.bus_track(10, CAPACITY);
    let source_slot = sounding_source(&mut engine, 11);
    engine.connect(11, 10, 0.5, true, true);

    engine.route(0);

    let (pre, post) = taps(&engine, source_slot);
    assert_ne!(pre, [0.0; FRAMES as usize], "the block produced a Send tap");
    assert_ne!(
        pre, post,
        "the fader and pan separate the taps, so only one of them can answer for this edge"
    );
    let (stage_left, stage_right) = engine.bus_stage(bus_slot);
    assert_eq!(stage_left, scaled(pre, 0.5), "half of the pre-fader tap");
    assert_eq!(
        stage_right,
        scaled(right_samples(engine.send_pre(source_slot)), 0.5)
    );
}

#[test]
fn a_post_fader_send_reads_the_tap_downstream_of_the_fader() {
    let mut engine = Engine::new();
    engine.bind();
    let bus_slot = engine.bus_track(10, CAPACITY);
    let source_slot = sounding_source(&mut engine, 11);
    engine.connect(11, 10, 0.5, true, false);

    engine.route(0);

    let (pre, post) = taps(&engine, source_slot);
    assert_ne!(
        post, [0.0; FRAMES as usize],
        "the block produced a Send tap"
    );
    assert_ne!(pre, post, "the two taps cannot both answer for this edge");
    let (stage_left, stage_right) = engine.bus_stage(bus_slot);
    assert_eq!(stage_left, scaled(post, 0.5), "half of the post-fader tap");
    assert_eq!(
        stage_right,
        scaled(right_samples(engine.send_post(source_slot)), 0.5)
    );
}

#[test]
fn a_feedback_edge_is_captured_one_block_before_it_is_injected() {
    let mut engine = Engine::new();
    engine.bind();
    let source = engine.track(21, CAPACITY, 0.25);
    let destination = engine.track(20, CAPACITY, -1.0);
    engine.publish_regions(source);
    engine.add_feedback(21, 20, 1.0);

    engine.route(0);
    engine.finish(0);
    let captured = samples(engine.work(source));
    assert_ne!(captured, [0.0; 4], "the source rendered into block one");

    engine.route(FRAMES as u64);

    // The destination's own fader and pan then act on what it receives, so the
    // injected block is read from the tap its output stage took before them.
    assert_eq!(
        samples(engine.send_pre(destination)),
        captured,
        "block two starts with block one's captured signal"
    );
    // The tap above is exactly what block one captured, so the destination
    // added no voice of its own: an audio track with no instrument to render.
}

/// The device-format change is one Rust walk: it judges every track's freeze
/// against the new rate, retunes and grows the track's blocks and re-offers the
/// work taps to the sidechain graph. A 44.1 kHz snapshot cannot play at 48 kHz,
/// so it must not survive the walk, while a matching one must.
#[test]
fn preparing_the_graph_drops_a_freeze_the_new_rate_cannot_replay() {
    let mut engine = Engine::new();
    engine.bind();
    let stale = engine.track(11, FRAMES, -1.0);
    let usable = engine.track(12, FRAMES, -1.0);
    engine.publish_freeze(stale, 44_100);
    engine.publish_freeze(usable, 48_000);
    // The engine block owns the device format, so the change is published there
    // and the walk reads it back.
    unsafe { hirari_engine_block_set_device_format(engine.state, 48_000.0, 8) };
    // Publishing a freeze is the store's own write, so the revision only counts
    // what the preparation walk changes.
    let before = engine.marks();

    assert_eq!(
        unsafe { hirari_engine_block_prepare_tracks(engine.state) },
        0,
        "every track prepared"
    );

    assert!(
        !unsafe { hirari_track_frozen_store_is_frozen(engine.frozen(stale)) },
        "a snapshot taken at the old rate is dropped"
    );
    assert!(
        unsafe { hirari_track_frozen_store_is_frozen(engine.frozen(usable)) },
        "a snapshot the new rate can replay is kept"
    );
    assert_eq!(
        unsafe { hirari_track_frozen_store_sample_rate(engine.frozen(usable)) },
        48_000
    );
    assert_eq!(
        engine.marks(),
        before + 3,
        "the dropped snapshot is retired and the layout marked, and every track's          preparation marks the layout the region walk depends on"
    );
    assert_eq!(
        unsafe { hirari_audio_buffer_storage_samples(engine.work(stale)) },
        8,
        "every track's block grows to the new device size"
    );
    assert_eq!(
        unsafe { hirari_audio_buffer_storage_samples(engine.work(usable)) },
        8
    );
}
