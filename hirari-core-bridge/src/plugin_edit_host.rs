//! Plugin-chain edit policy: bypass, reordering, parameters, presets and the
//! add/remove lifecycle.
//!
//! The chain, the frozen render and the layout revision are Rust state the track
//! already owns, so this module reaches them through the shared track session
//! (`track_session.rs`) instead of one host callback per field. The session takes
//! the engine's tracks lock and resolves the track; this module decides whether
//! an edit is admitted, applies it to the chain, keeps the drop-the-freeze and
//! bump-the-layout protocol, and records exactly one undo entry built from the
//! pre/post values.
//!
//! Two steps really are engine code and stay host callbacks: re-publishing the
//! track snapshot after the chain changed, and reading the engine's undo handle.
//! A replayed entry runs after the session that recorded it is gone, so it opens
//! its own session; the history resyncs the graph, which needs the tracks lock.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::effect_chain_host::{
    hirari_effect_chain_add_builtin, hirari_effect_chain_add_sandbox, hirari_effect_chain_bypass,
    hirari_effect_chain_buffer_free, hirari_effect_chain_count, hirari_effect_chain_move,
    hirari_effect_chain_node_kind, hirari_effect_chain_parameter,
    hirari_effect_chain_processor_state, hirari_effect_chain_processor_state_restore,
    hirari_effect_chain_remove, hirari_effect_chain_restore, hirari_effect_chain_set_bypass,
    hirari_effect_chain_set_parameter,
};
use crate::track_session::TrackSession;
use crate::undo_transaction::{
    hirari_undo_manager_commit, hirari_undo_manager_discard_native_callback,
    hirari_undo_manager_register_native_callback,
};

/// Re-publishes the track snapshot after the chain changed.
pub type SyncStructuralChangesFn = unsafe extern "C" fn(*mut c_void);
/// Returns the engine's Rust undo history handle.
pub type UndoStateFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariPluginEditHostOps {
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
pub unsafe extern "C" fn hirari_plugin_edit_host_install(ops: *const HirariPluginEditHostOps) {
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

pub(crate) fn register_replay(state: *mut c_void, callback: Box<dyn Fn()>) -> u64 {
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

/// Registers two replay closures as one entry in the engine's Rust history.
/// The edit was already applied, so the redo is not replayed.
pub(crate) fn commit_closures(
    state: *mut c_void,
    name: &[u8],
    undo: Box<dyn Fn()>,
    redo: Box<dyn Fn()>,
) -> bool {
    let undo_id = register_replay(state, undo);
    if undo_id == 0 {
        return false;
    }
    let redo_id = register_replay(state, redo);
    if redo_id == 0 {
        unsafe { hirari_undo_manager_discard_native_callback(state, undo_id) };
        return false;
    }
    unsafe {
        hirari_undo_manager_commit(state, name.as_ptr(), name.len(), undo_id, redo_id, false, false)
    }
}

/// Records an edit the caller already applied. A missing history keeps the edit
/// but records nothing, exactly as the engine did when no undo manager existed.
fn record_applied(
    host: *mut c_void,
    name: &str,
    undo: impl Fn() + 'static,
    redo: impl Fn() + 'static,
) -> bool {
    let Some(read_state) = host_ops().and_then(|ops| ops.undo_state) else {
        return false;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return false;
    }
    commit_closures(state, name.as_bytes(), Box::new(undo), Box::new(redo))
}

fn sync_structural_changes(host: *mut c_void) {
    if let Some(sync) = host_ops().and_then(|ops| ops.sync_structural_changes) {
        unsafe { sync(host) };
    }
}

/// Upper bound for a plugin state read, matching the preset file limit.
const MAX_PLUGIN_STATE_BYTES: usize = 16 * 1024 * 1024;

/// A plugin's type and serialized state, read from the track's own chain.
/// `None` means the track had no plugin at that index.
fn read_chain_state(chain: *mut c_void, index: u32) -> Option<(u32, Vec<u8>)> {
    if chain.is_null() || index >= unsafe { hirari_effect_chain_count(chain as *const c_void) } {
        return None;
    }
    let plugin_type = unsafe { hirari_effect_chain_node_kind(chain as *const c_void, index) };
    let mut length = 0_usize;
    let data = unsafe { hirari_effect_chain_processor_state(chain, index, &mut length) };
    if length > MAX_PLUGIN_STATE_BYTES {
        unsafe { hirari_effect_chain_buffer_free(data, length) };
        return None;
    }
    let state = if data.is_null() {
        Vec::new()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(data, length) }.to_vec();
        unsafe { hirari_effect_chain_buffer_free(data, length) };
        bytes
    };
    Some((plugin_type, state))
}

/// Runs one read/write against the track's chain under a session and applies the
/// edit protocol to the reported change. `None` is a missing track.
fn chain_edit<R>(
    host: *mut c_void,
    track_id: u32,
    edit: impl FnOnce(*mut c_void, &TrackSession) -> (bool, R),
) -> Option<R> {
    let session = TrackSession::open(host, track_id)?;
    let (changed, value) = edit(session.chain(), &session);
    session.commit_edit(changed);
    Some(value)
}

fn apply_bypass(host: *mut c_void, track_id: u32, index: u32, bypassed: bool) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        session.commit_edit(unsafe { hirari_effect_chain_set_bypass(chain, index, bypassed) });
    }
}

fn apply_parameter(
    host: *mut c_void,
    track_id: u32,
    index: u32,
    parameter_id: u32,
    value: f32,
) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        session.commit_edit(unsafe {
            hirari_effect_chain_set_parameter(chain, index, parameter_id, value)
        });
    }
}

fn apply_move(host: *mut c_void, track_id: u32, from: u32, to: u32) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        session.commit_edit(unsafe { hirari_effect_chain_move(chain, from, to) });
    }
}

fn apply_state(host: *mut c_void, track_id: u32, index: u32, state: &[u8]) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        session.commit_edit(unsafe {
            hirari_effect_chain_processor_state_restore(chain, index, state.as_ptr(), state.len())
        });
    }
}

/// One processor removed from a chain, addressed by the token the chain issued
/// for it. The chain keeps the node alive until it is restored or the track dies,
/// so a history entry carries no processor reference.
#[derive(Clone, Copy)]
struct CapturedPlugin {
    token: u32,
    bypassed: bool,
    parallel: bool,
}

fn replay_remove(host: *mut c_void, track_id: u32, index: u32) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        let mut bypassed = false;
        let mut parallel = false;
        let mut plugin_type = 0_u32;
        let mut token = 0_u32;
        session.commit_edit(unsafe {
            hirari_effect_chain_remove(
                chain,
                index,
                &mut bypassed,
                &mut parallel,
                &mut plugin_type,
                &mut token,
            )
        });
    }
}

fn replay_restore(host: *mut c_void, track_id: u32, index: u32, snapshot: CapturedPlugin) {
    if let Some(session) = TrackSession::open(host, track_id) {
        let chain = session.chain();
        session.commit_edit(unsafe {
            hirari_effect_chain_restore(
                chain,
                index,
                snapshot.token,
                snapshot.bypassed,
                snapshot.parallel,
            )
        });
    }
}

/// How a plugin gets back into a chain: built-ins are re-created by type, a
/// sandboxed bundle by its path.
enum AppendKind<'a>
{
    Builtin(u32),
    Sandboxed(&'a [u8]),
}

/// What inserting a plugin left behind for the history to replay: the index it
/// took plus the settings a fresh node does not carry.
struct Appended {
    index: u32,
    state: Vec<u8>,
    bypassed: bool,
}

/// Inserts a plugin, keeps the edit protocol and re-prepares the track when the
/// caller is adding one for the user (a replayed insert reuses the prepared
/// chain the track already has).
fn insert_plugin(
    host: *mut c_void,
    track_id: u32,
    kind: &AppendKind<'_>,
    prepare: bool,
) -> Option<Appended> {
    let session = TrackSession::open(host, track_id)?;
    let chain = session.chain();
    let inserted = match kind {
        AppendKind::Builtin(plugin_type) => unsafe {
            hirari_effect_chain_add_builtin(chain, *plugin_type, session.is_bus(), false)
        },
        AppendKind::Sandboxed(path) => unsafe {
            hirari_effect_chain_add_sandbox(chain, path.as_ptr(), path.len())
        },
    };
    if !session.commit_edit(inserted) {
        return None;
    }
    if prepare {
        session.prepare();
    }
    let index = unsafe { hirari_effect_chain_count(chain as *const c_void) }.saturating_sub(1);
    Some(Appended {
        index,
        state: read_chain_state(chain, index).map_or_else(Vec::new, |(_, state)| state),
        bypassed: unsafe { hirari_effect_chain_bypass(chain as *const c_void, index) },
    })
}

fn replay_append(host: *mut c_void, track_id: u32, kind: AppendKind<'_>, capture: &Appended) {
    let Some(appended) = insert_plugin(host, track_id, &kind, false) else {
        return;
    };
    if !capture.state.is_empty() {
        apply_state(host, track_id, appended.index, &capture.state);
    }
    apply_bypass(host, track_id, appended.index, capture.bypassed);
}

/// Sets a plugin's bypass flag. An out-of-range plugin index is rejected before
/// anything is recorded; a same-value set still records one entry, as the engine
/// did before the policy moved here.
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
    let Some((before, applied)) = chain_edit(host, track_id, |chain, _| {
        let before = unsafe { hirari_effect_chain_bypass(chain as *const c_void, plugin_index) };
        let applied = unsafe { hirari_effect_chain_set_bypass(chain, plugin_index, bypassed) };
        (applied, (before, applied))
    }) else {
        return false;
    };
    if !applied {
        return false;
    }
    record_applied(
        host,
        "Set Plugin Bypass",
        move || apply_bypass(host, track_id, plugin_index, before),
        move || apply_bypass(host, track_id, plugin_index, bypassed),
    );
    true
}

/// Moves a plugin inside its track's chain. Moving onto the same index is
/// rejected, matching the chain's contract; a rejected move records nothing.
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
    let Some(applied) = chain_edit(host, track_id, |chain, _| {
        let applied = unsafe { hirari_effect_chain_move(chain, from_index, to_index) };
        (applied, applied)
    }) else {
        return false;
    };
    if !applied {
        return false;
    }
    record_applied(
        host,
        "Move Plugin",
        move || apply_move(host, track_id, to_index, from_index),
        move || apply_move(host, track_id, from_index, to_index),
    );
    true
}

/// Sets one plugin parameter. Non-finite values are rejected before the chain is
/// touched; a rejected plugin records nothing. The undo pair keeps the exact
/// requested value rather than a clamped read-back, as the engine did.
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
    let Some((before, applied)) = chain_edit(host, track_id, |chain, _| {
        let before =
            unsafe { hirari_effect_chain_parameter(chain as *const c_void, plugin_index, parameter_id) };
        let applied =
            unsafe { hirari_effect_chain_set_parameter(chain, plugin_index, parameter_id, value) };
        (applied, (before, applied))
    }) else {
        return false;
    };
    if !applied {
        return false;
    }
    record_applied(
        host,
        "Set Plugin Parameter",
        move || apply_parameter(host, track_id, plugin_index, parameter_id, before),
        move || apply_parameter(host, track_id, plugin_index, parameter_id, value),
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
    chain_edit(host, track_id, |chain, _| {
        let applied =
            unsafe { hirari_effect_chain_set_parameter(chain, plugin_index, parameter_id, value) };
        (applied, applied)
    })
    .unwrap_or(false)
}

/// Owns the loaded preset until the request finishes.
struct PresetFile(*mut crate::plugin_preset_file::PluginPresetFile);

impl Drop for PresetFile {
    fn drop(&mut self) {
        unsafe { crate::plugin_preset_file::hirari_plugin_preset_file_free(self.0) };
    }
}

/// Loads a preset file into a plugin and records one state-swap undo entry.
///
/// A preset whose type differs from the plugin, a plugin that does not exist and
/// a failed state restore are all rejected without recording.
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
    let after = unsafe { std::slice::from_raw_parts(data, size) }.to_vec();
    #[derive(Default)]
    struct Step {
        before: Vec<u8>,
        applied: bool,
    }
    let step = chain_edit(host, track_id, |chain, _| match read_chain_state(chain, plugin_index) {
        None => (false, Step::default()),
        Some((plugin_type, _)) if plugin_type != preset_type => (false, Step::default()),
        Some((_, before)) => {
            let applied = unsafe {
                hirari_effect_chain_processor_state_restore(
                    chain,
                    plugin_index,
                    after.as_ptr(),
                    after.len(),
                )
            };
            (applied, Step { before, applied })
        }
    });
    let Some(step) = step else {
        return false;
    };
    if !step.applied {
        return false;
    }
    // The history keeps both snapshots; replay stores them without recording
    // another entry.
    let undo_state = step.before;
    let redo_state = after;
    record_applied(
        host,
        "Load Plugin Preset",
        move || apply_state(host, track_id, plugin_index, &undo_state),
        move || apply_state(host, track_id, plugin_index, &redo_state),
    );
    true
}

/// Writes a plugin's current state as a preset file.
///
/// An empty path, a missing plugin and an empty state are rejected before the
/// bytes reach the preset writer.
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
    let Some(read) =
        chain_edit(host, track_id, |chain, _| (false, read_chain_state(chain, plugin_index)))
    else {
        return false;
    };
    let Some((plugin_type, state)) = read else {
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

/// Reads a plugin's state. Returns the state length; `out` is only written when
/// it can hold the whole state. A missing track or plugin reports zero.
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
    let Some(read) =
        chain_edit(host, track_id, |chain, _| (false, read_chain_state(chain, plugin_index)))
    else {
        return 0;
    };
    let Some((_, state)) = read else {
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
    let bytes = if size == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(state, size) }
    };
    chain_edit(host, track_id, |chain, _| {
        let applied = unsafe {
            hirari_effect_chain_processor_state_restore(chain, plugin_index, bytes.as_ptr(), bytes.len())
        };
        (applied, applied)
    })
    .unwrap_or(false)
}

/// Appends a built-in plugin and records one `Add Plugin` undo entry.
///
/// A missing track and a failed insert are rejected without recording; the entry
/// captures the appended state and bypass so undo removes it and redo re-appends
/// it with the same settings.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_edit_add(
    host: *mut c_void,
    track_id: u32,
    plugin_type: u32,
) -> bool {
    let Some(appended) = insert_plugin(host, track_id, &AppendKind::Builtin(plugin_type), true)
    else {
        return false;
    };
    let capture = Appended {
        index: appended.index,
        state: appended.state,
        bypassed: appended.bypassed,
    };
    let undo_capture = Appended {
        index: capture.index,
        state: capture.state.clone(),
        bypassed: capture.bypassed,
    };
    let redo_capture = capture;
    record_applied(
        host,
        "Add Plugin",
        move || replay_remove(host, track_id, undo_capture.index),
        move || replay_append(host, track_id, AppendKind::Builtin(plugin_type), &redo_capture),
    );
    sync_structural_changes(host);
    true
}

/// Appends a sandboxed bundle and records one `Add Sandboxed Plugin` entry.
///
/// The accepted bundle formats are checked before the track is touched, exactly
/// as the engine did with the shared path policy.
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
    let bytes = unsafe { std::slice::from_raw_parts(path, path_length) };
    if !unsafe {
        crate::plugin_sandbox_host::hirari_plugin_sandbox_path_supported(bytes.as_ptr(), bytes.len())
    } {
        return false;
    }
    let Some(appended) = insert_plugin(host, track_id, &AppendKind::Sandboxed(bytes), false) else {
        return false;
    };
    let path = bytes.to_vec();
    let undo_capture = Appended {
        index: appended.index,
        state: appended.state.clone(),
        bypassed: appended.bypassed,
    };
    let redo_capture = appended;
    let redo_path = path.clone();
    record_applied(
        host,
        "Add Sandboxed Plugin",
        move || replay_remove(host, track_id, undo_capture.index),
        move || replay_append(host, track_id, AppendKind::Sandboxed(&redo_path), &redo_capture),
    );
    sync_structural_changes(host);
    true
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
    let removed = chain_edit(host, track_id, |chain, _| {
        let mut bypassed = false;
        let mut parallel = false;
        let mut kind = 0_u32;
        let mut token = 0_u32;
        let applied = unsafe {
            hirari_effect_chain_remove(
                chain,
                plugin_index,
                &mut bypassed,
                &mut parallel,
                &mut kind,
                &mut token,
            )
        };
        (
            applied,
            applied.then_some(CapturedPlugin {
                token,
                bypassed,
                parallel,
            }),
        )
    });
    let Some(Some(snapshot)) = removed else {
        return false;
    };
    record_applied(
        host,
        "Remove Plugin",
        move || replay_restore(host, track_id, plugin_index, snapshot),
        move || replay_remove(host, track_id, plugin_index),
    );
    sync_structural_changes(host);
    true
}
