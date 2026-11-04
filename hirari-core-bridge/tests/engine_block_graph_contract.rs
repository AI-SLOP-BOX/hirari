//! Contract for the Rust-owned engine graph walk (engine_block_host.rs).
//!
//! The walk is the first half of a device callback: it stages every track,
//! injects the one-block delayed feedback edges, feeds the live driver input,
//! publishes the routing destination table and walks the graph in process order,
//! fanning each eligible node into its buses and the master window. Tracks stay
//! opaque, so the test drives the published operations table and the Rust
//! handles the walk borrows instead of a C++ `Track`.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::bounce_driver::{
    hirari_bounce_session_create, hirari_bounce_session_destroy,
};
use hirari_core_bridge::built_in_midi_instrument::{
    hirari_builtin_midi_instrument_create, hirari_builtin_midi_instrument_destroy,
};
use hirari_core_bridge::bus_audio::{
    hirari_bus_audio_read, hirari_bus_system_bus, hirari_bus_system_create,
    hirari_bus_system_destroy, hirari_bus_system_register,
};
use hirari_core_bridge::channel_strip::{
    hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_set_solo,
};
use hirari_core_bridge::control_room_runtime::{
    hirari_control_room_state_create, hirari_control_room_state_destroy,
};
use hirari_core_bridge::engine_block_host::{
    hirari_engine_block_create, hirari_engine_block_destroy, hirari_engine_block_finish,
    hirari_engine_block_route, hirari_engine_block_set_playhead, hirari_engine_block_set_playing,
    HirariEngineBlockHandles, HirariEngineBlockOps, TRACK_BUS_STATE, TRACK_RUNTIME, TRACK_STRIP,
};
use hirari_core_bridge::engine_input_block::{
    hirari_input_blocks_create, hirari_input_blocks_destroy,
};
use hirari_core_bridge::engine_track_table::{
    hirari_engine_track_insert, hirari_engine_track_publish, hirari_engine_track_table_create,
    hirari_engine_track_table_destroy, HirariEngineTrackOps,
};
use hirari_core_bridge::master_limiter::{
    hirari_master_limiter_create, hirari_master_limiter_destroy,
};
use hirari_core_bridge::master_meter::{hirari_master_meter_create, hirari_master_meter_destroy};
use hirari_core_bridge::metronome_renderer::{hirari_metronome_create, hirari_metronome_destroy};
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_create, hirari_midi_buffer_destroy,
    hirari_midi_buffer_event_count,
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
use hirari_core_bridge::track_runtime::{
    hirari_track_runtime_create, hirari_track_runtime_destroy, hirari_track_runtime_publish_regions,
};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

const FRAMES: u32 = 4;
const BUS_ID: u32 = 0;

/// `Track::Type::Instrument`: the kind whose block the runtime renders with the
/// built-in voice. Any other kind stands for a bus or an audio track with
/// nothing to generate.
const TRACK_TYPE_INSTRUMENT: u32 = 2;
const TRACK_TYPE_AUDIO: u32 = 0;

/// Stand-in for a C++ `Track`: the walk reaches it only for its own storages,
/// exactly as the engine's table does.
struct FakeTrack {
    id: u32,
    runtime: *mut c_void,
    bus: *mut c_void,
    strip: *mut c_void,
}

unsafe extern "C" fn track_id_of(track: *mut c_void) -> u32 {
    unsafe { &*track.cast::<FakeTrack>() }.id
}

/// The table owns a handle per track; here the handle is the `FakeTrack` itself,
/// so a clone is the same pointer and a release frees nothing.
unsafe extern "C" fn owner_clone(owner: *mut c_void) -> *mut c_void {
    owner
}

unsafe extern "C" fn owner_release(_owner: *mut c_void) {}

unsafe extern "C" fn track_type_of(track: *mut c_void) -> u32 {
    if unsafe { &*track.cast::<FakeTrack>() }.bus.is_null() {
        0
    } else {
        3
    }
}

unsafe extern "C" fn bus_id_of(track: *mut c_void) -> u32 {
    if unsafe { &*track.cast::<FakeTrack>() }.bus.is_null() {
        u32::MAX
    } else {
        BUS_ID
    }
}

unsafe extern "C" fn latency_of(_track: *mut c_void) -> u32 {
    0
}

unsafe extern "C" fn apply_pdc(_track: *mut c_void, _samples: u32) {}

static TABLE_OPS: HirariEngineTrackOps = HirariEngineTrackOps {
    id: Some(track_id_of),
    clone_owner: Some(owner_clone),
    destroy_owner: Some(owner_release),
    track_type: Some(track_type_of),
    bus_id: Some(bus_id_of),
    latency: Some(latency_of),
    set_pdc_offset: Some(apply_pdc),
};

unsafe extern "C" fn storage_of(track: *mut c_void, kind: u32) -> *mut c_void {
    let track = unsafe { &*track.cast::<FakeTrack>() };
    match kind {
        TRACK_RUNTIME => track.runtime,
        TRACK_STRIP => track.strip,
        TRACK_BUS_STATE => track.bus,
        _ => ptr::null_mut(),
    }
}

unsafe extern "C" fn block_type_of(track: *mut c_void) -> u8 {
    if unsafe { &*track.cast::<FakeTrack>() }.bus.is_null() {
        0
    } else {
        3
    }
}

unsafe extern "C" fn edit_of(_track: *mut c_void) {}

unsafe extern "C" fn record_of(_track: *mut c_void, _playhead: u64) {}

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

fn samples(handle: *mut c_void) -> [f32; FRAMES as usize] {
    unsafe { *write_planes(handle)[0].cast::<[f32; FRAMES as usize]>() }
}

fn right_samples(handle: *mut c_void) -> [f32; FRAMES as usize] {
    unsafe { *write_planes(handle)[1].cast::<[f32; FRAMES as usize]>() }
}

/// One engine: the bound block state, the handles it borrows and the graph it
/// walks.
struct Engine {
    state: *mut c_void,
    output: *mut c_void,
    tracks: Vec<*mut c_void>,
    work: Vec<*mut c_void>,
    send_pre: Vec<*mut c_void>,
    send_post: Vec<*mut c_void>,
    runtime: Vec<*mut c_void>,
    midi: Vec<*mut c_void>,
    instruments: Vec<*mut c_void>,
    /// `[routing, bounce, input blocks, control room, mixer telemetry,
    /// metronome, limiter, meter, tempo, bus system, sidechain engine]`.
    borrowed: [*mut c_void; 11],
    /// The published graph the walk traverses, as in the engine.
    table: *mut c_void,
}

impl Engine {
    fn new() -> Self {
        Self {
            state: ptr::null_mut(),
            output: storage(2, 8192),
            tracks: Vec::new(),
            work: Vec::new(),
            send_pre: Vec::new(),
            send_post: Vec::new(),
            runtime: Vec::new(),
            midi: Vec::new(),
            instruments: Vec::new(),
            borrowed: [ptr::null_mut(); 11],
            table: ptr::null_mut(),
        }
    }

    fn bind(&mut self) -> &mut Self {
        let ops = HirariEngineBlockOps {
            storage: Some(storage_of),
            track_type: Some(block_type_of),
            begin_edit: Some(edit_of),
            end_edit: Some(edit_of),
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
        self.table = unsafe { hirari_engine_track_table_create(&TABLE_OPS) };
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
        self.state = unsafe { hirari_engine_block_create(handles, &ops) };
        assert!(!self.state.is_null());
        self
    }

    fn routing(&self) -> *mut c_void {
        self.borrowed[0]
    }

    fn bus_system(&self) -> *mut c_void {
        self.borrowed[9]
    }

    /// Adds a track whose work buffer holds `frames` samples. A non-negative
    /// level makes it an instrument track, so the runtime renders its built-in
    /// voice into the block the graph then routes; a negative level publishes a
    /// track with nothing to generate, which is how a bus or an audio track with
    /// no instrument reaches the walk.
    fn track(&mut self, id: u32, frames: u32, level: f32) -> usize {
        self.staged_track(id, frames, level, false)
    }

    fn staged_track(&mut self, id: u32, frames: u32, level: f32, solo: bool) -> usize {
        let work = storage(2, frames);
        // The Send taps are their own buffers: staging empties the work buffer,
        // so a pre-fader Send must survive it.
        let send_pre = storage(2, frames);
        let send_post = storage(2, frames);
        let null = ptr::null_mut::<c_void>();
        let voiced = level >= 0.0;
        let midi = unsafe { hirari_midi_buffer_create() };
        let instrument = if voiced {
            unsafe { hirari_builtin_midi_instrument_create() }
        } else {
            ptr::null_mut()
        };
        // One held note gives the voice something to render, and the runtime
        // consumes the buffer when the block reaches its chain stage. That is
        // how these contracts tell a walked track from a skipped one.
        unsafe { hirari_midi_buffer_add_note_on(midi, 1, 69, 100, 0, 0) };
        let runtime = unsafe {
            hirari_track_runtime_create(
                id,
                if voiced {
                    TRACK_TYPE_INSTRUMENT
                } else {
                    TRACK_TYPE_AUDIO
                },
                null,
                null,
                null,
                null,
                null,
                null,
                work,
                null,
                null,
                send_pre,
                send_post,
                midi,
                null,
                instrument,
            )
        };
        // Solo is strip state, exactly as the engine reads it.
        let strip = unsafe { hirari_channel_strip_create() };
        unsafe { hirari_channel_strip_set_solo(strip, solo) };
        let track = Box::into_raw(Box::new(FakeTrack {
            id,
            runtime,
            bus: ptr::null_mut(),
            strip,
        }));
        self.work.push(work);
        self.send_pre.push(send_pre);
        self.send_post.push(send_post);
        self.runtime.push(runtime);
        self.midi.push(midi);
        self.instruments.push(instrument);
        self.tracks.push(track.cast());
        let slot = self.tracks.len() - 1;
        // The engine publishes the graph after every structural edit; the walk
        // only ever sees the published snapshot.
        unsafe {
            assert!(hirari_engine_track_insert(
                self.table,
                usize::MAX,
                track.cast(),
                track.cast()
            ));
            hirari_engine_track_publish(self.table, 1, true);
        }
        slot
    }

    /// Opens the region gate so the runtime reaches its voice stage.
    fn publish_regions(&self, slot: usize) {
        unsafe { hirari_track_runtime_publish_regions(self.runtime[slot], ptr::null(), 0, 1) };
    }

    /// True once the walk has run this track's block: the runtime hands the
    /// staged note to its chain stage, which consumes the MIDI buffer.
    fn rendered(&self, slot: usize) -> bool {
        slot < self.midi.len() && unsafe { hirari_midi_buffer_event_count(self.midi[slot]) } == 0
    }

    /// Turns one slot into an AUX/BUS track fed by the engine's bus system.
    fn make_bus(&mut self, slot: usize) -> *mut c_void {
        let bus = unsafe {
            assert!(
                hirari_bus_system_register(self.bus_system(), BUS_ID),
                "the test bus must be present so the block start clears it"
            );
            hirari_bus_system_bus(self.bus_system(), BUS_ID)
        };
        assert!(!bus.is_null());
        unsafe { (*self.tracks[slot].cast::<FakeTrack>()).bus = bus };
        bus
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
            hirari_engine_block_route(self.state, self.output, 0, FRAMES, 44_100.0);
        };
    }

    /// Runs the tail half of one block, which is where feedback is captured.
    fn finish(&self, playhead: u64) {
        unsafe {
            hirari_engine_block_set_playhead(self.state, playhead);
            hirari_engine_block_set_playing(self.state, 1);
            hirari_engine_block_finish(self.state, self.output, 0, FRAMES, 44_100.0, 1.0, 0);
        };
    }

    fn master(&self) -> ([f32; FRAMES as usize], [f32; FRAMES as usize]) {
        (samples(self.output), right_samples(self.output))
    }

    fn bus_stage(&self, bus: *mut c_void) -> ([f32; FRAMES as usize], [f32; FRAMES as usize]) {
        let mut left = [0.0_f32; FRAMES as usize];
        let mut right = [0.0_f32; FRAMES as usize];
        assert!(unsafe {
            hirari_bus_audio_read(bus, left.as_mut_ptr(), right.as_mut_ptr(), FRAMES, false)
        });
        (left, right)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            // The block state borrows every other handle, so it is retired first.
            hirari_engine_block_destroy(self.state);
            hirari_engine_track_table_destroy(self.table);
            for handle in self.runtime.drain(..) {
                hirari_track_runtime_destroy(handle);
            }
            for handle in self.work.drain(..) {
                hirari_audio_buffer_storage_destroy(handle);
            }
            for handle in self.send_pre.drain(..) {
                hirari_audio_buffer_storage_destroy(handle);
            }
            for handle in self.send_post.drain(..) {
                hirari_audio_buffer_storage_destroy(handle);
            }
            for handle in self.tracks.drain(..) {
                let track = Box::from_raw(handle.cast::<FakeTrack>());
                hirari_channel_strip_destroy(track.strip);
            }
            for handle in self.instruments.drain(..) {
                hirari_builtin_midi_instrument_destroy(handle);
            }
            for handle in self.midi.drain(..) {
                hirari_midi_buffer_destroy(handle);
            }
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
            hirari_audio_buffer_storage_destroy(self.output);
        }
    }
}

#[test]
fn the_walk_clears_each_track_and_fans_its_block_into_the_master() {
    let mut engine = Engine::new();
    engine.bind();
    let slot = engine.track(3, 8, 0.5);
    engine.publish_regions(slot);
    // Stale audio from a previous block must not reach the master.
    fill(engine.work[slot], &[[9.0; 4], [9.0; 4]]);

    engine.route(0);

    assert!(engine.rendered(slot), "the node ran once");
    assert_eq!(engine.master().0, samples(engine.work[slot]));
    assert_eq!(engine.master().1, right_samples(engine.work[slot]));
    assert_ne!(
        samples(engine.work[slot]),
        [9.0; 4],
        "the tap was staged first"
    );
}

#[test]
fn a_solo_track_is_the_only_node_the_walk_reaches() {
    let mut engine = Engine::new();
    engine.bind();
    let soloed = engine.staged_track(1, 8, 0.25, true);
    let silent = engine.track(2, 8, -0.75);
    engine.publish_regions(soloed);
    engine.publish_regions(silent);

    engine.route(0);

    assert!(engine.rendered(soloed));
    assert!(
        !engine.rendered(silent),
        "a muted-by-solo node never renders"
    );
    assert_eq!(samples(engine.work[silent]), [0.0; 4]);
    assert_eq!(engine.master().0, samples(engine.work[soloed]));
}

#[test]
fn a_track_that_cannot_hold_the_block_is_never_staged_or_walked() {
    let mut engine = Engine::new();
    engine.bind();
    let short = engine.track(4, 1, 0.5);
    engine.publish_regions(short);
    fill(engine.work[short], &[[7.0; 4], [7.0; 4]]);

    engine.route(0);

    assert!(!engine.rendered(short), "the destination table skipped it");
    assert_eq!(samples(engine.work[short]), [7.0; 4]);
    assert_eq!(engine.master().0, [0.0; 4]);
}

#[test]
fn a_pre_fader_send_reaches_the_bus_stage_from_its_own_tap() {
    let mut engine = Engine::new();
    engine.bind();
    // A bus with no voice: the walk loads its pre-FX stage into the work buffer.
    let bus_slot = engine.track(10, 8, -1.0);
    let bus = engine.make_bus(bus_slot);
    // A source with no voice stage either, so every tap keeps the exact samples
    // the test publishes and the staged block stays silent.
    let source_slot = engine.track(11, 8, -1.0);
    fill(engine.send_pre[source_slot], &[[0.5; 4], [-0.5; 4]]);
    fill(engine.send_post[source_slot], &[[0.75; 4], [-0.75; 4]]);
    engine.connect(11, 10, 0.5, true, true);

    engine.route(0);

    let (stage_left, stage_right) = engine.bus_stage(bus);
    assert_eq!(stage_left, [0.25; 4], "half of the pre-fader tap");
    assert_eq!(stage_right, [-0.25; 4]);
    assert!(
        !engine.rendered(source_slot),
        "a Send is not a dry-path route"
    );
}

#[test]
fn a_post_fader_send_reads_the_tap_downstream_of_the_fader() {
    let mut engine = Engine::new();
    engine.bind();
    let bus_slot = engine.track(10, 8, -1.0);
    let bus = engine.make_bus(bus_slot);
    let source_slot = engine.track(11, 8, -1.0);
    fill(engine.send_pre[source_slot], &[[0.5; 4], [-0.5; 4]]);
    fill(engine.send_post[source_slot], &[[0.75; 4], [-0.75; 4]]);
    engine.connect(11, 10, 0.5, true, false);

    engine.route(0);

    let (stage_left, stage_right) = engine.bus_stage(bus);
    assert_eq!(stage_left, [0.375; 4], "half of the post-fader tap");
    assert_eq!(stage_right, [-0.375; 4]);
}

#[test]
fn a_feedback_edge_is_captured_one_block_before_it_is_injected() {
    let mut engine = Engine::new();
    engine.bind();
    let source = engine.track(21, 8, 0.25);
    let destination = engine.track(20, 8, -1.0);
    engine.publish_regions(source);
    engine.add_feedback(21, 20, 1.0);

    engine.route(0);
    engine.finish(0);
    let captured = samples(engine.work[source]);
    assert_ne!(captured, [0.0; 4], "the source rendered into block one");

    engine.route(FRAMES as u64);

    assert_eq!(
        samples(engine.work[destination]),
        captured,
        "block two starts with block one's captured signal"
    );
    assert!(
        !engine.rendered(destination),
        "the destination has no voice"
    );
}
