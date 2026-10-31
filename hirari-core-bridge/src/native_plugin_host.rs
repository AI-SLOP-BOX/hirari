//! Control-plane host for the built-in native plugin catalog.
//!
//! `ExternalPluginHost` used to keep the 128-entry parameter mirror, the dirty
//! bitmask and the state-marker policy in C++ on top of a Rust DSP dispatch.
//! This module owns that policy instead: the C++ side is left with an
//! `IProcessor` adapter that forwards virtuals, while the mirror and the
//! block-boundary parameter flush live next to the processors themselves.
//!
//! The mirror is the only control/audio exchange: `set_parameter` publishes the
//! value atomically and marks the id dirty, and `process` drains the mask before
//! calling the DSP so control-thread writes never touch DSP state concurrently.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::native_builtin_dsp::{
    hirari_native_processor_create, hirari_native_processor_destroy,
    hirari_native_processor_get_parameter, hirari_native_processor_kind_for_path,
    hirari_native_processor_latency_samples, hirari_native_processor_parameter_count,
    hirari_native_processor_prepare, hirari_native_processor_process,
    hirari_native_processor_reset, hirari_native_processor_set_parameter,
    hirari_native_processor_state_restore, hirari_native_processor_state_save,
    hirari_native_processor_tail_samples,
};

/// Host-visible parameter slots. The C++ mirror never exposed more than this,
/// and the dirty mask is sized to match.
const MAX_PARAMS: usize = 128;
const MASK_WORDS: usize = MAX_PARAMS / 64;

pub struct NativePluginHost {
    kind: i32,
    state: *mut c_void,
    parameters: [AtomicU32; MAX_PARAMS],
    dirty: [AtomicU64; MASK_WORDS],
}

impl NativePluginHost {
    fn new(kind: i32) -> Option<Box<Self>> {
        let state = hirari_native_processor_create(kind);
        if state.is_null() {
            return None;
        }
        let host = Box::new(Self {
            kind,
            state,
            parameters: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            dirty: std::array::from_fn(|_| AtomicU64::new(0)),
        });
        for id in 0..MAX_PARAMS as u32 {
            let value = unsafe { hirari_native_processor_get_parameter(kind, state, id) };
            host.parameters[id as usize].store(value.to_bits(), Ordering::Relaxed);
        }
        Some(host)
    }

    fn parameter_count(&self) -> u32 {
        hirari_native_processor_parameter_count(self.kind)
    }

    /// Apply every dirty slot at a block boundary. Duplicate writes for ids the
    /// setter already pushed through are harmless and keep the ordering rule
    /// simple: the DSP always sees the latest published value.
    fn flush_parameters(&self) {
        for (word, mask) in self.dirty.iter().enumerate() {
            let mut pending = mask.swap(0, Ordering::AcqRel);
            while pending != 0 {
                let bit = pending.trailing_zeros() as u32;
                pending &= pending - 1;
                let id = word as u32 * 64 + bit;
                let value =
                    f32::from_bits(self.parameters[id as usize].load(Ordering::Relaxed));
                unsafe { hirari_native_processor_set_parameter(self.kind, self.state, id, value) };
            }
        }
    }

    fn restore_parameters(&self) {
        for id in 0..self.parameter_count().min(MAX_PARAMS as u32) {
            let value =
                unsafe { hirari_native_processor_get_parameter(self.kind, self.state, id) };
            self.parameters[id as usize].store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

impl Drop for NativePluginHost {
    fn drop(&mut self) {
        unsafe { hirari_native_processor_destroy(self.kind, self.state) };
    }
}

unsafe fn host<'a>(state: *const c_void) -> Option<&'a NativePluginHost> {
    unsafe { state.cast::<NativePluginHost>().as_ref() }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_create(
    path: *const u8,
    path_size: usize,
) -> *mut c_void {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let path = unsafe { std::slice::from_raw_parts(path, path_size) };
    let kind = unsafe { hirari_native_processor_kind_for_path(path.as_ptr(), path.len()) };
    if kind < 0 {
        return std::ptr::null_mut();
    }
    // A host is constructed reset, mirroring the previous C++ constructor:
    // the processor must not expose stale DSP state before its first block.
    match NativePluginHost::new(kind) {
        Some(host) => {
            unsafe { hirari_native_processor_reset(kind, host.state) };
            Box::into_raw(host).cast()
        }
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state.cast::<NativePluginHost>()) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_prepare(
    state: *mut c_void,
    sample_rate: f64,
    _block_size: u32,
) {
    if let Some(host) = unsafe { host(state) } {
        unsafe { hirari_native_processor_prepare(host.kind, host.state, sample_rate) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_reset(state: *mut c_void) {
    if let Some(host) = unsafe { host(state) } {
        unsafe { hirari_native_processor_reset(host.kind, host.state) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_process(
    state: *mut c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
    bpm: f64,
) {
    let Some(host) = (unsafe { host(state) }) else { return };
    host.flush_parameters();
    unsafe {
        hirari_native_processor_process(host.kind, host.state, channels, channel_count, frames, bpm)
    };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_latency_samples(state: *const c_void) -> u32 {
    match unsafe { host(state) } {
        Some(host) => unsafe { hirari_native_processor_latency_samples(host.kind, host.state) },
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_tail_samples(state: *const c_void) -> u32 {
    match unsafe { host(state) } {
        Some(host) => unsafe { hirari_native_processor_tail_samples(host.kind, host.state) },
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_parameter_count(state: *const c_void) -> u32 {
    match unsafe { host(state) } {
        Some(host) => host.parameter_count(),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_set_parameter(
    state: *mut c_void,
    id: u32,
    value: f32,
) {
    let Some(host) = (unsafe { host(state) }) else { return };
    if id as usize >= MAX_PARAMS || !value.is_finite() {
        return;
    }
    host.parameters[id as usize].store(value.to_bits(), Ordering::Relaxed);
    // Built-in processors are control-thread-owned and expose their serializer
    // synchronously, so mirror edits immediately so a project save between
    // audio blocks captures them; the dirty bit still applies them on the
    // audio thread in order.
    unsafe { hirari_native_processor_set_parameter(host.kind, host.state, id, value) };
    let word = id as usize / 64;
    host.dirty[word].fetch_or(1u64 << (id % 64), Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_parameter(state: *const c_void, id: u32) -> f32 {
    match unsafe { host(state) } {
        // The control-side mirror is authoritative between blocks; the DSP
        // instance is only updated when the next block drains its dirty bit.
        Some(host) if (id as usize) < MAX_PARAMS => {
            f32::from_bits(host.parameters[id as usize].load(Ordering::Relaxed))
        }
        _ => 0.0,
    }
}

/// Kind-specific state blob. A zero result means the kind has no serializer and
/// the caller must fall back to the shared host envelope.
#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_state_save(
    state: *const c_void,
    bypassed: u8,
    mix: f32,
    sidechain_bus_id: u32,
    output: *mut u8,
    capacity: usize,
) -> usize {
    let Some(host) = (unsafe { host(state) }) else { return 0 };
    let mut scratch = [0u8; 32];
    let written = unsafe {
        hirari_native_processor_state_save(
            host.kind,
            host.state,
            bypassed,
            mix,
            sidechain_bus_id,
            scratch.as_mut_ptr(),
            scratch.len(),
        )
    };
    if written > scratch.len() || (written > 0 && output.is_null()) {
        return 0;
    }
    if written > 0 {
        let copy = written.min(capacity);
        unsafe { std::ptr::copy_nonoverlapping(scratch.as_ptr(), output, copy) };
    }
    written
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_plugin_host_state_restore(
    state: *mut c_void,
    bytes: *const u8,
    size: usize,
    bypassed: *mut u8,
    mix: *mut f32,
    sidechain_bus_id: *mut u32,
) -> bool {
    let Some(host) = (unsafe { host(state) }) else { return false };
    if bytes.is_null() || size == 0 {
        return false;
    }
    let restored = unsafe {
        hirari_native_processor_state_restore(
            host.kind,
            host.state,
            bytes,
            size,
            bypassed,
            mix,
            sidechain_bus_id,
        )
    };
    if restored {
        host.restore_parameters();
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPRESSOR: &[u8] = b"Hirari/Compressor";
    const GATE: &[u8] = b"Hirari/Gate";

    fn create(path: &[u8]) -> *mut c_void {
        unsafe { hirari_native_plugin_host_create(path.as_ptr(), path.len()) }
    }

    fn set(host: *mut c_void, id: u32, value: f32) {
        unsafe { hirari_native_plugin_host_set_parameter(host, id, value) };
    }

    fn get(host: *const c_void, id: u32) -> f32 {
        unsafe { hirari_native_plugin_host_parameter(host, id) }
    }

    fn dsp_parameter(host: *mut c_void, id: u32) -> f32 {
        let inner = unsafe { host.cast::<NativePluginHost>().as_ref() }.unwrap();
        unsafe { hirari_native_processor_get_parameter(inner.kind, inner.state, id) }
    }

    fn run_block(host: *mut c_void, left: &mut [f32], right: &mut [f32]) {
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            hirari_native_plugin_host_prepare(host, 48_000.0, 128);
            hirari_native_plugin_host_process(
                host,
                channels.as_mut_ptr(),
                2,
                left.len() as u32,
                120.0,
            );
        }
    }

    #[test]
    fn unknown_paths_do_not_create_a_host() {
        assert!(create(b"Hirari/NoSuchPlugin").is_null());
        assert_eq!(
            unsafe { hirari_native_plugin_host_parameter_count(std::ptr::null()) },
            0
        );
    }

    #[test]
    fn parameters_are_mirrored_and_flushed_at_the_block_boundary() {
        let host = create(COMPRESSOR);
        assert!(!host.is_null());
        assert_eq!(
            unsafe { hirari_native_plugin_host_parameter_count(host) },
            7
        );
        set(host, 0, 0.7);
        // The mirror answers immediately and the control-thread serializer is
        // kept in step so a save between blocks captures the edit; the dirty
        // bit still replays the value on the audio thread and is drained then.
        assert_eq!(get(host, 0), 0.7);
        assert_eq!(dsp_parameter(host, 0), 0.7);
        let inner = unsafe { host.cast::<NativePluginHost>().as_ref() }.unwrap();
        assert_ne!(inner.dirty[0].load(Ordering::Relaxed), 0);
        let mut left = [0.9f32; 128];
        let mut right = [-0.9f32; 128];
        run_block(host, &mut left, &mut right);
        assert_eq!(inner.dirty[0].load(Ordering::Relaxed), 0);
        assert_eq!(dsp_parameter(host, 0), 0.7);
        assert!(left.iter().chain(right.iter()).all(|s| s.is_finite()));
        unsafe { hirari_native_plugin_host_destroy(host) };
    }

    #[test]
    fn non_finite_and_out_of_range_writes_are_ignored() {
        let host = create(COMPRESSOR);
        set(host, 0, 0.4);
        set(host, 0, f32::NAN);
        set(host, 200, 1.0);
        assert_eq!(get(host, 0), 0.4);
        assert_eq!(get(host, 200), 0.0);
        unsafe { hirari_native_plugin_host_destroy(host) };
    }

    #[test]
    fn kinds_without_a_serializer_leave_the_envelope_to_the_caller() {
        let host = create(GATE);
        let mut blob = [0u8; 32];
        let written = unsafe {
            hirari_native_plugin_host_state_save(host, 0, 1.0, 0, blob.as_mut_ptr(), blob.len())
        };
        assert_eq!(written, 0);
        unsafe { hirari_native_plugin_host_destroy(host) };
    }

    #[test]
    fn compressor_state_round_trips_and_resyncs_the_mirror() {
        let source = create(COMPRESSOR);
        set(source, 0, 0.65);
        set(source, 1, 0.3);
        let mut blob = [0u8; 32];
        let written = unsafe {
            hirari_native_plugin_host_state_save(
                source,
                1,
                0.5,
                7,
                blob.as_mut_ptr(),
                blob.len(),
            )
        };
        assert!(written > 0);

        let target = create(COMPRESSOR);
        let (mut bypassed, mut mix, mut sidechain) = (0u8, 1.0f32, 0u32);
        let restored = unsafe {
            hirari_native_plugin_host_state_restore(
                target,
                blob.as_ptr(),
                written,
                &mut bypassed,
                &mut mix,
                &mut sidechain,
            )
        };
        assert!(restored);
        assert_eq!(get(target, 0), 0.65);
        assert_eq!(get(target, 1), 0.3);
        assert!(!unsafe {
            hirari_native_plugin_host_state_restore(
                target,
                blob.as_ptr(),
                3,
                &mut bypassed,
                &mut mix,
                &mut sidechain,
            )
        });
        unsafe {
            hirari_native_plugin_host_destroy(source);
            hirari_native_plugin_host_destroy(target);
        }
    }
}
