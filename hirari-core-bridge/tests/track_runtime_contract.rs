use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::built_in_midi_instrument::{
    hirari_builtin_midi_instrument_create, hirari_builtin_midi_instrument_destroy,
};
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_create, hirari_midi_buffer_destroy,
    hirari_midi_buffer_event_count,
};
use hirari_core_bridge::track_frozen_store::{
    hirari_track_frozen_store_create, hirari_track_frozen_store_destroy,
    hirari_track_frozen_store_publish,
};
use hirari_core_bridge::track_midi_articulation::{
    hirari_track_midi_articulation_create, hirari_track_midi_articulation_destroy,
};
use hirari_core_bridge::track_runtime::{
    hirari_track_process_block, hirari_track_runtime_create, hirari_track_runtime_destroy,
    hirari_track_runtime_eq, hirari_track_runtime_pdc_requested,
    hirari_track_runtime_publish_regions, hirari_track_runtime_set_delays,
    hirari_track_runtime_set_eq, hirari_track_runtime_set_muted, hirari_track_runtime_set_spatial,
    hirari_track_runtime_set_transport, hirari_track_runtime_spatial,
    hirari_track_runtime_track_delay_samples, hirari_track_runtime_transport_bpm,
};
use std::ffi::c_void;

const FRAMES: usize = 4;
const MAX_DELAY_SAMPLES: u32 = 8192;
/// `Track::Type::Instrument`, the track kind that gets a built-in voice.
const TRACK_TYPE_INSTRUMENT: u32 = 2;

fn block() -> *mut c_void {
    unsafe {
        let storage = hirari_audio_buffer_storage_create();
        assert!(
            hirari_audio_buffer_storage_reserve_buffer(storage, 2, FRAMES as u32) == 0
                && hirari_audio_buffer_storage_resize(storage, 2, FRAMES as u32),
            "test block storage must be reservable"
        );
        storage
    }
}

fn planes(storage: *mut c_void) -> [*mut f32; 2] {
    unsafe { *hirari_audio_buffer_storage_array_write(storage).cast::<[*mut f32; 2]>() }
}

fn samples(pointer: *mut f32) -> [f32; FRAMES] {
    unsafe { *pointer.cast::<[f32; FRAMES]>() }
}

struct Fixture {
    runtime: *mut c_void,
    work: *mut c_void,
    pre_insert: *mut c_void,
    midi: *mut c_void,
    articulation: *mut c_void,
    instrument: *mut c_void,
    frozen: Option<*mut c_void>,
}

impl Fixture {
    fn new() -> Self {
        Self::bind(std::ptr::null_mut())
    }

    /// `frozen` is a publication store already holding one rendered block.
    fn bind(frozen: *mut c_void) -> Self {
        let null = std::ptr::null_mut::<c_void>();
        unsafe {
            let work = block();
            let pre_insert = block();
            let midi = hirari_midi_buffer_create();
            let articulation = hirari_track_midi_articulation_create();
            let instrument = hirari_builtin_midi_instrument_create();
            Self {
                // Only the block taps, the MIDI stores, the built-in voice and
                // the freeze store are bound: the output stage stops at its own
                // handle guards, which keeps this contract on the pipeline
                // before the chain.
                runtime: hirari_track_runtime_create(
                    7,
                    TRACK_TYPE_INSTRUMENT,
                    null,
                    null,
                    null,
                    frozen,
                    null,
                    null,
                    work,
                    pre_insert,
                    null,
                    null,
                    null,
                    midi,
                    articulation,
                    instrument,
                ),
                work,
                pre_insert,
                midi,
                articulation,
                instrument,
                frozen: (frozen != null).then_some(frozen),
            }
        }
    }

    /// Opens the region gate with an empty published table.
    fn with_regions(&self) {
        unsafe { hirari_track_runtime_publish_regions(self.runtime, std::ptr::null(), 0, 1) };
    }

    /// One held note is enough for the voice to change the block.
    fn stage_note(&self) {
        unsafe { hirari_midi_buffer_add_note_on(self.midi, 1, 69, 100, 0, 0) };
    }

    fn pending_events(&self) -> usize {
        unsafe { hirari_midi_buffer_event_count(self.midi) }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            hirari_track_runtime_destroy(self.runtime);
            hirari_audio_buffer_storage_destroy(self.work);
            hirari_audio_buffer_storage_destroy(self.pre_insert);
            hirari_midi_buffer_destroy(self.midi);
            hirari_track_midi_articulation_destroy(self.articulation);
            hirari_builtin_midi_instrument_destroy(self.instrument);
            if let Some(frozen) = self.frozen {
                hirari_track_frozen_store_destroy(frozen);
            }
        }
    }
}

#[test]
fn midi_preparation_runs_before_the_pre_insert_snapshot() {
    let fixture = Fixture::new();
    fixture.with_regions();
    fixture.stage_note();
    let [input_left, input_right] = planes(fixture.work);
    let [snapshot_left, snapshot_right] = planes(fixture.pre_insert);
    unsafe {
        for index in 0..FRAMES {
            *input_left.add(index) = 1.0;
            *input_right.add(index) = -1.0;
        }
        *snapshot_left = 9.0;
        *snapshot_right = 9.0;
    }

    assert!(
        unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 0) },
        "the block reaches the insert stage"
    );
    let rendered = samples(input_left);
    // The voice accumulates onto the block instead of replacing it, and its
    // phase starts at zero so the first frame is still untouched signal.
    assert_eq!(
        rendered[0], 1.0,
        "the voice adds silence to the first frame"
    );
    assert!(
        rendered[1] > 1.0 && rendered[2] > rendered[1] && rendered[3] > rendered[2],
        "the built-in voice renders into the block being prepared"
    );
    assert_eq!(
        samples(snapshot_left),
        rendered,
        "the tap follows the voice"
    );
    assert_eq!(
        samples(snapshot_right),
        samples(input_right),
        "the tap follows the voice"
    );
    assert_eq!(fixture.pending_events(), 0, "the chain consumed the block");
}

#[test]
fn a_muted_track_clears_the_block_and_skips_midi_preparation() {
    let fixture = Fixture::new();
    fixture.with_regions();
    fixture.stage_note();
    let [left, right] = planes(fixture.work);
    unsafe {
        hirari_track_runtime_set_muted(fixture.runtime, 1);
        *left = 1.0;
        *right = -1.0;
    }

    assert!(
        !unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 0) },
        "the mute is handled inside the runtime"
    );
    assert_eq!(samples(left), [0.0; 4], "the mute clears the whole block");
    assert_eq!(samples(right), [0.0; 4]);
    assert_eq!(fixture.pending_events(), 1, "the note was never rendered");
}

#[test]
fn a_block_without_published_regions_never_reaches_the_midi_stage() {
    let fixture = Fixture::new();
    fixture.stage_note();
    let [left, right] = planes(fixture.work);
    unsafe {
        *left = 1.0;
        *right = -1.0;
    }

    assert!(
        !unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 1) },
        "the region gate closes the block"
    );
    assert_eq!(samples(left), [0.0; 4], "the requested clear still runs");
    assert_eq!(samples(right), [0.0; 4]);
    assert_eq!(fixture.pending_events(), 1, "the note was never rendered");
}

#[test]
fn a_published_freeze_is_read_through_its_storage_handle() {
    let source = block();
    let [source_left, source_right] = planes(source);
    unsafe {
        for (index, value) in [0.5_f32, -0.25, 0.75, -1.0].into_iter().enumerate() {
            *source_left.add(index) = value;
            *source_right.add(index) = -value;
        }
    }
    let store = hirari_track_frozen_store_create();
    // The publication takes the render storage, so the store's teardown is what
    // releases `source` here.
    assert!(unsafe {
        hirari_track_frozen_store_publish(
            store,
            source,
            2,
            FRAMES as u64,
            FRAMES as u64,
            48_000,
            0,
            0,
            std::ptr::null(),
            0,
        )
    });

    let fixture = Fixture::bind(store);
    fixture.stage_note();
    let [left, right] = planes(fixture.work);
    unsafe { *left = 3.0 };
    assert!(
        !unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 0) },
        "the frozen render replaces the live pipeline"
    );
    assert_eq!(samples(left), [0.5, -0.25, 0.75, -1.0]);
    assert_eq!(samples(right), [-0.5, 0.25, -0.75, 1.0]);
    assert_eq!(
        fixture.pending_events(),
        1,
        "the freeze skips the live MIDI"
    );
}

#[test]
fn delays_are_bounded_by_the_shared_delay_line() {
    let fixture = Fixture::new();
    unsafe {
        hirari_track_runtime_set_delays(fixture.runtime, u32::MAX, 9_000);
        assert_eq!(
            hirari_track_runtime_pdc_requested(fixture.runtime),
            MAX_DELAY_SAMPLES
        );
        assert_eq!(
            hirari_track_runtime_track_delay_samples(fixture.runtime),
            MAX_DELAY_SAMPLES
        );
    }
}

#[test]
fn transport_spatial_and_eq_state_round_trips() {
    let fixture = Fixture::new();
    unsafe {
        hirari_track_runtime_set_transport(fixture.runtime, 140.0, 1);
        assert_eq!(hirari_track_runtime_transport_bpm(fixture.runtime), 140.0);
        hirari_track_runtime_set_spatial(fixture.runtime, 0.25, -0.5, 1.0);
        assert_eq!(hirari_track_runtime_spatial(fixture.runtime, 0), 0.25);
        assert_eq!(hirari_track_runtime_spatial(fixture.runtime, 1), -0.5);
        assert_eq!(hirari_track_runtime_spatial(fixture.runtime, 2), 1.0);
        hirari_track_runtime_set_eq(fixture.runtime, 1.5, 0.5);
        assert_eq!(hirari_track_runtime_eq(fixture.runtime, 0), 1.5);
        assert_eq!(hirari_track_runtime_eq(fixture.runtime, 1), 0.5);
    }
}
