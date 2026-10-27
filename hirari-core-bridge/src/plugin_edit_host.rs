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
/// Writes the parameter's current value and reports whether the track exists.
pub type QueryParameterFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, *mut f32) -> bool;
pub type ApplyParameterFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32, f32) -> bool;
pub type RecordParameterUndoFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, f32, f32);
/// Reads a plugin's type and copies its state. Returns `usize::MAX` when the
/// track or plugin does not exist, otherwise the state length.
pub type ReadPluginFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut u32, *mut u8, usize) -> usize;
/// Stores a plugin state and, when `record` is set, records the preset undo
/// entry from `before` and `state`.
pub type ApplyPluginStateFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *const u8,
    usize,
    *const u8,
    usize,
    bool,
) -> bool;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariPluginEditHostOps {
    pub query_bypass: Option<QueryBypassFn>,
    pub apply_bypass: Option<ApplyBypassFn>,
    pub move_plugin: Option<MovePluginFn>,
    pub record_bypass_undo: Option<RecordBypassUndoFn>,
    pub record_move_undo: Option<RecordMoveUndoFn>,
    pub query_parameter: Option<QueryParameterFn>,
    pub apply_parameter: Option<ApplyParameterFn>,
    pub record_parameter_undo: Option<RecordParameterUndoFn>,
    pub read_plugin: Option<ReadPluginFn>,
    pub apply_plugin_state: Option<ApplyPluginStateFn>,
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

/// Sets one plugin parameter. Non-finite values are rejected before the
/// engine is touched; a rejected index/parameter records nothing. The undo
/// pair keeps the exact requested value rather than a clamped read-back,
/// matching the previous engine behavior.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_set_parameter(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    parameter_id: u32,
    value: f32,
) -> bool {
    if !value.is_finite() {
        return false;
    }
    let Some(ops) = host_ops() else { return false };
    let (Some(query), Some(apply), Some(record)) = (
        ops.query_parameter,
        ops.apply_parameter,
        ops.record_parameter_undo,
    ) else {
        return false;
    };
    let mut before = 0.0_f32;
    if !unsafe { query(host, track_id, plugin_index, parameter_id, &mut before) } {
        return false;
    }
    if !unsafe { apply(host, track_id, plugin_index, parameter_id, value) } {
        return false;
    }
    unsafe {
        record(
            host,
            track_id,
            plugin_index,
            parameter_id,
            before,
            value,
        )
    };
    true
}

/// Applies a plugin parameter without touching the undo history. Used by
/// hydration and automation replay, where the caller owns the edit log.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_set_parameter_without_undo(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    parameter_id: u32,
    value: f32,
) -> bool {
    if !value.is_finite() {
        return false;
    }
    match host_ops().and_then(|ops| ops.apply_parameter) {
        Some(apply) => unsafe { apply(host, track_id, plugin_index, parameter_id, value) },
        None => false,
    }
}

/// Upper bound for a plugin state read, matching the preset file limit.
const MAX_PLUGIN_STATE_BYTES: usize = 16 * 1024 * 1024;

/// Owns the loaded preset until the request finishes.
struct PresetFile(*mut crate::plugin_preset_file::PluginPresetFile);

impl Drop for PresetFile {
    fn drop(&mut self) {
        unsafe { crate::plugin_preset_file::hirari_plugin_preset_file_free(self.0) };
    }
}

fn read_plugin(
    read: ReadPluginFn,
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
) -> Option<(u32, Vec<u8>)> {
    let required = unsafe {
        read(
            host,
            track_id,
            plugin_index,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if required == usize::MAX || required > MAX_PLUGIN_STATE_BYTES {
        return None;
    }
    let mut plugin_type = 0_u32;
    let mut state = vec![0u8; required];
    let actual = unsafe {
        read(
            host,
            track_id,
            plugin_index,
            &mut plugin_type,
            state.as_mut_ptr(),
            required,
        )
    };
    (actual == required).then_some((plugin_type, state))
}

/// Loads a preset file into a plugin and records one state-swap undo entry.
///
/// The engine rejected a preset whose type differs from the plugin, a plugin
/// that does not exist and a failed state restore, and snapshotted the current
/// state for undo; all of that lives here now.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `path`
/// must be readable for `path_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_load_preset(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    let Some(ops) = host_ops() else { return false };
    let (Some(read), Some(apply)) = (ops.read_plugin, ops.apply_plugin_state) else {
        return false;
    };
    let preset = PresetFile(unsafe {
        crate::plugin_preset_file::hirari_plugin_preset_file_load(path, path_length)
    });
    if preset.0.is_null() {
        return false;
    }
    let preset_type =
        unsafe { crate::plugin_preset_file::hirari_plugin_preset_file_type(preset.0) };
    let data = unsafe { crate::plugin_preset_file::hirari_plugin_preset_file_data(preset.0) };
    let size = unsafe { crate::plugin_preset_file::hirari_plugin_preset_file_size(preset.0) };
    if data.is_null() || size == 0 {
        return false;
    }
    let after = unsafe { std::slice::from_raw_parts(data, size) };
    let Some((plugin_type, before)) = read_plugin(read, host, track_id, plugin_index) else {
        return false;
    };
    if plugin_type != preset_type {
        return false;
    }
    unsafe {
        apply(
            host,
            track_id,
            plugin_index,
            after.as_ptr(),
            after.len(),
            before.as_ptr(),
            before.len(),
            true,
        )
    }
}

/// Writes a plugin's current state as a preset file.
///
/// The engine rejected an empty path, a missing plugin and an empty state
/// before handing the bytes to the preset writer.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `path`
/// must be readable for `path_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_save_preset(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    if path.is_null() || path_length == 0 {
        return false;
    }
    let Some(read) = host_ops().and_then(|ops| ops.read_plugin) else {
        return false;
    };
    let Some((plugin_type, state)) = read_plugin(read, host, track_id, plugin_index) else {
        return false;
    };
    if state.is_empty() {
        return false;
    }
    unsafe {
        crate::plugin_preset_file::hirari_plugin_preset_file_save(
            path,
            path_length,
            plugin_type,
            state.as_ptr(),
            state.len(),
        )
    }
}

/// Reads a plugin's state through the same host surface the preset policy
/// uses. Returns the state length; `out` is only written when it can hold the
/// whole state.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `out`
/// must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_get_state(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let Some(read) = host_ops().and_then(|ops| ops.read_plugin) else {
        return 0;
    };
    let Some((_, state)) = read_plugin(read, host, track_id, plugin_index) else {
        return 0;
    };
    if !out.is_null() && capacity >= state.len() && !state.is_empty() {
        unsafe { std::ptr::copy_nonoverlapping(state.as_ptr(), out, state.len()) };
    }
    state.len()
}

/// Stores a plugin state without touching the undo history.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `state`
/// must be readable for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_set_state(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    state: *const u8,
    size: usize,
) -> bool {
    if size != 0 && state.is_null() {
        return false;
    }
    let state = if size == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(state, size) }
    };
    match host_ops().and_then(|ops| ops.apply_plugin_state) {
        Some(apply) => unsafe {
            apply(
                host,
                track_id,
                plugin_index,
                state.as_ptr(),
                state.len(),
                std::ptr::null(),
                0,
                false,
            )
        },
        None => false,
    }
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
