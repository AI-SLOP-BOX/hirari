//! The device-config transaction is Rust-owned.
//!
//! What used to be three C++ facade methods (`apply_config`, `apply_config_locked`,
//! `try_apply_config`), a `select_audio_device` forwarder, an `EngineConfig` struct
//! and the engine's `apply_config` relay is one policy here: the format bounds from
//! `audio_config_rules`, the play-state gate read straight from the engine-block
//! store, the driver reconfigure or the detached-graph prepare, and the generation
//! check that tells a caller whether the transaction actually landed. The only C++
//! piece left is the session mutex this module locks around a transaction, because
//! the destructor, shutdown and reconnect paths on that side take the same lock.

use std::os::raw::c_void;

use crate::audio_config_rules::apply_config_valid;
use crate::audio_device_mode::hirari_native_test_isolation;
use crate::audio_driver_host::{
    hirari_audio_driver_host_buffer_size, hirari_audio_driver_host_is_running,
    hirari_audio_driver_host_reconfigure, hirari_audio_driver_host_sample_rate,
    hirari_audio_driver_host_select_device,
};
use crate::engine_block_host::{
    hirari_engine_block_device_block_frames, hirari_engine_block_generation,
    hirari_engine_block_is_playing,
};
use crate::ffi;
use crate::tempo::hirari_tempo_map_set_current_bpm;

/// Holds the C++ session lock for one configuration transaction. Two UI or CLI
/// callers must never interleave a device reconfigure with a second prepare, or
/// the published generation describes neither request.
struct ConfigLock<'a> {
    engine: &'a ffi::AudioEngine,
}

impl ConfigLock<'_> {
    fn new(engine: &ffi::AudioEngine) -> ConfigLock<'_> {
        engine.lock_config();
        ConfigLock { engine }
    }
}

impl Drop for ConfigLock<'_> {
    fn drop(&mut self) {
        self.engine.unlock_config();
    }
}

fn driver_host(engine: &ffi::AudioEngine) -> *mut c_void {
    engine.audio_driver_host() as *mut c_void
}

/// The engine-block handle is zero exactly when this session owns no graph, which
/// is the guard the deleted C++ `!m_engine` checks made.
fn block_state(engine: &ffi::AudioEngine) -> *const c_void {
    engine.engine_block_state() as *const c_void
}

/// The device block length the graph is prepared for, read from the store the
/// engine itself prepares: the deleted C++ accessor only relayed this value.
pub fn block_frames(engine: &ffi::AudioEngine) -> u32 {
    let block = block_state(engine);
    if block.is_null() {
        return 0;
    }
    unsafe { hirari_engine_block_device_block_frames(block) }
}

fn generation(engine: &ffi::AudioEngine) -> u64 {
    let block = block_state(engine);
    if block.is_null() {
        return 0;
    }
    unsafe { hirari_engine_block_generation(block) }
}

fn tempo_state(engine: &ffi::AudioEngine) -> *mut c_void {
    engine.tempo_state() as *mut c_void
}

/// Mutable pinned view of the session engine. The wrapper owns exactly one engine
/// object for its whole lifetime and never moves it, so pinning is stable.
fn pinned_core(
    engine: &ffi::AudioEngine,
) -> core::pin::Pin<&mut ffi::HirariUnifiedEngine> {
    unsafe {
        core::pin::Pin::new_unchecked(&mut *(crate::audio_engine_delegate_ext::host(
            engine,
        ) as *mut ffi::HirariUnifiedEngine))
    }
}

/// Rebuilds the graph at the format the device negotiated. Device selection has to
/// run the same quiesce/prepare sequence as any other reconfiguration, so the
/// sequence lives here once instead of in a per-driver-branch copy. The tempo map
/// keeps its current tempo; only the format changes.
fn apply_negotiated_driver_config(engine: &ffi::AudioEngine) {
    let host = driver_host(engine);
    if host.is_null() || block_state(engine).is_null() {
        return;
    }
    engine.get_core().join_audio_callbacks();
    // The engine prepared its buffers from an integral rate, so the truncated
    // driver rate is what the graph sees.
    let sample_rate = f64::from(hirari_audio_driver_host_sample_rate(host) as u32);
    let block_size = hirari_audio_driver_host_buffer_size(host);
    pinned_core(engine).prepare_to_play(sample_rate, block_size);
}

/// The transaction itself. Returns whether the requested format was applied; a
/// running device host is rebuilt together with the graph so the callback can never
/// keep the previous format, and an engine that owns no running device (CLI renders,
/// automation, isolated tests, a platform with no backend) has nothing to
/// reconfigure, so its detached graph is prepared directly.
fn apply_config_locked(engine: &ffi::AudioEngine, tempo: f32, sr: u32, bs: u32) -> bool {
    let block = block_state(engine);
    if block.is_null() || !apply_config_valid(tempo, f64::from(sr), bs) {
        return false;
    }
    // A configuration transition is a stop-the-world control operation. Do not
    // silently stop a live transport here: callers must perform the explicit
    // transport transition first, and this check runs while the session lock is
    // held so a concurrent start cannot slip between the UI preflight and the
    // driver reconfiguration.
    if unsafe { hirari_engine_block_is_playing(block) } != 0 {
        return false;
    }
    let host = driver_host(engine);
    if !hirari_native_test_isolation() && hirari_audio_driver_host_is_running(host) {
        return hirari_audio_driver_host_reconfigure(host, f64::from(sr), bs);
    }
    engine.get_core().join_audio_callbacks();
    unsafe { hirari_tempo_map_set_current_bpm(tempo_state(engine), tempo) };
    pinned_core(engine).prepare_to_play(f64::from(sr), bs);
    true
}

/// The void entry point every caller that does not need the outcome uses. Invalid
/// requests quiesce nothing and advance no generation.
pub fn apply_config(engine: &ffi::AudioEngine, tempo: f32, sr: u32, bs: u32) {
    let _lock = ConfigLock::new(engine);
    let _ = apply_config_locked(engine, tempo, sr, bs);
}

/// The reporting entry point: the transaction must both land and be observable as a
/// new generation at the requested format. There is no separate device-reconciliation
/// step, because `apply_config_locked` already rebuilds a running host with the graph.
pub fn try_apply_config(engine: &ffi::AudioEngine, tempo: f32, sr: u32, bs: u32) -> bool {
    let core = engine.get_core();
    if block_state(engine).is_null() || !apply_config_valid(tempo, f64::from(sr), bs) {
        return false;
    }
    let _lock = ConfigLock::new(engine);
    let before = generation(engine);
    if !apply_config_locked(engine, tempo, sr, bs) {
        return false;
    }
    generation(engine) != before
        && (core.get_sample_rate() - f64::from(sr)).abs() < 0.5
        && block_frames(engine) == bs
}

/// Selects a device and applies the format it negotiated as one answer, then checks
/// that the graph really ended up at that format.
pub fn select_audio_device(
    engine: &ffi::AudioEngine,
    device_id: u32,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    let host = driver_host(engine);
    if host.is_null() || block_state(engine).is_null() {
        return false;
    }
    if !hirari_audio_driver_host_select_device(host, device_id, sample_rate, buffer_size)
        || !hirari_audio_driver_host_is_running(host)
    {
        return false;
    }
    apply_negotiated_driver_config(engine);
    let core = engine.get_core();
    (core.get_sample_rate() - sample_rate).abs() < 0.5
        && block_frames(engine) == hirari_audio_driver_host_buffer_size(host)
}

impl ffi::AudioEngine {
    pub fn apply_config(&self, tempo: f32, sr: u32, bs: u32) {
        apply_config(self, tempo, sr, bs);
    }

    pub fn try_apply_config(&self, tempo: f32, sr: u32, bs: u32) -> bool {
        try_apply_config(self, tempo, sr, bs)
    }

    pub fn select_audio_device(&self, device_id: u32, sample_rate: f64, buffer_size: u32) -> bool {
        select_audio_device(self, device_id, sample_rate, buffer_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detached_engine_applies_a_valid_format_and_rejects_an_invalid_one() {
        let engine = ffi::new_audio_engine_offline();
        let before = generation(&engine);
        assert!(before > 0, "an offline session still owns its graph");
        // A rejected format answers false without quiescing anything or moving
        // the generation the caller compares against.
        assert!(!try_apply_config(&engine, 120.0, 0, 256));
        assert_eq!(generation(&engine), before);
        // A graph with no running device has nothing to reconfigure, so the
        // detached branch prepares it and the transaction reports the landing.
        assert!(try_apply_config(&engine, 120.0, 48_000, 128));
        assert_ne!(generation(&engine), before);
        assert_eq!(block_frames(&engine), 128);
        assert!((engine.get_core().get_sample_rate() - 48_000.0).abs() < 0.5);
    }
}
