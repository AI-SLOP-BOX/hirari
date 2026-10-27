//! Plugin-chain edit policy: bypass and reordering.
//!
//! The track keeps the effect chain because the audio callback walks it. This
//! module owns the admission rules and the undo decision: it reads the current
//! bypass state through a host callback, applies the mutation, and lets the
//! engine record exactly one undo entry built from the pre/post values.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

/// Returns `-1` when the track is missing, otherwise the plugin's bypass
/// state (`0`/`1`). An out-of-range plugin index on an existing track reads as
/// `0`, matching `Track::getPluginBypass`.
pub type QueryBypassFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> i32;
pub type ApplyBypassFn = unsafe extern "C" fn(*mut c_void, u32, u32, bool) -> bool;
pub type MovePluginFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32) -> bool;
pub type RecordBypassUndoFn = unsafe extern "C" fn(*mut c_void, u32, u32, bool, bool);
pub type RecordMoveUndoFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariPluginEditHostOps {
    pub query_bypass: Option<QueryBypassFn>,
    pub apply_bypass: Option<ApplyBypassFn>,
    pub move_plugin: Option<MovePluginFn>,
    pub record_bypass_undo: Option<RecordBypassUndoFn>,
    pub record_move_undo: Option<RecordMoveUndoFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariPluginEditHostOps {}
unsafe impl Sync for HirariPluginEditHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariPluginEditHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariPluginEditHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_host_install(
    ops: *const HirariPluginEditHostOps,
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

/// Sets a plugin's bypass flag. The engine rejected an out-of-range plugin
/// index before anything was recorded; a same-value set still records one
/// entry, as it did before the policy moved here.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_set_bypass(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    bypassed: bool,
) -> bool {
    let Some(ops) = host_ops() else { return false };
    let (Some(query), Some(apply), Some(record)) = (
        ops.query_bypass,
        ops.apply_bypass,
        ops.record_bypass_undo,
    ) else {
        return false;
    };
    let current = unsafe { query(host, track_id, plugin_index) };
    if current < 0 {
        return false;
    }
    if !unsafe { apply(host, track_id, plugin_index, bypassed) } {
        return false;
    }
    unsafe { record(host, track_id, plugin_index, current != 0, bypassed) };
    true
}

/// Moves a plugin inside its track's chain. Moving onto the same index is
/// rejected, matching `Track::movePlugin`'s contract; no undo entry is
/// recorded for a rejected move.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_move(
    host: *mut c_void,
    track_id: u32,
    from_index: u32,
    to_index: u32,
) -> bool {
    if from_index == to_index {
        return false;
    }
    let Some(ops) = host_ops() else { return false };
    let (Some(move_plugin), Some(record)) = (ops.move_plugin, ops.record_move_undo) else {
        return false;
    };
    if !unsafe { move_plugin(host, track_id, from_index, to_index) } {
        return false;
    }
    unsafe { record(host, track_id, from_index, to_index) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_table_is_empty_until_installed() {
        // The table is process-global; this only asserts the default shape is
        // absent before any native engine installs callbacks.
        let ops = HirariPluginEditHostOps::default();
        assert!(ops.query_bypass.is_none());
        assert!(ops.apply_bypass.is_none());
        assert!(ops.move_plugin.is_none());
        assert!(ops.record_bypass_undo.is_none());
        assert!(ops.record_move_undo.is_none());
    }
}
