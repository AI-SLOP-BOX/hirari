//! Contract for the Rust-owned engine block head (engine_block_host.rs).
//!
//! The head is what runs before the graph is touched: the admission gates that
//! decide whether a block renders at all, the cycle wrap, the device-generation
//! check, the command drain, each track's MIDI input reset and the timeline
//! staging. Tracks stay opaque, so the test drives the published operations
//! table and the handles the head borrows instead of a C++ `Track`.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::channel_strip::{
    hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_gain,
};
use hirari_core_bridge::engine_block_host::{
    hirari_engine_block_begin, hirari_engine_block_create, hirari_engine_block_cycle_start,
    hirari_engine_block_destroy, hirari_engine_block_playhead,
    hirari_engine_block_range_overflowed, hirari_engine_block_set_config_transition,
    hirari_engine_block_set_cycle, hirari_engine_block_set_loop, hirari_engine_block_set_playhead,
    hirari_engine_block_set_playing, hirari_engine_block_set_test_tone, HirariEngineBlockHandles,
    HirariEngineBlockOps, TRACK_PLUGIN_MIDI, TRACK_RUNTIME, TRACK_STRIP,
};
use hirari_core_bridge::engine_rings::{
    hirari_engine_command_ring_push, hirari_engine_rings_create, hirari_engine_rings_destroy,
    CommandRecord, EngineRings,
};
use hirari_core_bridge::engine_track_table::{
    hirari_engine_track_insert, hirari_engine_track_publish, hirari_engine_track_table_create,
    hirari_engine_track_table_destroy, HirariEngineTrackOps,
};
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_create, hirari_midi_buffer_destroy,
    hirari_midi_buffer_event_count,
};
use hirari_core_bridge::track_runtime::{
    hirari_track_runtime_create, hirari_track_runtime_destroy, hirari_track_runtime_is_muted,
};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

const FRAMES: u32 = 4;
const SAMPLE_RATE: f64 = 44_100.0;
/// The device configuration generation every published snapshot in these tests
/// carries, matching the one `hirari_engine_block_create` starts the transport at.
const GENERATION: u64 = 1;

/// Stand-in for a C++ `Track`: the block reaches it only for a storage handle,
/// so the fader, pan and mute the head writes land in the real Rust storages and
/// the edit counter is the assertion for the control protocol around them.
struct FakeTrack {
    id: u32,
    midi: *mut c_void,
    strip: *mut c_void,
    runtime: *mut c_void,
    edits: AtomicUsize,
}

unsafe extern "C" fn track_id_of(track: *mut c_void) -> u32 {
    unsafe { &*track.cast::<FakeTrack>() }.id
}

/// The table owns one handle per track; here it is the `FakeTrack` itself, so a
/// clone is the same pointer and a release frees nothing.
unsafe extern "C" fn owner_clone(owner: *mut c_void) -> *mut c_void {
    owner
}

unsafe extern "C" fn owner_release(_owner: *mut c_void) {}

unsafe extern "C" fn track_type_of(_track: *mut c_void) -> u32 {
    0
}

unsafe extern "C" fn bus_id_of(_track: *mut c_void) -> u32 {
    u32::MAX
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
        TRACK_PLUGIN_MIDI => track.midi,
        _ => ptr::null_mut(),
    }
}

unsafe extern "C" fn type_of(_track: *mut c_void) -> u8 {
    0
}

unsafe extern "C" fn begin_edit_of(track: *mut c_void) {
    unsafe { &*track.cast::<FakeTrack>() }
        .edits
        .fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn end_edit_of(_track: *mut c_void) {}

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

/// One engine: the bound head, the track table it reads and the master window
/// it silences.
struct Engine {
    state: *mut c_void,
    output: *mut c_void,
    table: *mut c_void,
    rings: *mut c_void,
    tracks: Vec<*mut c_void>,
}

impl Engine {
    fn new() -> Self {
        let ops = HirariEngineBlockOps {
            storage: Some(storage_of),
            track_type: Some(type_of),
            begin_edit: Some(begin_edit_of),
            end_edit: Some(end_edit_of),
            record: Some(record_of),
        };
        let table = unsafe { hirari_engine_track_table_create(&TABLE_OPS) };
        let rings = hirari_engine_rings_create();
        let handles = HirariEngineBlockHandles {
            track_table: table,
            rings: rings.cast(),
            ..HirariEngineBlockHandles::default()
        };
        let state = unsafe { hirari_engine_block_create(handles, &ops) };
        assert!(!state.is_null());
        Self {
            state,
            output: storage(2, 8192),
            table,
            rings: rings.cast(),
            tracks: Vec::new(),
        }
    }

    /// Adds one published track, the same double-buffered snapshot the engine
    /// hands the head after a structural edit.
    fn track(&mut self, id: u32) -> usize {
        let null = ptr::null_mut::<c_void>();
        let runtime = unsafe {
            hirari_track_runtime_create(
                id, null, null, null, null, null, null, null, null, null, null, null, null, None,
                null,
            )
        };
        let track = Box::into_raw(Box::new(FakeTrack {
            id,
            midi: hirari_midi_buffer_create(),
            strip: unsafe { hirari_channel_strip_create() },
            runtime,
            edits: AtomicUsize::new(0),
        }));
        self.tracks.push(track.cast());
        let slot = self.tracks.len() - 1;
        unsafe {
            assert!(hirari_engine_track_insert(
                self.table,
                usize::MAX,
                track.cast(),
                track.cast()
            ));
            hirari_engine_track_publish(self.table, GENERATION, true);
        }
        slot
    }

    /// How many control edits the head ran, counting only the protocol around a
    /// command it actually applied.
    fn edits(&self, slot: usize) -> usize {
        unsafe { &*self.tracks[slot].cast::<FakeTrack>() }
            .edits
            .load(Ordering::Relaxed)
    }

    fn gain(&self, slot: usize) -> f32 {
        unsafe {
            hirari_channel_strip_gain(
                (*self.tracks[slot].cast::<FakeTrack>()).strip as *const c_void,
            )
        }
    }

    fn muted(&self, slot: usize) -> bool {
        unsafe { hirari_track_runtime_is_muted((*self.tracks[slot].cast::<FakeTrack>()).runtime) }
    }

    fn midi_events(&self, slot: usize) -> usize {
        let midi = unsafe { (*self.tracks[slot].cast::<FakeTrack>()).midi };
        unsafe { hirari_midi_buffer_event_count(midi) }
    }

    fn queue(&self, type_: u32, tid: u32, val: f32, generation: u64) {
        let record = CommandRecord {
            type_,
            tid,
            val,
            ts: 0,
            expected_audio_generation: generation,
        };
        assert!(
            unsafe { hirari_engine_command_ring_push(self.rings.cast::<EngineRings>(), &record) },
            "the test ring holds one block of commands"
        );
    }

    /// Runs the head with the transport the test asked for, reporting the clamped
    /// block length the rest of the callback would have to use.
    fn begin(&self, frames: u32) -> (u8, u32) {
        let mut handled = 0;
        let admitted = unsafe {
            hirari_engine_block_begin(
                self.state,
                self.output,
                0,
                frames,
                SAMPLE_RATE,
                0,
                &mut handled,
            )
        };
        (admitted, handled)
    }

    fn playhead(&self) -> u64 {
        unsafe { hirari_engine_block_playhead(self.state) }
    }

    fn start(&self) {
        unsafe {
            hirari_engine_block_set_playing(self.state, 1);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            // The block state borrows the table and the rings, so it goes first.
            hirari_engine_block_destroy(self.state);
            hirari_engine_track_table_destroy(self.table);
            hirari_engine_rings_destroy(self.rings.cast::<EngineRings>());
            for handle in self.tracks.drain(..) {
                let track = Box::from_raw(handle.cast::<FakeTrack>());
                hirari_midi_buffer_destroy(track.midi);
                hirari_channel_strip_destroy(track.strip);
                hirari_track_runtime_destroy(track.runtime);
            }
            hirari_audio_buffer_storage_destroy(self.output);
        }
    }
}

#[test]
fn a_stopped_engine_silences_the_window_and_renders_nothing() {
    let engine = Engine::new();
    // Stale audio from a previous block must never leave a stopped engine.
    fill(engine.output, &[[9.0; 4], [9.0; 4]]);

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 0, "the head declined the block");
    assert_eq!(samples(engine.output), [0.0; 4]);
    assert_eq!(right_samples(engine.output), [0.0; 4]);
}

#[test]
fn a_block_beyond_the_buffer_is_clamped_and_an_empty_one_declined() {
    let engine = Engine::new();
    engine.start();

    let (admitted, frames) = engine.begin(0);
    assert_eq!((admitted, frames), (0, 0), "nothing to render");

    let (admitted, frames) = engine.begin(20_000);
    assert_eq!(
        (admitted, frames),
        (0, 0),
        "a block larger than the engine's cap is silenced, not clamped"
    );
    assert_eq!(samples(engine.output), [0.0; 4]);
}

#[test]
fn a_playhead_past_the_cycle_restarts_at_the_cycle_start() {
    let engine = Engine::new();
    engine.start();
    assert_eq!(
        unsafe { hirari_engine_block_set_cycle(engine.state, 1_000, 2_000, 1) },
        1
    );
    unsafe { hirari_engine_block_set_playhead(engine.state, 5_000) };

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 1);
    assert_eq!(
        engine.playhead(),
        1_000,
        "the head rewinds before the walk reads the position"
    );

    // An empty cycle is refused and leaves the transport untouched.
    assert_eq!(
        unsafe { hirari_engine_block_set_cycle(engine.state, 2_000, 2_000, 1) },
        0
    );
    assert_eq!(
        unsafe { hirari_engine_block_cycle_start(engine.state) },
        1_000
    );

    // Without looping the position is simply carried through.
    unsafe { hirari_engine_block_set_loop(engine.state, 0) };
    unsafe { hirari_engine_block_set_playhead(engine.state, 5_000) };
    let (admitted, _) = engine.begin(FRAMES);
    assert_eq!(admitted, 1);
    assert_eq!(engine.playhead(), 5_000, "only the tail advances it");
}

#[test]
fn a_config_transition_silences_a_running_engine() {
    let engine = Engine::new();
    engine.start();
    fill(engine.output, &[[8.0; 4], [8.0; 4]]);
    unsafe { hirari_engine_block_set_config_transition(engine.state, 1) };

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 0, "the buffers being replaced are not traversed");
    assert_eq!(samples(engine.output), [0.0; 4]);

    unsafe { hirari_engine_block_set_config_transition(engine.state, 0) };
    assert_eq!(engine.begin(FRAMES).0, 1);
}

#[test]
fn only_commands_for_the_published_generation_apply() {
    let mut engine = Engine::new();
    engine.start();
    let slot = engine.track(7);
    // A fader move admitted before the device rebuild, and one for this format.
    engine.queue(0, 7, 4.0, GENERATION - 1);
    engine.queue(2, 7, 1.0, GENERATION - 1);
    engine.queue(0, 7, 0.25, GENERATION);

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 1);
    assert_eq!(
        engine.edits(slot),
        1,
        "a command that waited through a rebuild never applies"
    );
    assert_eq!(engine.gain(slot), 0.25, "and the value is clamped");
    assert!(!engine.muted(slot), "no mute command applied");
}

#[test]
fn a_command_for_a_retired_track_is_dropped() {
    let mut engine = Engine::new();
    engine.start();
    engine.track(7);
    engine.queue(0, 99, 0.5, GENERATION);

    assert_eq!(engine.begin(FRAMES).0, 1);
    assert_eq!(samples(engine.output), [0.0; 4]);
}

#[test]
fn the_head_empties_each_tracks_own_midi_input() {
    let mut engine = Engine::new();
    engine.start();
    let slot = engine.track(7);
    let midi = unsafe { (*engine.tracks[slot].cast::<FakeTrack>()).midi };
    unsafe { hirari_midi_buffer_add_note_on(midi, 1, 60, 90, 0, 0) };
    assert_eq!(engine.midi_events(slot), 1);

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 1);
    assert_eq!(
        engine.midi_events(slot),
        0,
        "a track cannot replay last block's events"
    );

    // A stopped head clears the inputs as well, before it declines the block.
    unsafe { hirari_midi_buffer_add_note_on(midi, 1, 61, 90, 0, 0) };
    unsafe { hirari_engine_block_set_playing(engine.state, 0) };
    assert_eq!(engine.begin(FRAMES).0, 0);
    assert_eq!(engine.midi_events(slot), 0);
}

#[test]
fn a_saturated_playhead_is_reported_instead_of_wrapping() {
    let mut engine = Engine::new();
    engine.start();
    engine.track(7);
    unsafe { hirari_engine_block_set_playhead(engine.state, u64::MAX - 2) };

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 0, "the block is dropped, never wrapped around");
    assert_eq!(
        engine.playhead(),
        u64::MAX - 2,
        "and the position is left alone"
    );
    assert_eq!(
        unsafe { hirari_engine_block_range_overflowed(engine.state) },
        1,
        "the overflow is sticky for the control thread"
    );
}

#[test]
fn the_test_tone_primes_the_callback_while_stopped() {
    let engine = Engine::new();
    unsafe { hirari_engine_block_set_test_tone(engine.state, 1) };

    let (admitted, frames) = engine.begin(FRAMES);

    assert_eq!((admitted, frames), (1, FRAMES));
    assert!(
        samples(engine.output).iter().any(|value| *value != 0.0),
        "the tone is written into the master window"
    );
    assert_eq!(engine.playhead(), 0, "a tone is not transport motion");

    unsafe { hirari_engine_block_set_test_tone(engine.state, 0) };
    assert_eq!(engine.begin(FRAMES).0, 0, "and the engine goes quiet again");
}
