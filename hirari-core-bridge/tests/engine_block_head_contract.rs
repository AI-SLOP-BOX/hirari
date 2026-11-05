//! Contract for the Rust-owned engine block head (engine_block_host.rs).
//!
//! The head is what runs before the graph is touched: the admission gates that
//! decide whether a block renders at all, the cycle wrap, the device-generation
//! check, the command drain, each track's MIDI input reset and the timeline
//! staging. Tracks are the Rust objects of `engine_track.rs`, so the head writes
//! through the storages they own and the test reads the same values back.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::engine_block_host::{
    hirari_engine_block_begin, hirari_engine_block_create, hirari_engine_block_cycle_start,
    hirari_engine_block_destroy, hirari_engine_block_playhead,
    hirari_engine_block_range_overflowed, hirari_engine_block_set_config_transition,
    hirari_engine_block_set_cycle, hirari_engine_block_set_loop, hirari_engine_block_set_playhead,
    hirari_engine_block_set_playing, hirari_engine_block_set_test_tone, HirariEngineBlockHandles,
};
use hirari_core_bridge::engine_rings::{
    hirari_engine_command_ring_push, hirari_engine_rings_create, hirari_engine_rings_destroy,
    CommandRecord, EngineRings,
};
use hirari_core_bridge::engine_track::{
    hirari_layout_revision_create, hirari_layout_revision_current, hirari_layout_revision_release,
    hirari_track_get_volume, hirari_track_is_muted, hirari_track_new, hirari_track_prepare,
    hirari_track_storage, STORAGE_PLUGIN_MIDI, TRACK_TYPE_AUDIO,
};
use hirari_core_bridge::engine_track_table::{
    hirari_engine_track_insert, hirari_engine_track_publish, hirari_engine_track_table_create,
    hirari_engine_track_table_destroy,
};
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_event_count,
};
use std::ffi::c_void;
use std::ptr;

const FRAMES: u32 = 4;
const SAMPLE_RATE: f64 = 44_100.0;
/// The device configuration generation every published snapshot in these tests
/// carries, matching the one `hirari_engine_block_create` starts the transport at.
const GENERATION: u64 = 1;
/// A track prepared for more samples than any block these contracts walk.
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
    revision: *mut c_void,
    tracks: Vec<*mut c_void>,
    /// The revision value the engine started at, so the edit count is relative.
    start: u64,
}

impl Engine {
    fn new() -> Self {
        let revision = hirari_layout_revision_create();
        let start = unsafe { hirari_layout_revision_current(revision) };
        let table = unsafe { hirari_engine_track_table_create() };
        let rings = hirari_engine_rings_create();
        let handles = HirariEngineBlockHandles {
            track_table: table,
            rings: rings.cast(),
            ..HirariEngineBlockHandles::default()
        };
        let state = unsafe { hirari_engine_block_create(handles) };
        assert!(!state.is_null());
        Self {
            state,
            output: storage(2, 8192),
            table,
            rings: rings.cast(),
            revision,
            tracks: Vec::new(),
            start,
        }
    }

    /// Adds one published track, the same double-buffered snapshot the engine
    /// hands the head after a structural edit.
    fn track(&mut self, id: u32) -> usize {
        let name = b"test track";
        let mut track = ptr::null_mut();
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
        assert!(!track.is_null());
        assert_eq!(
            unsafe { hirari_track_prepare(track, SAMPLE_RATE, CAPACITY) },
            0,
            "the test track must prepare"
        );
        self.tracks.push(track);
        let slot = self.tracks.len() - 1;
        // The table owns the handle from here, so destroying it releases the
        // track and every storage it owns.
        unsafe {
            assert!(hirari_engine_track_insert(self.table, usize::MAX, owner));
            hirari_engine_track_publish(self.table, GENERATION, true);
        }
        slot
    }

    fn handle(&self, slot: usize, kind: u32) -> *mut c_void {
        unsafe { hirari_track_storage(self.tracks[slot], kind) }
    }

    /// How many control edits this layout has seen. One edit marks it twice: the
    /// freeze drop before the store write and the publication after it. Building
    /// a track is itself an edit, so a contract counts from its own baseline.
    fn edits(&self) -> u64 {
        let marks = unsafe { hirari_layout_revision_current(self.revision) } - self.start;
        marks / 2
    }

    fn gain(&self, slot: usize) -> f32 {
        unsafe { hirari_track_get_volume(self.tracks[slot]) }
    }

    fn muted(&self, slot: usize) -> bool {
        unsafe { hirari_track_is_muted(self.tracks[slot]) }
    }

    fn midi_events(&self, slot: usize) -> usize {
        unsafe { hirari_midi_buffer_event_count(self.handle(slot, STORAGE_PLUGIN_MIDI)) }
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
        let admitted =
            unsafe { hirari_engine_block_begin(self.state, self.output, 0, frames, &mut handled) };
        (admitted, handled)
    }

    fn playhead(&self) -> u64 {
        unsafe { hirari_engine_block_playhead(self.state) }
    }

    fn start_playback(&self) {
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
            hirari_layout_revision_release(self.revision);
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
    engine.start_playback();

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
    engine.start_playback();
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
    engine.start_playback();
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
    engine.start_playback();
    let slot = engine.track(7);
    // A fader move admitted before the device rebuild, and one for this format.
    engine.queue(0, 7, 4.0, GENERATION - 1);
    engine.queue(2, 7, 1.0, GENERATION - 1);
    engine.queue(0, 7, 0.25, GENERATION);
    let before = engine.edits();

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 1);
    assert_eq!(
        engine.edits() - before,
        1,
        "a command that waited through a rebuild never applies"
    );
    assert_eq!(engine.gain(slot), 0.25, "and the value is clamped");
    assert!(!engine.muted(slot), "no mute command applied");
}

#[test]
fn a_command_for_a_retired_track_is_dropped() {
    let mut engine = Engine::new();
    engine.start_playback();
    engine.track(7);
    engine.queue(0, 99, 0.5, GENERATION);

    assert_eq!(engine.begin(FRAMES).0, 1);
    assert_eq!(samples(engine.output), [0.0; 4]);
}

#[test]
fn the_head_empties_each_tracks_own_midi_input() {
    let mut engine = Engine::new();
    engine.start_playback();
    let slot = engine.track(7);
    unsafe {
        hirari_midi_buffer_add_note_on(engine.handle(slot, STORAGE_PLUGIN_MIDI), 1, 60, 90, 0, 0)
    };
    assert_eq!(engine.midi_events(slot), 1);

    let (admitted, _) = engine.begin(FRAMES);

    assert_eq!(admitted, 1);
    assert_eq!(
        engine.midi_events(slot),
        0,
        "a track cannot replay last block's events"
    );

    // A stopped head clears the inputs as well, before it declines the block.
    unsafe {
        hirari_midi_buffer_add_note_on(engine.handle(slot, STORAGE_PLUGIN_MIDI), 1, 61, 90, 0, 0)
    };
    unsafe { hirari_engine_block_set_playing(engine.state, 0) };
    assert_eq!(engine.begin(FRAMES).0, 0);
    assert_eq!(engine.midi_events(slot), 0);
}

#[test]
fn a_saturated_playhead_is_reported_instead_of_wrapping() {
    let mut engine = Engine::new();
    engine.start_playback();
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
