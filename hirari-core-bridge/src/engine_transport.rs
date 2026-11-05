//! Transport, preview and VCA assignment served straight from the Rust-owned
//! stores. The deleted C++ `HirariUnifiedEngine` methods were one-line
//! forwarders onto this engine-block store, plus the stop-time quiescence the
//! device callback needs: join the in-flight callbacks, then trim each live
//! track's retired region generations on the control thread.

use crate::audio_driver_host;
use crate::engine_block_host::{self, hirari_engine_block_track_table};
use crate::engine_track_table;
use crate::ffi::{AudioEngine, HirariUnifiedEngine};
use crate::realtime_vca_manager::hirari_vca_assign_track;
use crate::region_store;
use crate::track_session::{self, TrackSession};
use std::ffi::c_void;

fn block(core: &HirariUnifiedEngine) -> *mut c_void {
    core.engine_block_state() as *mut c_void
}

fn address(core: &HirariUnifiedEngine) -> *mut c_void {
    core as *const HirariUnifiedEngine as *mut c_void
}

fn sampler(core: &HirariUnifiedEngine) -> *mut c_void {
    core.preview_sampler() as *mut c_void
}

pub fn is_playing(core: &HirariUnifiedEngine) -> bool {
    unsafe { engine_block_host::hirari_engine_block_is_playing(block(core) as *const c_void) != 0 }
}

/// The audio-range overflow flag published by the block's render gate.
pub fn range_overflowed(core: &HirariUnifiedEngine) -> bool {
    unsafe {
        engine_block_host::hirari_engine_block_range_overflowed(block(core) as *const c_void) != 0
    }
}

pub fn playhead(core: &HirariUnifiedEngine) -> u64 {
    unsafe { engine_block_host::hirari_engine_block_playhead(block(core) as *const c_void) }
}

pub fn set_playhead(core: &HirariUnifiedEngine, pos: u64) {
    unsafe { engine_block_host::hirari_engine_block_set_playhead(block(core), pos) };
}

pub fn loop_enabled(core: &HirariUnifiedEngine) -> bool {
    unsafe { engine_block_host::hirari_engine_block_loop_enabled(block(core) as *const c_void) != 0 }
}

pub fn set_loop(core: &HirariUnifiedEngine, enabled: bool) {
    unsafe {
        engine_block_host::hirari_engine_block_set_loop(block(core), u8::from(enabled));
    }
}

pub fn cycle_start(core: &HirariUnifiedEngine) -> u64 {
    unsafe { engine_block_host::hirari_engine_block_cycle_start(block(core) as *const c_void) }
}

pub fn cycle_end(core: &HirariUnifiedEngine) -> u64 {
    unsafe { engine_block_host::hirari_engine_block_cycle_end(block(core) as *const c_void) }
}

pub fn set_cycle_range(
    core: &HirariUnifiedEngine,
    start_sample: u64,
    end_sample: u64,
    enabled: bool,
) -> bool {
    unsafe {
        engine_block_host::hirari_engine_block_set_cycle(
            block(core),
            start_sample,
            end_sample,
            u8::from(enabled),
        ) != 0
    }
}

pub fn set_test_tone(core: &HirariUnifiedEngine, enabled: bool) {
    unsafe {
        engine_block_host::hirari_engine_block_set_test_tone(block(core), u8::from(enabled));
    }
}

/// Control-thread quiescence: the deleted C++ join yielded until the device
/// callback retired its in-flight count; the engine still owns that counter.
fn join_audio_callbacks(core: &HirariUnifiedEngine) {
    core.join_audio_callbacks();
}

/// The stop-time trim: every live track's retired region generations are
/// released on the control thread, once no callback can be holding them.
fn trim_retired_regions(core: &HirariUnifiedEngine) {
    let host = address(core);
    let _guard = match track_session::GraphLock::acquire(host) {
        Some(guard) => guard,
        None => return,
    };
    let table = unsafe { hirari_engine_block_track_table(block(core) as *const c_void) };
    if table.is_null() {
        return;
    }
    let count = unsafe { engine_track_table::hirari_engine_track_count(table) };
    let pointers = unsafe { engine_track_table::hirari_engine_track_ptrs(table) };
    if pointers.is_null() {
        return;
    }
    // SAFETY: the table's buffer holds `count` entries while its lock is held.
    for &track in unsafe { std::slice::from_raw_parts(pointers, count) } {
        if !track.is_null() {
            unsafe { region_store::hirari_region_store_trim_retired(track) };
        }
    }
}

/// Transport start/stop. Starting is gated on the driver actually running:
/// playing into a dead device would advance the timeline with nobody hearing
/// it. Stopping joins the callback, then trims the retired generations.
pub fn try_set_playing(engine: &AudioEngine, core: &HirariUnifiedEngine, playing: bool) -> bool {
    if playing
        && !audio_driver_host::hirari_audio_driver_host_is_running(
            engine.audio_driver_host() as *mut c_void
        )
    {
        return false;
    }
    unsafe {
        engine_block_host::hirari_engine_block_set_playing(block(core), u8::from(playing));
    }
    if !playing {
        join_audio_callbacks(core);
        trim_retired_regions(core);
    }
    true
}

pub fn set_playing(engine: &AudioEngine, core: &HirariUnifiedEngine, playing: bool) {
    let _ = try_set_playing(engine, core, playing);
}

/// Swaps the preview-sampler buffer. Retired generations are reclaimed only
/// while the callback is quiescent, so a running preview never has its buffer
/// freed underneath it.
pub fn set_preview_sample(core: &HirariUnifiedEngine, samples: &[f32], source_rate: f64) {
    if samples.is_empty() || !source_rate.is_finite() || source_rate <= 0.0 {
        clear_preview_sample(core);
        return;
    }
    if !is_playing(core) {
        unsafe { crate::preview_sampler::hirari_preview_sampler_clear(sampler(core)) };
        join_audio_callbacks(core);
        unsafe { crate::preview_sampler::hirari_preview_sampler_reclaim(sampler(core)) };
    }
    unsafe {
        crate::preview_sampler::hirari_preview_sampler_set(
            sampler(core),
            samples.as_ptr(),
            samples.len(),
            source_rate,
        );
    }
}

pub fn clear_preview_sample(core: &HirariUnifiedEngine) {
    unsafe { crate::preview_sampler::hirari_preview_sampler_clear(sampler(core)) };
    if !is_playing(core) {
        join_audio_callbacks(core);
        unsafe { crate::preview_sampler::hirari_preview_sampler_reclaim(sampler(core)) };
    }
}

/// A track must exist live in the graph before the Rust VCA manager records
/// the assignment; id 0 on either side is never a valid pairing.
pub fn assign_track_to_vca(core: &HirariUnifiedEngine, track_id: u32, group_id: u32) -> bool {
    if track_id == 0 || group_id == 0 {
        return false;
    }
    TrackSession::open(address(core), track_id).is_some()
        && unsafe { hirari_vca_assign_track(track_id, group_id) }
}
