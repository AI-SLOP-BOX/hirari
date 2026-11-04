use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::track_frozen_store::{
    hirari_track_frozen_store_create, hirari_track_frozen_store_destroy,
    hirari_track_frozen_store_publish,
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

struct MidiStage {
    calls: usize,
}

/// The runtime hands over the block planes it already resolved, so the MIDI
/// stage writes the built-in voice directly into the block being rendered.
unsafe extern "C" fn prepare_midi(
    context: *mut c_void,
    _playhead: u64,
    frames: u32,
    left: *mut f32,
    right: *mut f32,
) {
    let stage = unsafe { &mut *context.cast::<MidiStage>() };
    stage.calls += 1;
    for frame in 0..frames as usize {
        unsafe {
            *left.add(frame) = 0.25;
            *right.add(frame) = -0.5;
        }
    }
}

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
    frozen: Option<*mut c_void>,
    stage: *mut MidiStage,
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
            let stage = Box::into_raw(Box::new(MidiStage { calls: 0 }));
            Self {
                // Only the block taps, the MIDI stage and the freeze store are
                // bound: the output stage stops at its own handle guards, which
                // keeps this contract on the pipeline before the chain.
                runtime: hirari_track_runtime_create(
                    7,
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
                    null,
                    Some(prepare_midi),
                    stage.cast(),
                ),
                work,
                pre_insert,
                frozen: (frozen != null).then_some(frozen),
                stage,
            }
        }
    }

    /// Opens the region gate with an empty published table.
    fn with_regions(&self) {
        unsafe { hirari_track_runtime_publish_regions(self.runtime, std::ptr::null(), 0, 1) };
    }

    fn calls(&self) -> usize {
        unsafe { (*self.stage).calls }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            hirari_track_runtime_destroy(self.runtime);
            hirari_audio_buffer_storage_destroy(self.work);
            hirari_audio_buffer_storage_destroy(self.pre_insert);
            if let Some(frozen) = self.frozen {
                hirari_track_frozen_store_destroy(frozen);
            }
            drop(Box::from_raw(self.stage));
        }
    }
}

#[test]
fn midi_preparation_runs_before_the_pre_insert_snapshot() {
    let fixture = Fixture::new();
    fixture.with_regions();
    let [snapshot_left, snapshot_right] = planes(fixture.pre_insert);
    unsafe {
        *snapshot_left = 9.0;
        *snapshot_right = 9.0;
    }

    assert!(
        unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 0) },
        "the block reaches the insert stage"
    );
    assert_eq!(fixture.calls(), 1);
    assert_eq!(
        samples(snapshot_left),
        [0.25; 4],
        "the tap follows the voice"
    );
    assert_eq!(
        samples(snapshot_right),
        [-0.5; 4],
        "the tap follows the voice"
    );
}

#[test]
fn a_muted_track_clears_the_block_and_skips_midi_preparation() {
    let fixture = Fixture::new();
    fixture.with_regions();
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
    assert_eq!(fixture.calls(), 0);
}

#[test]
fn a_block_without_published_regions_never_reaches_the_midi_stage() {
    let fixture = Fixture::new();
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
    assert_eq!(fixture.calls(), 0);
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
    let [left, right] = planes(fixture.work);
    unsafe { *left = 3.0 };
    assert!(
        !unsafe { hirari_track_process_block(fixture.runtime, fixture.work, FRAMES as u32, 0, 0) },
        "the frozen render replaces the live pipeline"
    );
    assert_eq!(samples(left), [0.5, -0.25, 0.75, -1.0]);
    assert_eq!(samples(right), [-0.5, 0.25, -0.75, 1.0]);
    assert_eq!(fixture.calls(), 0);
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
