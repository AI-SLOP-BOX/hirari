//! Control-thread edit policy for track naming, input monitoring and macro
//! MIDI learn.
//!
//! The engine keeps the monitoring atomics and the MIDI learn manager because
//! the audio callback reads them. This module owns the admission rules and the
//! undo decision: it rejects the edit before anything is touched, applies the
//! resolved mutation through the track's own session, and records exactly one
//! undo entry.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::slice;
use std::sync::{Mutex, OnceLock};

use crate::plugin_edit_host::{commit_closures, UndoStateFn};
use crate::track_name_store::MAX_TRACK_NAME_BYTES;
use crate::track_session::TrackSession;

/// Matches the engine's per-input channel addressing limit.
const MAX_MONITOR_CHANNEL: u32 = 32;
/// MIDI channels 0..=15 plus 16 for "any channel".
const MAX_MIDI_CHANNEL: u8 = 16;
const MAX_MIDI_CC: u16 = 127;
const MAX_MIDI_CC14: u16 = 16383;
/// Matches `HirariUnifiedEngine::kMaxMacros`.
const MAX_MACROS: u32 = 128;

/// Applies the monitor focus and reports whether the target track exists.
pub type UpdateMonitorFocusFn =
    unsafe extern "C" fn(*mut c_void, u32, bool, u32, u32) -> bool;
/// Hands the Rust-owned macro-control or MIDI-learn store handle to this
/// module; the engine holds them but never decides what a binding means.
pub type StoreFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariControlEditHostOps {
    pub update_monitor_focus: Option<UpdateMonitorFocusFn>,
    pub macro_control_store: Option<StoreFn>,
    pub midi_learn_store: Option<StoreFn>,
    pub undo_state: Option<UndoStateFn>,
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

/// Renames a track. Rejects empty, oversized and NUL-containing names before
/// touching the track, treats an identical name as a successful no-op, and
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
    rename(host, track_id, requested)
}

/// Writes the name into the track's own store, then records the reversible edit
/// once the session has released the tracks lock. A missing history keeps the
/// edit and records nothing, as the engine did when no undo manager existed.
fn rename(host: *mut c_void, track_id: u32, requested: &[u8]) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let current = session.read_name();
    if current == requested {
        return true;
    }
    if !session.write_name(requested) {
        return false;
    }
    session.commit_edit(true);
    drop(session);
    let Some(read_state) = host_ops().and_then(|ops| ops.undo_state) else {
        return true;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return true;
    }
    let redo_name = requested.to_vec();
    commit_closures(
        state,
        b"Rename Track",
        Box::new(move || replay_name(host, track_id, &current)),
        Box::new(move || replay_name(host, track_id, &redo_name)),
    );
    true
}
/// Replays one side of a rename through a fresh session on the same track.
fn replay_name(host: *mut c_void, track_id: u32, name: &[u8]) {
    if let Some(session) = TrackSession::open(host, track_id) {
        session.commit_edit(session.write_name(name));
    }
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
    if let Some(control) = host_ops().and_then(|ops| ops.macro_control_store) {
        unsafe {
            crate::macro_mapping::hirari_macro_control_set_value(control(host), macro_index, value)
        };
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
    let Some(ops) = host_ops() else {
        return false;
    };
    let (Some(learn), Some(control)) = (ops.midi_learn_store, ops.macro_control_store) else {
        return false;
    };
    // The learn manager stores a pointer to the macro control it drives, so a
    // binding without that target is rejected exactly as the engine did.
    let macro_state = unsafe { control(host) };
    if macro_state.is_null() {
        return false;
    }
    let learn_state = unsafe { learn(host) };
    if cc14 {
        unsafe {
            crate::midi_learn::hirari_midi_learn_add_mapping_14bit(
                learn_state,
                controller,
                channel,
                macro_state,
                macro_index,
                minimum,
                maximum,
                curve,
                pickup,
            )
        };
    } else {
        unsafe {
            crate::midi_learn::hirari_midi_learn_add_mapping(
                learn_state,
                controller as u8,
                channel,
                macro_state,
                macro_index,
                minimum,
                maximum,
                curve,
                pickup,
            )
        };
    }
    true
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
