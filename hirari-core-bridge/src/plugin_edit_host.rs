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

use crate::undo_transaction::{
    hirari_undo_manager_commit, hirari_undo_manager_discard_native_callback,
    hirari_undo_manager_register_native_callback,
};

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
/// Appends a plugin and writes its new index. A non-null `path` appends the
/// sandboxed bundle at that path instead of the built-in `plugin_type`.
/// `prepare` re-prepares the track for playback, as the engine did on the
/// initial insert only.
pub type AppendPluginFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *const u8, usize, bool, *mut u32) -> bool;
/// Removes a plugin. When `out_snapshot` is non-null the removed processor is
/// captured into an opaque handle the caller must release.
pub type RemovePluginFn = unsafe extern "C" fn(*mut c_void, u32, u32, *mut *mut c_void) -> bool;
/// Re-inserts a captured snapshot at its original index.
pub type RestorePluginFn = unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void) -> bool;
pub type ReleasePluginSnapshotFn = unsafe extern "C" fn(*mut c_void);
/// Replays one side of a plugin add/remove undo entry.
pub type ReplayPluginEditFn = unsafe extern "C" fn(*mut c_void);
/// Records an applied action; the host invokes `undo`/`redo` with `edit` and
/// releases it once the history drops the entry.
pub type RecordPluginReplayFn = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    *mut c_void,
    ReplayPluginEditFn,
    ReplayPluginEditFn,
    ReplayPluginEditFn,
);
/// Re-publishes the track snapshot after the chain changed.
pub type SyncStructuralChangesFn = unsafe extern "C" fn(*mut c_void);
/// Returns the engine's Rust undo history handle.
pub type UndoStateFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

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
    pub append_plugin: Option<AppendPluginFn>,
    pub remove_plugin: Option<RemovePluginFn>,
    pub restore_plugin: Option<RestorePluginFn>,
    pub release_plugin_snapshot: Option<ReleasePluginSnapshotFn>,
    pub record_plugin_replay: Option<RecordPluginReplayFn>,
    pub sync_structural_changes: Option<SyncStructuralChangesFn>,
    pub undo_state: Option<UndoStateFn>,
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

/// A history callback body. The history owns the box and drops it when the
/// entry leaves the undo/redo stacks.
struct ReplayClosure(Box<dyn Fn()>);

unsafe extern "C" fn invoke_replay(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    let closure = unsafe { &*(context as *const ReplayClosure) };
    (closure.0)();
}

unsafe extern "C" fn release_replay(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(context as *mut ReplayClosure) });
}

fn register_replay(state: *mut c_void, callback: Box<dyn Fn()>) -> u64 {
    let context = Box::into_raw(Box::new(ReplayClosure(callback))).cast::<c_void>();
    let id = unsafe {
        hirari_undo_manager_register_native_callback(
            state,
            context,
            Some(invoke_replay),
            Some(release_replay),
        )
    };
    if id == 0 {
        unsafe { drop(Box::from_raw(context as *mut ReplayClosure)) };
    }
    id
}

/// Records an edit the caller already applied in the engine's own history.
/// `recordAppliedAction` semantics: no replay, one diagnostics line.
fn record_applied(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    name: &str,
    undo: impl Fn() + 'static,
    redo: impl Fn() + 'static,
) -> bool {
    let Some(read_state) = ops.undo_state else {
        return false;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return false;
    }
    let undo_id = register_replay(state, Box::new(undo));
    if undo_id == 0 {
        return false;
    }
    let redo_id = register_replay(state, Box::new(redo));
    if redo_id == 0 {
        unsafe { hirari_undo_manager_discard_native_callback(state, undo_id) };
        return false;
    }
    unsafe {
        hirari_undo_manager_commit(
            state,
            name.as_ptr(),
            name.len(),
            undo_id,
            redo_id,
            false,
            false,
        )
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
    match host_ops() {
        Some(ops) => set_bypass_with(&ops, host, track_id, plugin_index, bypassed),
        None => false,
    }
}

fn set_bypass_with(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    bypassed: bool,
) -> bool {
    let (Some(query), Some(apply)) = (ops.query_bypass, ops.apply_bypass) else {
        return false;
    };
    let current = unsafe { query(host, track_id, plugin_index) };
    if current < 0 {
        return false;
    }
    if !unsafe { apply(host, track_id, plugin_index, bypassed) } {
        return false;
    }
    if let Some(record) = ops.record_bypass_undo {
        unsafe { record(host, track_id, plugin_index, current != 0, bypassed) };
        return true;
    }
    let before = current != 0;
    record_applied(
        ops,
        host,
        "Set Plugin Bypass",
        move || {
            unsafe { apply(host, track_id, plugin_index, before) };
        },
        move || {
            unsafe { apply(host, track_id, plugin_index, bypassed) };
        },
    );
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
    let Some(move_plugin) = ops.move_plugin else {
        return false;
    };
    if !unsafe { move_plugin(host, track_id, from_index, to_index) } {
        return false;
    }
    if let Some(record) = ops.record_move_undo {
        unsafe { record(host, track_id, from_index, to_index) };
        return true;
    }
    record_applied(
        &ops,
        host,
        "Move Plugin",
        move || {
            unsafe { move_plugin(host, track_id, to_index, from_index) };
        },
        move || {
            unsafe { move_plugin(host, track_id, from_index, to_index) };
        },
    );
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
    let (Some(query), Some(apply)) = (ops.query_parameter, ops.apply_parameter) else {
        return false;
    };
    let mut before = 0.0_f32;
    if !unsafe { query(host, track_id, plugin_index, parameter_id, &mut before) } {
        return false;
    }
    if !unsafe { apply(host, track_id, plugin_index, parameter_id, value) } {
        return false;
    }
    if let Some(record) = ops.record_parameter_undo {
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
        return true;
    }
    record_applied(
        &ops,
        host,
        "Set Plugin Parameter",
        move || {
            unsafe { apply(host, track_id, plugin_index, parameter_id, before) };
        },
        move || {
            unsafe { apply(host, track_id, plugin_index, parameter_id, value) };
        },
    );
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
    if ops.undo_state.is_none() {
        return unsafe {
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
        };
    }
    if !unsafe {
        apply(
            host,
            track_id,
            plugin_index,
            after.as_ptr(),
            after.len(),
            std::ptr::null(),
            0,
            false,
        )
    } {
        return false;
    }
    // The history keeps both snapshots; replay stores them without recording
    // another entry.
    let undo_copy = before.clone();
    let redo_copy = after.to_vec();
    record_applied(
        &ops,
        host,
        "Load Plugin Preset",
        move || unsafe {
            apply(
                host,
                track_id,
                plugin_index,
                undo_copy.as_ptr(),
                undo_copy.len(),
                std::ptr::null(),
                0,
                false,
            );
        },
        move || unsafe {
            apply(
                host,
                track_id,
                plugin_index,
                redo_copy.as_ptr(),
                redo_copy.len(),
                std::ptr::null(),
                0,
                false,
            );
        },
    );
    true
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

/// What an `Add`/`Remove Plugin` undo entry replays.
enum PluginEditKind {
    /// A built-in plugin was appended; replay re-appends it by type.
    Added { plugin_type: u32 },
    /// A sandboxed bundle was appended; replay re-appends it by path.
    SandboxedAdded { path: Vec<u8> },
    /// A plugin was removed; replay re-inserts its captured snapshot.
    Removed { snapshot: *mut c_void },
}

/// One recorded plugin add/remove entry. It owns the state captured after the
/// insert (or the removed processor snapshot) until the history drops it. The
/// host table is copied in so a replay never depends on the current global.
struct PluginEdit {
    ops: HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    state: Vec<u8>,
    bypassed: bool,
    kind: PluginEditKind,
}

fn capture_appended(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
) -> (Vec<u8>, bool) {
    let state = ops
        .read_plugin
        .and_then(|read| read_plugin(read, host, track_id, plugin_index))
        .map(|(_, state)| state)
        .unwrap_or_default();
    let bypassed = ops
        .query_bypass
        .map(|query| unsafe { query(host, track_id, plugin_index) } > 0)
        .unwrap_or(false);
    (state, bypassed)
}

fn reappend(edit: &PluginEdit, native_type: u32, path: &[u8]) -> Option<u32> {
    let append = edit.ops.append_plugin?;
    // An empty path selects the built-in plugin; the host treats a non-null
    // pointer as a sandboxed bundle request.
    let path_ptr = if path.is_empty() {
        std::ptr::null()
    } else {
        path.as_ptr()
    };
    let mut index = 0_u32;
    let inserted = unsafe {
        append(
            edit.host,
            edit.track_id,
            native_type,
            path_ptr,
            path.len(),
            false,
            &mut index,
        )
    };
    if !inserted {
        return None;
    }
    if !edit.state.is_empty() {
        if let Some(apply) = edit.ops.apply_plugin_state {
            unsafe {
                apply(
                    edit.host,
                    edit.track_id,
                    index,
                    edit.state.as_ptr(),
                    edit.state.len(),
                    std::ptr::null(),
                    0,
                    false,
                )
            };
        }
    }
    if let Some(apply) = edit.ops.apply_bypass {
        unsafe { apply(edit.host, edit.track_id, index, edit.bypassed) };
    }
    Some(index)
}

unsafe extern "C" fn replay_plugin_undo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const PluginEdit) };
    match &edit.kind {
        PluginEditKind::Added { .. } | PluginEditKind::SandboxedAdded { .. } => {
            if let Some(remove) = edit.ops.remove_plugin {
                unsafe { remove(edit.host, edit.track_id, edit.plugin_index, std::ptr::null_mut()) };
            }
        }
        PluginEditKind::Removed { snapshot } => {
            if let Some(restore) = edit.ops.restore_plugin {
                unsafe { restore(edit.host, edit.track_id, edit.plugin_index, *snapshot) };
            }
        }
    }
}

unsafe extern "C" fn replay_plugin_redo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const PluginEdit) };
    match &edit.kind {
        PluginEditKind::Added { plugin_type } => {
            // The re-appended plugin takes a fresh index; it is appended by
            // the Track contract, as the engine's redo always did.
            let _ = reappend(edit, *plugin_type, &[]);
        }
        PluginEditKind::SandboxedAdded { path } => {
            let _ = reappend(edit, 0, path);
        }
        PluginEditKind::Removed { .. } => {
            if let Some(remove) = edit.ops.remove_plugin {
                unsafe { remove(edit.host, edit.track_id, edit.plugin_index, std::ptr::null_mut()) };
            }
        }
    }
}

unsafe extern "C" fn release_plugin_edit(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { Box::from_raw(edit as *mut PluginEdit) };
    if let PluginEditKind::Removed { snapshot } = edit.kind {
        if let Some(release) = edit.ops.release_plugin_snapshot {
            unsafe { release(snapshot) };
        }
    }
}

/// Hands one plugin edit to the history. A missing recorder drops the entry
/// (releasing its snapshot) instead of leaking it.
fn record_plugin_edit(ops: &HirariPluginEditHostOps, host: *mut c_void, label: &str, edit: PluginEdit) {
    let edit = Box::into_raw(Box::new(edit));
    match ops.record_plugin_replay {
        Some(record) => {
            unsafe {
                record(
                    host,
                    label.as_ptr(),
                    label.len(),
                    edit.cast(),
                    replay_plugin_undo,
                    replay_plugin_redo,
                    release_plugin_edit,
                )
            };
        }
        None => unsafe { release_plugin_edit(edit.cast()) },
    }
}

fn sync_structural_changes(ops: &HirariPluginEditHostOps, host: *mut c_void) {
    if let Some(sync) = ops.sync_structural_changes {
        unsafe { sync(host) };
    }
}

fn append_builtin(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    plugin_type: u32,
) -> bool {
    let Some(append) = ops.append_plugin else {
        return false;
    };
    let mut plugin_index = 0_u32;
    let inserted = unsafe {
        append(
            host,
            track_id,
            plugin_type,
            std::ptr::null(),
            0,
            true,
            &mut plugin_index,
        )
    };
    if !inserted {
        return false;
    }
    let (state, bypassed) = capture_appended(ops, host, track_id, plugin_index);
    record_plugin_edit(
        ops,
        host,
        "Add Plugin",
        PluginEdit {
            ops: *ops,
            host,
            track_id,
            plugin_index,
            state,
            bypassed,
            kind: PluginEditKind::Added { plugin_type },
        },
    );
    sync_structural_changes(ops, host);
    true
}

fn append_sandboxed(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
) -> bool {
    if !unsafe {
        crate::plugin_sandbox_host::hirari_plugin_sandbox_path_supported(path.as_ptr(), path.len())
    } {
        return false;
    }
    let Some(append) = ops.append_plugin else {
        return false;
    };
    let mut plugin_index = 0_u32;
    let inserted = unsafe {
        append(
            host,
            track_id,
            0,
            path.as_ptr(),
            path.len(),
            false,
            &mut plugin_index,
        )
    };
    if !inserted {
        return false;
    }
    let (state, bypassed) = capture_appended(ops, host, track_id, plugin_index);
    record_plugin_edit(
        ops,
        host,
        "Add Sandboxed Plugin",
        PluginEdit {
            ops: *ops,
            host,
            track_id,
            plugin_index,
            state,
            bypassed,
            kind: PluginEditKind::SandboxedAdded {
                path: path.to_vec(),
            },
        },
    );
    sync_structural_changes(ops, host);
    true
}

fn remove_chain_plugin(
    ops: &HirariPluginEditHostOps,
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
) -> bool {
    let Some(remove) = ops.remove_plugin else {
        return false;
    };
    let mut snapshot = std::ptr::null_mut();
    if !unsafe { remove(host, track_id, plugin_index, &mut snapshot) } || snapshot.is_null() {
        return false;
    }
    record_plugin_edit(
        ops,
        host,
        "Remove Plugin",
        PluginEdit {
            ops: *ops,
            host,
            track_id,
            plugin_index,
            state: Vec::new(),
            bypassed: false,
            kind: PluginEditKind::Removed { snapshot },
        },
    );
    sync_structural_changes(ops, host);
    true
}

/// Appends a built-in plugin and records one `Add Plugin` undo entry.
///
/// The engine rejected a missing track and a failed insert without recording
/// anything; the entry captures the appended state and bypass so undo removes
/// it and redo re-appends it with the same settings.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_add(
    host: *mut c_void,
    track_id: u32,
    plugin_type: u32,
) -> bool {
    match host_ops() {
        Some(ops) => append_builtin(&ops, host, track_id, plugin_type),
        None => false,
    }
}

/// Appends a sandboxed bundle and records one `Add Sandboxed Plugin` entry.
///
/// The accepted bundle formats are checked before the engine is touched,
/// exactly as the engine did with the shared path policy.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `path`
/// must be readable for `path_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_add_sandboxed(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    if path.is_null() || path_length == 0 {
        return false;
    }
    let path = unsafe { std::slice::from_raw_parts(path, path_length) };
    match host_ops() {
        Some(ops) => append_sandboxed(&ops, host, track_id, path),
        None => false,
    }
}

/// Removes a plugin and records one `Remove Plugin` undo entry whose undo
/// re-inserts the captured processor at the same index.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_remove(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
) -> bool {
    match host_ops() {
        Some(ops) => remove_chain_plugin(&ops, host, track_id, plugin_index),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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

    #[derive(Default)]
    struct TestState {
        calls: Vec<String>,
        undo: Option<(ReplayPluginEditFn, usize)>,
        redo: Option<(ReplayPluginEditFn, usize)>,
        release: Option<(ReplayPluginEditFn, usize)>,
        plugin_count: u32,
    }

    // The host table is process-global, so these tests serialize on one lock.
    static SERIAL: Mutex<()> = Mutex::new(());
    static STATE: Mutex<TestState> = Mutex::new(TestState {
        calls: Vec::new(),
        undo: None,
        redo: None,
        release: None,
        plugin_count: 0,
    });

    fn reset() -> std::sync::MutexGuard<'static, TestState> {
        let mut state = STATE.lock().unwrap();
        state.calls.clear();
        state.undo = None;
        state.redo = None;
        state.release = None;
        state.plugin_count = 0;
        state
    }

    fn drain() -> Vec<String> {
        std::mem::take(&mut STATE.lock().unwrap().calls)
    }

    unsafe extern "C" fn fake_append(
        _host: *mut c_void,
        _track: u32,
        plugin_type: u32,
        path: *const u8,
        path_len: usize,
        prepare: bool,
        out_index: *mut u32,
    ) -> bool {
        let mut state = STATE.lock().unwrap();
        let kind = if path.is_null() {
            format!("append type={plugin_type}")
        } else {
            format!("append sandboxed len={path_len}")
        };
        state.calls.push(format!("{kind} prepare={prepare}"));
        state.plugin_count += 1;
        unsafe { *out_index = state.plugin_count - 1 };
        true
    }

    unsafe extern "C" fn fake_read(
        _host: *mut c_void,
        _track: u32,
        _index: u32,
        plugin_type: *mut u32,
        out: *mut u8,
        capacity: usize,
    ) -> usize {
        if !plugin_type.is_null() {
            unsafe { *plugin_type = 5 };
        }
        if !out.is_null() && capacity >= 2 {
            unsafe { std::ptr::copy_nonoverlapping(b"SS".as_ptr(), out, 2) };
        }
        2
    }

    unsafe extern "C" fn fake_query_bypass(
        _host: *mut c_void,
        _track: u32,
        _index: u32,
    ) -> i32 {
        1
    }

    unsafe extern "C" fn fake_apply_bypass(
        _host: *mut c_void,
        _track: u32,
        _index: u32,
        bypassed: bool,
    ) -> bool {
        STATE
            .lock()
            .unwrap()
            .calls
            .push(format!("bypass={bypassed}"));
        true
    }

    unsafe extern "C" fn fake_apply_state(
        _host: *mut c_void,
        _track: u32,
        _index: u32,
        state: *const u8,
        size: usize,
        _before: *const u8,
        _before_size: usize,
        _record: bool,
    ) -> bool {
        STATE
            .lock()
            .unwrap()
            .calls
            .push(format!("state len={size}"));
        true
    }

    unsafe extern "C" fn fake_remove(
        _host: *mut c_void,
        _track: u32,
        index: u32,
        out_snapshot: *mut *mut c_void,
    ) -> bool {
        let mut state = STATE.lock().unwrap();
        if out_snapshot.is_null() {
            state.calls.push(format!("remove {index}"));
        } else {
            state.calls.push(format!("remove {index} snapshot"));
            unsafe { *out_snapshot = 0xBEEF as *mut c_void };
        }
        true
    }

    unsafe extern "C" fn fake_restore(
        _host: *mut c_void,
        _track: u32,
        index: u32,
        snapshot: *mut c_void,
    ) -> bool {
        STATE
            .lock()
            .unwrap()
            .calls
            .push(format!("restore {index} {:?}", snapshot));
        true
    }

    unsafe extern "C" fn fake_release(snapshot: *mut c_void) {
        STATE
            .lock()
            .unwrap()
            .calls
            .push(format!("release {:?}", snapshot));
    }

    unsafe extern "C" fn fake_record(
        _host: *mut c_void,
        label: *const u8,
        label_len: usize,
        edit: *mut c_void,
        undo: ReplayPluginEditFn,
        redo: ReplayPluginEditFn,
        release: ReplayPluginEditFn,
    ) {
        let label = unsafe { std::slice::from_raw_parts(label, label_len) };
        let mut state = STATE.lock().unwrap();
        state
            .calls
            .push(format!("record {}", String::from_utf8_lossy(label)));
        state.undo = Some((undo, edit as usize));
        state.redo = Some((redo, edit as usize));
        state.release = Some((release, edit as usize));
    }

    unsafe extern "C" fn fake_sync(_host: *mut c_void) {
        STATE.lock().unwrap().calls.push("sync".to_string());
    }

    fn fake_ops() -> HirariPluginEditHostOps {
        HirariPluginEditHostOps {
            query_bypass: Some(fake_query_bypass),
            apply_bypass: Some(fake_apply_bypass),
            read_plugin: Some(fake_read),
            apply_plugin_state: Some(fake_apply_state),
            append_plugin: Some(fake_append),
            remove_plugin: Some(fake_remove),
            restore_plugin: Some(fake_restore),
            release_plugin_snapshot: Some(fake_release),
            record_plugin_replay: Some(fake_record),
            sync_structural_changes: Some(fake_sync),
            ..Default::default()
        }
    }

    fn pending() -> (
        (ReplayPluginEditFn, usize),
        (ReplayPluginEditFn, usize),
        (ReplayPluginEditFn, usize),
    ) {
        let state = STATE.lock().unwrap();
        (
            state.undo.unwrap(),
            state.redo.unwrap(),
            state.release.unwrap(),
        )
    }

    #[test]
    fn add_captures_state_and_replays_undo_and_redo() {
        let _guard = SERIAL.lock().unwrap();
        drop(reset());

        assert!(append_builtin(&fake_ops(), std::ptr::null_mut(), 4, 5));
        assert_eq!(
            drain(),
            vec!["append type=5 prepare=true", "record Add Plugin", "sync"],
            "the initial insert prepares the track and records one entry"
        );

        let ((undo, edit), (redo, _), (release, _)) = pending();
        unsafe { undo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["remove 0"], "undo removes the appended plugin");

        unsafe { redo(edit as *mut c_void) };
        assert_eq!(
            drain(),
            vec!["append type=5 prepare=false", "state len=2", "bypass=true"],
            "redo re-appends without re-preparing and restores state plus bypass"
        );

        unsafe { release(edit as *mut c_void) };
        assert!(drain().is_empty(), "an add entry owns no snapshot to release");
    }

    #[test]
    fn add_sandboxed_records_the_bundle_path() {
        let _guard = SERIAL.lock().unwrap();
        drop(reset());

        assert!(append_sandboxed(
            &fake_ops(),
            std::ptr::null_mut(),
            4,
            b"/tmp/Example.clap"
        ));
        assert_eq!(
            drain(),
            vec![
                "append sandboxed len=17 prepare=false",
                "record Add Sandboxed Plugin",
                "sync"
            ]
        );

        let ((_, edit), (redo, _), (release, _)) = pending();
        unsafe { redo(edit as *mut c_void) };
        assert_eq!(
            drain(),
            vec!["append sandboxed len=17 prepare=false", "state len=2", "bypass=true"]
        );
        unsafe { release(edit as *mut c_void) };
        assert!(drain().is_empty(), "a sandbox entry owns no snapshot");
    }

    #[test]
    fn unsupported_bundle_paths_are_rejected_before_the_engine() {
        let _guard = SERIAL.lock().unwrap();
        drop(reset());

        assert!(!append_sandboxed(
            &fake_ops(),
            std::ptr::null_mut(),
            4,
            b"/tmp/notes.txt"
        ));
        assert!(drain().is_empty());
    }

    #[test]
    fn remove_replays_the_captured_snapshot_and_releases_it() {
        let _guard = SERIAL.lock().unwrap();
        drop(reset());

        assert!(remove_chain_plugin(
            &fake_ops(),
            std::ptr::null_mut(),
            4,
            2
        ));
        assert_eq!(
            drain(),
            vec!["remove 2 snapshot", "record Remove Plugin", "sync"]
        );

        let ((undo, edit), (redo, _), (release, _)) = pending();
        unsafe { undo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["restore 2 0xbeef"]);

        unsafe { redo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["remove 2"]);

        unsafe { release(edit as *mut c_void) };
        assert_eq!(drain(), vec!["release 0xbeef"]);
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn bypass_without_native_recorder_uses_the_rust_history() {
        let _guard = SERIAL.lock().unwrap();
        drop(reset());
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = fake_ops();
        ops.record_bypass_undo = None;
        ops.undo_state = Some(fake_undo_state);

        assert!(set_bypass_with(
            &ops,
            std::ptr::null_mut(),
            4,
            2,
            false
        ));
        assert_eq!(drain(), vec!["bypass=false"]);
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        assert_eq!(drain(), vec!["bypass=true"]);

        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
}
