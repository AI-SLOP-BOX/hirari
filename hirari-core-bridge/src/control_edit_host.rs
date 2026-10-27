//! Control-thread edit policy for track naming, input monitoring and macro
//! MIDI learn.
//!
//! The engine keeps the track objects, the monitoring atomics and the MIDI
//! learn manager because the audio callback reads them. This module owns the
//! admission rules and the undo decision: it reads the current value through
//! host callbacks, rejects the edit before anything is touched, applies the
//! resolved mutation, and records exactly one undo entry.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::slice;
use std::sync::{Mutex, OnceLock};

/// Mirrors `HirariUnifiedEngine::kMaxTrackNameBytes`; names are persisted and
/// compared byte for byte.
const MAX_TRACK_NAME_BYTES: usize = 256;
/// Matches the engine's per-input channel addressing limit.
const MAX_MONITOR_CHANNEL: u32 = 32;
/// MIDI channels 0..=15 plus 16 for "any channel".
const MAX_MIDI_CHANNEL: u8 = 16;
const MAX_MIDI_CC: u16 = 127;
const MAX_MIDI_CC14: u16 = 16383;
/// Matches `HirariUnifiedEngine::kMaxMacros`.
const MAX_MACROS: u32 = 128;

pub type QueryTrackNameFn =
    unsafe extern "C" fn(*mut c_void, u32, *mut u8, usize) -> isize;
pub type ApplyTrackNameFn = unsafe extern "C" fn(*mut c_void, u32, *const u8, usize) -> bool;
pub type RecordNameUndoFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    *const u8,
    usize,
    *const u8,
    usize,
);
/// Applies the monitor focus and reports whether the target track exists.
pub type UpdateMonitorFocusFn =
    unsafe extern "C" fn(*mut c_void, u32, bool, u32, u32) -> bool;
pub type SetMacroValueFn = unsafe extern "C" fn(*mut c_void, u32, f32);
/// `cc14` selects the 14-bit controller table; `controller` then carries the
/// full 0..=16383 value instead of a 7-bit CC number.
pub type BindMacroFn =
    unsafe extern "C" fn(*mut c_void, u32, u8, bool, u16, f32, f32, f32, bool) -> bool;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariControlEditHostOps {
    pub query_track_name: Option<QueryTrackNameFn>,
    pub apply_track_name: Option<ApplyTrackNameFn>,
    pub record_name_undo: Option<RecordNameUndoFn>,
    pub update_monitor_focus: Option<UpdateMonitorFocusFn>,
    pub set_macro_value: Option<SetMacroValueFn>,
    pub bind_macro: Option<BindMacroFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariControlEditHostOps {}
unsafe impl Sync for HirariControlEditHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariControlEditHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariControlEditHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_host_install(
    ops: *const HirariControlEditHostOps,
) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { std::ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

fn valid_track_name(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= MAX_TRACK_NAME_BYTES && !name.contains(&0)
}

fn valid_monitor_channels(left: u32, right: u32) -> bool {
    left < MAX_MONITOR_CHANNEL && right < MAX_MONITOR_CHANNEL
}

fn valid_macro_binding(
    macro_index: u32,
    channel: u8,
    cc14: bool,
    controller: u16,
    minimum: f32,
    maximum: f32,
    curve: f32,
) -> bool {
    if macro_index >= MAX_MACROS || channel > MAX_MIDI_CHANNEL {
        return false;
    }
    if cc14 {
        if controller > MAX_MIDI_CC14 {
            return false;
        }
    } else if controller > MAX_MIDI_CC {
        return false;
    }
    minimum.is_finite()
        && maximum.is_finite()
        && minimum <= maximum
        && curve.is_finite()
        && (-1.0..=1.0).contains(&curve)
}

fn query_track_name(
    ops: &HirariControlEditHostOps,
    host: *mut c_void,
    track_id: u32,
) -> Option<Vec<u8>> {
    let query = ops.query_track_name?;
    let needed = unsafe { query(host, track_id, std::ptr::null_mut(), 0) };
    if needed < 0 {
        return None;
    }
    let mut buffer = vec![0_u8; needed as usize];
    if needed == 0 {
        return Some(buffer);
    }
    let written = unsafe { query(host, track_id, buffer.as_mut_ptr(), buffer.len()) };
    if written != needed {
        return None;
    }
    Some(buffer)
}

/// Renames a track. Rejects empty, oversized and NUL-containing names before
/// touching the engine, treats an identical name as a successful no-op, and
/// records one `Rename Track` undo entry otherwise.
///
/// # Safety
/// `name` must point to `name_len` readable bytes for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_rename_track(
    host: *mut c_void,
    track_id: u32,
    name: *const u8,
    name_len: usize,
) -> bool {
    if name.is_null() || name_len == 0 || name_len > MAX_TRACK_NAME_BYTES {
        return false;
    }
    let requested = unsafe { slice::from_raw_parts(name, name_len) };
    if !valid_track_name(requested) {
        return false;
    }
    let Some(ops) = host_ops() else { return false };
    let (Some(apply), Some(record)) = (ops.apply_track_name, ops.record_name_undo) else {
        return false;
    };
    let Some(current) = query_track_name(&ops, host, track_id) else {
        return false;
    };
    if current == requested {
        return true;
    }
    if !unsafe { apply(host, track_id, requested.as_ptr(), requested.len()) } {
        return false;
    }
    unsafe {
        record(
            host,
            track_id,
            current.as_ptr(),
            current.len(),
            requested.as_ptr(),
            requested.len(),
        )
    };
    true
}

/// Moves the single-input-monitor focus. Enabling requires the track to exist;
/// disabling is always accepted, matching the engine's previous contract.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_set_input_monitor(
    host: *mut c_void,
    track_id: u32,
    enabled: bool,
    left_channel: u32,
    right_channel: u32,
) -> bool {
    if !valid_monitor_channels(left_channel, right_channel) {
        return false;
    }
    let Some(update) = host_ops().and_then(|ops| ops.update_monitor_focus) else {
        return false;
    };
    unsafe { update(host, track_id, enabled, left_channel, right_channel) }
}

/// Sets a macro value. Non-finite values are ignored without touching the
/// engine, as before.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_set_macro_value(
    host: *mut c_void,
    macro_index: u32,
    value: f32,
) {
    if !value.is_finite() {
        return;
    }
    if let Some(set) = host_ops().and_then(|ops| ops.set_macro_value) {
        unsafe { set(host, macro_index, value) };
    }
}

fn bind_macro(
    host: *mut c_void,
    channel: u8,
    cc14: bool,
    controller: u16,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
) -> bool {
    if !valid_macro_binding(macro_index, channel, cc14, controller, minimum, maximum, curve) {
        return false;
    }
    let Some(bind) = host_ops().and_then(|ops| ops.bind_macro) else {
        return false;
    };
    unsafe {
        bind(
            host,
            macro_index,
            channel,
            cc14,
            controller,
            minimum,
            maximum,
            curve,
            pickup,
        )
    }
}

/// Binds a 7-bit MIDI CC to a macro.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_bind_midi_cc(
    host: *mut c_void,
    channel: u8,
    cc: u16,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
) -> bool {
    bind_macro(
        host, channel, false, cc, macro_index, minimum, maximum, curve, pickup,
    )
}

/// Binds a 14-bit MIDI controller to a macro. The controller number is
/// validated against the 14-bit range before it reaches the learn manager.
/// 7-bit controllers above 127 are rejected here rather than truncated.
#[no_mangle]
pub unsafe extern "C" fn hirari_control_edit_bind_midi_cc14(
    host: *mut c_void,
    channel: u8,
    controller: u16,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
) -> bool {
    bind_macro(
        host, channel, true, controller, macro_index, minimum, maximum, curve, pickup,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_names_require_content_and_a_bounded_size() {
        assert!(valid_track_name(b"Drums"));
        assert!(!valid_track_name(b""));
        assert!(!valid_track_name(b"bad\0name"));
        assert!(valid_track_name(&vec![b'a'; MAX_TRACK_NAME_BYTES]));
        assert!(!valid_track_name(&vec![b'a'; MAX_TRACK_NAME_BYTES + 1]));
    }

    #[test]
    fn monitor_channels_stay_inside_the_engine_limit() {
        assert!(valid_monitor_channels(0, 1));
        assert!(valid_monitor_channels(31, 31));
        assert!(!valid_monitor_channels(32, 0));
        assert!(!valid_monitor_channels(0, 32));
    }

    #[test]
    fn macro_bindings_reject_out_of_range_values() {
        assert!(valid_macro_binding(0, 0, false, 1, 0.0, 1.0, 0.0));
        assert!(valid_macro_binding(127, 16, false, 127, 0.0, 1.0, -1.0));
        assert!(!valid_macro_binding(128, 0, false, 1, 0.0, 1.0, 0.0));
        assert!(!valid_macro_binding(0, 17, false, 1, 0.0, 1.0, 0.0));
        assert!(!valid_macro_binding(0, 0, false, 128, 0.0, 1.0, 0.0));
        assert!(!valid_macro_binding(0, 0, true, 16384, 0.0, 1.0, 0.0));
        assert!(valid_macro_binding(0, 0, true, 16383, 0.0, 1.0, 0.0));
        assert!(!valid_macro_binding(
            0,
            0,
            false,
            1,
            f32::NAN,
            1.0,
            0.0
        ));
        assert!(!valid_macro_binding(0, 0, false, 1, 1.0, 0.0, 0.0));
        assert!(!valid_macro_binding(0, 0, false, 1, 0.0, 1.0, 1.5));
    }

    #[test]
    fn cc14_accepts_the_full_controller_range() {
        // A 7-bit binding must not silently accept a number that only fits
        // the 14-bit range, while the 14-bit path keeps it intact.
        assert!(valid_macro_binding(1, 2, true, 1000, 0.0, 1.0, 0.0));
        assert!(!valid_macro_binding(1, 2, false, 1000, 0.0, 1.0, 0.0));
    }
}
