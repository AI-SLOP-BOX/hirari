//! Track lifecycle policy: create, duplicate and remove, with their undo
//! entries.
//!
//! The engine keeps the track vector, the audio-thread snapshot and the
//! routing graph because the callback reads them. This module owns the
//! decisions around them: it names a new track, decides what undo and redo do
//! for each lifecycle edit, and hands the entry to the history. C++ keeps the
//! mutations and owns the captured track until the history drops the entry.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};

use crate::plugin_edit_host::{commit_closures, UndoStateFn};
use crate::region_edit_host::{HirariRegionListRegionsFn, HirariRegionListTracksFn};

/// `Track::Type::Bus`, mirrored from `track_part_1a.inc`.
pub const TRACK_TYPE_BUS: u32 = 3;

/// Creates a track with the given display name and appends it. Writes the
/// insert index and returns a handle that keeps the created track alive.
pub type CreateTrackFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *const u8,
    usize,
    *mut usize,
) -> *mut c_void;
/// Deep-clones a track under the id the caller allocated, giving each copied
/// region the matching id from the caller's batch, appends the copy, writes its
/// insert index and returns a handle. Null when the source track is missing or
/// its region count no longer matches the batch.
pub type DuplicateTrackFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *const u32, usize, *mut usize) -> *mut c_void;
/// Takes a track out of the project together with its links and routes and
/// returns the handle that owns it. Null when the track is missing.
pub type TakeTrackFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
/// Removes a created or copied track again. `dropSidechainLinks` mirrors the
/// separate paths the engine had for create and duplicate; `dropRouting`
/// also clears the removed track's sidechain routes and connections.
pub type RemoveTrackFn = unsafe extern "C" fn(*mut c_void, *mut c_void, bool, bool) -> bool;
/// Re-inserts a captured track at its index, restoring the links and routes
/// captured by `take`. `stampGeneration` republishes the audio generation.
pub type RestoreTrackFn = unsafe extern "C" fn(*mut c_void, *mut c_void, bool) -> bool;
/// Frees a lifecycle handle. The handle keeps its track (and a taken track's
/// backups) alive until the history drops the entry.
pub type ReleaseTrackHandleFn = unsafe extern "C" fn(*mut c_void);
pub type ReplayTrackEditFn = unsafe extern "C" fn(*mut c_void);
/// Records an applied action; the host invokes `undo`/`redo` with `edit` and
/// releases it once the history drops the entry.
pub type RecordTrackReplayFn = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    *mut c_void,
    ReplayTrackEditFn,
    ReplayTrackEditFn,
    ReplayTrackEditFn,
);
/// Re-publishes the audio-thread track snapshot and rebuilds the graph.
pub type SyncStructuralChangesFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackEditHostOps {
    /// Lists track ids / one track's region ids in storage order; the id
    /// allocator in `id_host` reads the project through these.
    pub list_tracks: Option<HirariRegionListTracksFn>,
    pub list_regions: Option<HirariRegionListRegionsFn>,
    pub create: Option<CreateTrackFn>,
    pub duplicate: Option<DuplicateTrackFn>,
    pub take: Option<TakeTrackFn>,
    pub remove: Option<RemoveTrackFn>,
    pub restore: Option<RestoreTrackFn>,
    pub release_handle: Option<ReleaseTrackHandleFn>,
    pub record_replay: Option<RecordTrackReplayFn>,
    pub sync_structural_changes: Option<SyncStructuralChangesFn>,
    pub undo_state: Option<UndoStateFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariTrackEditHostOps {}
unsafe impl Sync for HirariTrackEditHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackEditHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackEditHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_edit_host_install(ops: *const HirariTrackEditHostOps) {
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

enum TrackEditKind {
    /// A track was created or duplicated; undo removes it, redo restores it.
    Created {
        handle: *mut c_void,
        drop_sidechain_links: bool,
        stamp_generation: bool,
    },
    /// A track was removed; undo restores it with its links, redo removes it.
    Removed { handle: *mut c_void },
}

/// One lifecycle undo entry. The host table is copied in so replay never
/// depends on the current global.
struct TrackEdit {
    ops: HirariTrackEditHostOps,
    host: *mut c_void,
    kind: TrackEditKind,
}

unsafe extern "C" fn replay_track_undo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const TrackEdit) };
    match &edit.kind {
        TrackEditKind::Created {
            handle,
            drop_sidechain_links,
            ..
        } => {
            if let Some(remove) = edit.ops.remove {
                unsafe { remove(edit.host, *handle, *drop_sidechain_links, false) };
            }
        }
        TrackEditKind::Removed { handle } => {
            if let Some(restore) = edit.ops.restore {
                unsafe { restore(edit.host, *handle, false) };
            }
        }
    }
}

unsafe extern "C" fn replay_track_redo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const TrackEdit) };
    match &edit.kind {
        TrackEditKind::Created {
            handle,
            stamp_generation,
            ..
        } => {
            if let Some(restore) = edit.ops.restore {
                unsafe { restore(edit.host, *handle, *stamp_generation) };
            }
        }
        TrackEditKind::Removed { handle } => {
            if let Some(remove) = edit.ops.remove {
                unsafe { remove(edit.host, *handle, true, true) };
            }
        }
    }
}

unsafe extern "C" fn release_track_edit(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { Box::from_raw(edit as *mut TrackEdit) };
    let handle = match edit.kind {
        TrackEditKind::Created { handle, .. } | TrackEditKind::Removed { handle } => handle,
    };
    if let Some(release) = edit.ops.release_handle {
        unsafe { release(handle) };
    }
}

struct OwnedTrackEdit {
    raw: *mut c_void,
    release: ReplayTrackEditFn,
}

impl Drop for OwnedTrackEdit {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { (self.release)(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

fn record_track_in_history(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    label: &str,
    edit: *mut c_void,
) {
    let Some(read_state) = ops.undo_state else {
        unsafe { release_track_edit(edit) };
        return;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        unsafe { release_track_edit(edit) };
        return;
    }
    let owned = Arc::new(OwnedTrackEdit {
        raw: edit,
        release: release_track_edit,
    });
    let (undo_owned, redo_owned) = (owned.clone(), owned.clone());
    commit_closures(
        state,
        label.as_bytes(),
        Box::new(move || unsafe { replay_track_undo(undo_owned.raw) }),
        Box::new(move || unsafe { replay_track_redo(redo_owned.raw) }),
    );
}

/// Hands one lifecycle edit to the history. Without a native recorder the
/// entry goes into the engine's Rust history; with no history the handle is
/// freed instead of leaked.
fn record_track_edit(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    label: &str,
    edit: TrackEdit,
) {
    let edit = Box::into_raw(Box::new(edit));
    match ops.record_replay {
        Some(record) => {
            unsafe {
                record(
                    host,
                    label.as_ptr(),
                    label.len(),
                    edit.cast(),
                    replay_track_undo,
                    replay_track_redo,
                    release_track_edit,
                )
            };
        }
        None => record_track_in_history(ops, host, label, edit.cast()),
    }
}

fn sync_structural_changes(ops: &HirariTrackEditHostOps, host: *mut c_void) {
    if let Some(sync) = ops.sync_structural_changes {
        unsafe { sync(host) };
    }
}

fn add_track_impl(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    track_type: u32,
) -> u32 {
    let Some(create) = ops.create else {
        return 0;
    };
    let id = crate::id_host::next_track_id(ops.list_tracks, host);
    if id == 0 {
        return 0;
    }
    let name = if track_type == TRACK_TYPE_BUS {
        format!("Bus {}", id.wrapping_add(1))
    } else {
        format!("Track {}", id.wrapping_add(1))
    };
    let mut index = 0_usize;
    let handle = unsafe {
        create(
            host,
            track_type,
            id,
            name.as_ptr(),
            name.len(),
            &mut index,
        )
    };
    if handle.is_null() {
        return 0;
    }
    record_track_edit(
        ops,
        host,
        "Add Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Created {
                handle,
                drop_sidechain_links: true,
                stamp_generation: true,
            },
        },
    );
    sync_structural_changes(ops, host);
    id
}

fn duplicate_track_impl(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    track_id: u32,
) -> u32 {
    let Some(duplicate) = ops.duplicate else {
        return 0;
    };
    let new_id = crate::id_host::next_track_id(ops.list_tracks, host);
    if new_id == 0 {
        return 0;
    }
    // The copy's regions each need an id of their own; the host checks that the
    // batch covers every region the source track holds before it clones them.
    let region_count = crate::id_host::region_count(ops.list_regions, host, track_id);
    let region_ids =
        crate::id_host::next_region_ids(ops.list_tracks, ops.list_regions, host, region_count);
    if region_ids.len() != region_count {
        return 0;
    }
    let mut index = 0_usize;
    let handle = unsafe {
        duplicate(
            host,
            track_id,
            new_id,
            region_ids.as_ptr(),
            region_ids.len(),
            &mut index,
        )
    };
    if handle.is_null() {
        return 0;
    }
    record_track_edit(
        ops,
        host,
        "Duplicate Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Created {
                handle,
                // The duplicate path never dropped sidechain links and never
                // stamped the audio generation, matching the engine exactly.
                drop_sidechain_links: false,
                stamp_generation: false,
            },
        },
    );
    sync_structural_changes(ops, host);
    new_id
}

fn remove_track_impl(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    track_id: u32,
) -> bool {
    let Some(take) = ops.take else {
        return false;
    };
    let handle = unsafe { take(host, track_id) };
    if handle.is_null() {
        return false;
    }
    // Removing the bootstrap placeholder (ID 1) from an otherwise empty project
    // rewinds the counter so the first track a document installs keeps its
    // canonical ID instead of becoming ID 2.
    if track_id == 1 {
        crate::id_host::rewind_track_counter(ops.list_tracks, host);
    }
    record_track_edit(
        ops,
        host,
        "Remove Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Removed { handle },
        },
    );
    sync_structural_changes(ops, host);
    true
}

/// Creates a track, names it from its allocated id and records one
/// `Add Track` entry.
///
/// The engine named the track `Track <id+1>`, or `Bus <id+1>` for a bus, and
/// recorded one entry whose undo removed it and redo re-inserted it at the
/// end. Returns the new id, or 0 when no id is available.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_add(host: *mut c_void, track_type: u32) -> u32 {
    match host_ops() {
        Some(ops) => add_track_impl(&ops, host, track_type),
        None => 0,
    }
}

/// Duplicates a track and records one `Duplicate Track` entry.
///
/// The copy kept the source's deep state (plugins, automation, regions, MIDI)
/// and was named `<source> Copy`; that storage stays in C++. Returns the new
/// id, or 0 when the source track is missing.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_duplicate(host: *mut c_void, track_id: u32) -> u32 {
    match host_ops() {
        Some(ops) => duplicate_track_impl(&ops, host, track_id),
        None => 0,
    }
}

/// Removes a track and records one `Remove Track` entry whose undo restores it
/// with its sidechain links and routing connections.
///
/// Returns false, recording nothing, when the track is missing.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_remove(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => remove_track_impl(&ops, host, track_id),
        None => false,
    }
}

/// Reads one processor state out of a chain, which the caller owns once copied.
fn chain_state(chain: *mut c_void, index: u32) -> Vec<u8> {
    let mut length = 0usize;
    let data = unsafe { crate::effect_chain_host::hirari_effect_chain_processor_state(chain, index, &mut length) };
    if data.is_null() || length == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length).to_vec() };
    unsafe { crate::effect_chain_host::hirari_effect_chain_buffer_free(data, length) };
    bytes
}

/// Reads one sandbox slot's path out of a chain.
fn chain_sandbox_path(chain: *mut c_void, slot: usize) -> Vec<u8> {
    let length =
        unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(chain, slot, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut path = vec![0u8; length];
    let written = unsafe {
        crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(
            chain,
            slot,
            path.as_mut_ptr(),
            length,
        )
    };
    path.truncate(written.min(length));
    path
}

/// Copies the store state a duplicated track inherits.
///
/// The engine builds the new track and hands over its storages; the name with
/// the copy suffix, the fader, the mute and phase flags, the three automation
/// lists, the plugin lanes and the chain in order are then read and written
/// store-to-store here. A sandbox path the destination chain refuses is skipped
/// like the engine's own walk did, so one rejected worker never aborts the
/// duplicate.
///
/// # Safety
/// Every handle must be a live storage of its kind, and the two handle sets must
/// belong to tracks of the same kind so the chain rebuild selects the same DSP
/// variants.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_track_clone_state(
    source_strip: *mut c_void,
    source_runtime: *mut c_void,
    source_automation: *mut c_void,
    source_chain: *mut c_void,
    source_name: *mut c_void,
    dest_strip: *mut c_void,
    dest_runtime: *mut c_void,
    dest_automation: *mut c_void,
    dest_chain: *mut c_void,
    dest_name: *mut c_void,
    dest_is_bus: bool,
) -> bool {
    if source_strip.is_null()
        || source_automation.is_null()
        || source_chain.is_null()
        || dest_strip.is_null()
        || dest_automation.is_null()
        || dest_chain.is_null()
    {
        return false;
    }
    // A name that cannot carry the suffix aborts the duplicate before any other
    // storage is touched.
    if !crate::track_name_store::copy_with_suffix(
        source_name as *const c_void,
        dest_name,
        b" Copy",
    ) {
        return false;
    }
    unsafe {
        crate::channel_strip::hirari_channel_strip_set_gain(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_gain(source_strip),
        );
        crate::channel_strip::hirari_channel_strip_set_pan(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_pan(source_strip),
        );
        crate::track_runtime::hirari_track_runtime_set_muted(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_muted(source_runtime) as u8,
        );
        crate::track_runtime::hirari_track_runtime_set_phase_invert(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_phase_inverted(source_runtime) as u8,
        );
        for kind in 0..3u32 {
            let mut count = 0usize;
            let points =
                crate::track_automation_store::hirari_track_automation_store_points(
                    source_automation, kind, &mut count,
                );
            crate::track_automation_store::hirari_track_automation_store_set_points(
                dest_automation, kind, points, count,
            );
        }
        let mut lane_count = 0usize;
        let lanes = crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(
            source_automation,
            &mut lane_count,
        );
        if lane_count != 0 {
            crate::track_automation_store::hirari_track_automation_store_set_plugin_lanes(
                dest_automation, lanes, lane_count,
            );
        }
        let slot_count = crate::effect_chain_host::hirari_effect_chain_sandbox_slot_count(source_chain);
        let mut sandbox_slot = 0usize;
        for index in 0..crate::effect_chain_host::hirari_effect_chain_count(source_chain) {
            let kind = crate::effect_chain_host::hirari_effect_chain_node_kind(source_chain, index);
            let state = chain_state(source_chain, index);
            let bypassed = crate::effect_chain_host::hirari_effect_chain_bypass(source_chain, index);
            let added = if kind == crate::effect_chain_nodes::SANDBOX_KIND {
                let path_index = sandbox_slot;
                sandbox_slot += 1;
                if path_index >= slot_count {
                    false
                } else {
                    let path = chain_sandbox_path(source_chain, path_index);
                    !path.is_empty()
                        && crate::effect_chain_host::hirari_effect_chain_add_sandbox(
                            dest_chain,
                            path.as_ptr(),
                            path.len(),
                        )
                }
            } else {
                crate::effect_chain_host::hirari_effect_chain_add_builtin(
                    dest_chain, kind, dest_is_bus, false,
                )
            };
            if added && !state.is_empty() {
                let dest_index =
                    crate::effect_chain_host::hirari_effect_chain_count(dest_chain) - 1;
                crate::effect_chain_host::hirari_effect_chain_processor_state_restore(
                    dest_chain,
                    dest_index,
                    state.as_ptr(),
                    state.len(),
                );
                crate::effect_chain_host::hirari_effect_chain_set_bypass(
                    dest_chain,
                    dest_index,
                    bypassed,
                );
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestState {
        calls: Vec<String>,
        track_ids: Vec<u32>,
        region_ids: Vec<u32>,
        undo: Option<(ReplayTrackEditFn, usize)>,
        redo: Option<(ReplayTrackEditFn, usize)>,
        release: Option<(ReplayTrackEditFn, usize)>,
    }

    static SERIAL: Mutex<()> = Mutex::new(());
    static STATE: Mutex<TestState> = Mutex::new(TestState {
        calls: Vec::new(),
        track_ids: Vec::new(),
        region_ids: Vec::new(),
        undo: None,
        redo: None,
        release: None,
    });

    fn reset() {
        let mut state = STATE.lock().unwrap();
        state.calls.clear();
        state.track_ids.clear();
        state.region_ids.clear();
        state.undo = None;
        state.redo = None;
        state.release = None;
    }

    fn drain() -> Vec<String> {
        std::mem::take(&mut STATE.lock().unwrap().calls)
    }

    fn call(entry: &str) {
        STATE.lock().unwrap().calls.push(entry.to_string());
    }

    /// Gives the project the ids it holds and restarts the counters, so the ids
    /// the allocator hands out next are the same in every run.
    fn project(host: usize, track_ids: &[u32], region_ids: &[u32]) -> *mut c_void {
        {
            let mut state = STATE.lock().unwrap();
            state.track_ids = track_ids.to_vec();
            state.region_ids = region_ids.to_vec();
        }
        let host = host as *mut c_void;
        unsafe { crate::id_host::hirari_ids_reset_project(host) };
        host
    }

    fn fill(ids: &[u32], out: *mut u32, capacity: usize) -> usize {
        if out.is_null() || capacity < ids.len() {
            return ids.len();
        }
        unsafe { std::ptr::copy_nonoverlapping(ids.as_ptr(), out, ids.len()) };
        ids.len()
    }

    unsafe extern "C" fn fake_list_tracks(
        _host: *mut c_void,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let state = STATE.lock().unwrap();
        fill(&state.track_ids.clone(), out, capacity)
    }

    unsafe extern "C" fn fake_list_regions(
        _host: *mut c_void,
        _track_id: u32,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let state = STATE.lock().unwrap();
        fill(&state.region_ids.clone(), out, capacity)
    }

    unsafe extern "C" fn fake_create(
        _host: *mut c_void,
        track_type: u32,
        id: u32,
        name: *const u8,
        name_len: usize,
        out_index: *mut usize,
    ) -> *mut c_void {
        let name = unsafe { std::slice::from_raw_parts(name, name_len) };
        call(&format!(
            "create type={track_type} id={id} name={}",
            String::from_utf8_lossy(name)
        ));
        unsafe { *out_index = 4 };
        0xC0DE as *mut c_void
    }

    unsafe extern "C" fn fake_duplicate(
        _host: *mut c_void,
        track_id: u32,
        new_id: u32,
        region_ids: *const u32,
        region_id_count: usize,
        out_index: *mut usize,
    ) -> *mut c_void {
        let ids = unsafe { std::slice::from_raw_parts(region_ids, region_id_count) }.to_vec();
        call(&format!("duplicate {track_id} -> {new_id} regions={ids:?}"));
        unsafe { *out_index = 2 };
        0xCAFE as *mut c_void
    }

    unsafe extern "C" fn fake_take(_host: *mut c_void, track_id: u32) -> *mut c_void {
        call(&format!("take {track_id}"));
        if track_id == 1 {
            // The bootstrap placeholder is the last track of a fresh project, so
            // taking it leaves the listing empty for the counter rewind to read.
            STATE.lock().unwrap().track_ids.clear();
        }
        0xF00D as *mut c_void
    }

    unsafe extern "C" fn fake_remove(
        _host: *mut c_void,
        handle: *mut c_void,
        drop_links: bool,
        drop_routing: bool,
    ) -> bool {
        call(&format!(
            "remove {handle:?} links={drop_links} routing={drop_routing}"
        ));
        true
    }

    unsafe extern "C" fn fake_restore(
        _host: *mut c_void,
        handle: *mut c_void,
        stamp: bool,
    ) -> bool {
        call(&format!("restore {handle:?} stamp={stamp}"));
        true
    }

    unsafe extern "C" fn fake_release(handle: *mut c_void) {
        call(&format!("release {handle:?}"));
    }

    unsafe extern "C" fn fake_record(
        _host: *mut c_void,
        label: *const u8,
        label_len: usize,
        edit: *mut c_void,
        undo: ReplayTrackEditFn,
        redo: ReplayTrackEditFn,
        release: ReplayTrackEditFn,
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
        call("sync");
    }

    fn fake_ops() -> HirariTrackEditHostOps {
        HirariTrackEditHostOps {
            list_tracks: Some(fake_list_tracks),
            list_regions: Some(fake_list_regions),
            create: Some(fake_create),
            duplicate: Some(fake_duplicate),
            take: Some(fake_take),
            remove: Some(fake_remove),
            restore: Some(fake_restore),
            release_handle: Some(fake_release),
            record_replay: Some(fake_record),
            sync_structural_changes: Some(fake_sync),
            undo_state: None,
        }
    }

    fn pending() -> (
        (ReplayTrackEditFn, usize),
        (ReplayTrackEditFn, usize),
        (ReplayTrackEditFn, usize),
    ) {
        let state = STATE.lock().unwrap();
        (
            state.undo.unwrap(),
            state.redo.unwrap(),
            state.release.unwrap(),
        )
    }

    /// Six ids already taken, so the next track the allocator hands out is 7.
    const TAKEN_TRACKS: [u32; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn add_names_tracks_and_replays_undo_and_redo() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();
        let host = project(0x100, &TAKEN_TRACKS, &[]);

        assert_eq!(add_track_impl(&ops, host, 0), 7);
        assert_eq!(
            drain(),
            vec!["create type=0 id=7 name=Track 8", "record Add Track", "sync"]
        );

        let ((undo, edit), (redo, _), (release, _)) = pending();
        unsafe { undo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["remove 0xc0de links=true routing=false"]);

        unsafe { redo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["restore 0xc0de stamp=true"]);

        unsafe { release(edit as *mut c_void) };
        assert_eq!(drain(), vec!["release 0xc0de"]);
    }

    #[test]
    fn buses_are_named_from_the_allocated_id() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();

        assert_eq!(
            add_track_impl(&ops, project(0x200, &TAKEN_TRACKS, &[]), TRACK_TYPE_BUS),
            7
        );
        assert_eq!(
            drain(),
            vec!["create type=3 id=7 name=Bus 8", "record Add Track", "sync"]
        );
    }

    #[test]
    fn duplicate_replays_without_generation_or_link_changes() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();
        let host = project(0x300, &TAKEN_TRACKS, &[1, 2]);

        assert_eq!(duplicate_track_impl(&ops, host, 4), 7);
        assert_eq!(
            drain(),
            vec![
                "duplicate 4 -> 7 regions=[3, 4]",
                "record Duplicate Track",
                "sync"
            ]
        );

        let ((undo, edit), (redo, _), (release, _)) = pending();
        unsafe { undo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["remove 0xcafe links=false routing=false"]);

        unsafe { redo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["restore 0xcafe stamp=false"]);

        unsafe { release(edit as *mut c_void) };
        assert_eq!(drain(), vec!["release 0xcafe"]);
    }

    #[test]
    fn removal_restores_the_taken_track_on_undo() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();
        let host = project(0x400, &[4], &[1]);

        assert!(remove_track_impl(&ops, host, 4));
        assert_eq!(drain(), vec!["take 4", "record Remove Track", "sync"]);

        let ((undo, edit), (redo, _), (release, _)) = pending();
        unsafe { undo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["restore 0xf00d stamp=false"]);

        unsafe { redo(edit as *mut c_void) };
        assert_eq!(drain(), vec!["remove 0xf00d links=true routing=true"]);

        unsafe { release(edit as *mut c_void) };
        assert_eq!(drain(), vec!["release 0xf00d"]);
    }

    #[test]
    fn removing_the_bootstrap_placeholder_reuses_its_id() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();
        let host = project(0x900, &[1], &[]);

        assert!(remove_track_impl(&ops, host, 1));
        assert_eq!(crate::id_host::next_track_id(ops.list_tracks, host), 1);
    }

    #[test]
    fn a_missing_track_records_nothing() {        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = HirariTrackEditHostOps {
            take: Some(fake_take_missing),
            record_replay: Some(fake_record),
            sync_structural_changes: Some(fake_sync),
            ..Default::default()
        };
        assert!(!remove_track_impl(&ops, project(0xA00, &TAKEN_TRACKS, &[]), 4));
        assert_eq!(drain(), vec!["take 4"]);
    }

    unsafe extern "C" fn fake_take_missing(
        _host: *mut c_void,
        track_id: u32,
    ) -> *mut c_void {
        call(&format!("take {track_id}"));
        std::ptr::null_mut()
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn add_track_without_native_recorder_uses_the_rust_history() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = fake_ops();
        ops.record_replay = None;
        ops.undo_state = Some(fake_undo_state);
        let host = project(0xB00, &TAKEN_TRACKS, &[]);
        assert_eq!(add_track_impl(&ops, host, 0), 7);
        assert_eq!(
            drain(),
            vec!["create type=0 id=7 name=Track 8", "sync"]
        );
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        assert_eq!(
            drain(),
            vec!["remove 0xc0de links=true routing=false"]
        );
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        assert_eq!(drain(), vec!["release 0xc0de"]);
        FAKE_UNDO.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }

    use crate::channel_strip::{
        hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_gain,
        hirari_channel_strip_pan, hirari_channel_strip_set_gain, hirari_channel_strip_set_pan,
    };
    use crate::effect_chain_host::{
        hirari_effect_chain_add_builtin, hirari_effect_chain_bypass, hirari_effect_chain_count,
        hirari_effect_chain_create, hirari_effect_chain_destroy, hirari_effect_chain_node_kind,
        hirari_effect_chain_set_bypass,
    };
    use crate::track_automation_store::{
        hirari_track_automation_store_create, hirari_track_automation_store_destroy,
        hirari_track_automation_store_points, hirari_track_automation_store_plugin_lane_views,
        hirari_track_automation_store_set_plugin_lanes, hirari_track_automation_store_set_points,
        AutomationPoint,
    };
    use crate::track_runtime::{
        hirari_track_runtime_create, hirari_track_runtime_destroy,
        hirari_track_runtime_is_muted, hirari_track_runtime_is_phase_inverted,
        hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
    };

    /// One track's Rust storages, destroyed in the order the runtime releases
    /// the handles it borrows.
    struct Stores {
        strip: *mut c_void,
        chain: *mut c_void,
        automation: *mut c_void,
        runtime: *mut c_void,
        name: *mut c_void,
    }

    impl Stores {
        fn new(id: u32) -> Self {
            let strip = unsafe { hirari_channel_strip_create() };
            let chain = unsafe { hirari_effect_chain_create() };
            let automation = unsafe { hirari_track_automation_store_create() };
            let name = unsafe { crate::track_name_store::hirari_track_name_store_create() };
            let runtime = unsafe {
                hirari_track_runtime_create(
                    id, chain, strip, automation, std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), None, std::ptr::null_mut(),
                )
            };
            Self { strip, chain, automation, runtime, name }
        }
        fn set_name(&self, name: &[u8]) {
            assert!(unsafe {
                crate::track_name_store::hirari_track_name_store_set(
                    self.name,
                    name.as_ptr(),
                    name.len(),
                )
            });
        }
        fn name(&self) -> Vec<u8> {
            crate::track_name_store::read(self.name as *const c_void)
        }
    }

    impl Drop for Stores {
        fn drop(&mut self) {
            unsafe {
                hirari_track_runtime_destroy(self.runtime);
                hirari_track_automation_store_destroy(self.automation);
                hirari_effect_chain_destroy(self.chain);
                hirari_channel_strip_destroy(self.strip);
                crate::track_name_store::hirari_track_name_store_destroy(self.name);
            }
        }
    }

    fn points(store: *mut c_void, kind: u32) -> Vec<AutomationPoint> {
        let mut count = 0usize;
        let raw = unsafe { hirari_track_automation_store_points(store, kind, &mut count) };
        if raw.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(raw, count) }.to_vec()
    }

    #[test]
    fn cloning_state_copies_stores_onto_a_fresh_track() {
        let source = Stores::new(1);
        let dest = Stores::new(2);
        unsafe {
            hirari_channel_strip_set_gain(source.strip, 0.5);
            hirari_channel_strip_set_pan(source.strip, -0.25);
            hirari_channel_strip_set_gain(dest.strip, 1.0);
            hirari_track_runtime_set_muted(source.runtime, 1);
            hirari_track_runtime_set_phase_invert(source.runtime, 1);
            let volume = [
                AutomationPoint { time: 4.0, value: 0.2, curve: 0.0 },
                AutomationPoint { time: 1.0, value: 0.8, curve: 0.5 },
            ];
            hirari_track_automation_store_set_points(source.automation, 0, volume.as_ptr(), 2);
            let delay = [AutomationPoint { time: 2.0, value: 0.5, curve: 0.0 }];
            hirari_track_automation_store_set_points(
                source.automation,
                2,
                delay.as_ptr(),
                delay.len(),
            );
            let lane = [AutomationPoint { time: 3.0, value: 0.75, curve: 0.25 }];
            let lanes = [crate::track_automation_store::PluginLaneView {
                processor_index: 1,
                parameter_id: 7,
                points: lane.as_ptr(),
                point_count: 1,
            }];
            hirari_track_automation_store_set_plugin_lanes(
                source.automation,
                lanes.as_ptr(),
                lanes.len(),
            );
            assert!(hirari_effect_chain_add_builtin(source.chain, 1, false, false));
            assert!(hirari_effect_chain_add_builtin(source.chain, 4, false, false));
            assert!(hirari_effect_chain_set_bypass(source.chain, 1, true));
            source.set_name(b"Guitar");
        }
        // The chain bypass and processor state follow the source node, so the
        // duplicate reports the same insert list and the same parameter values.
        unsafe {
            assert!(hirari_track_clone_state(
                source.strip,
                source.runtime,
                source.automation,
                source.chain,
                source.name,
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            ));
        }
        assert_eq!(dest.name(), b"Guitar Copy".to_vec());
        assert_eq!(source.name(), b"Guitar".to_vec());
        assert_eq!(unsafe { hirari_channel_strip_gain(dest.strip) }, 0.5);
        assert_eq!(unsafe { hirari_channel_strip_pan(dest.strip) }, -0.25);
        assert!(unsafe { hirari_track_runtime_is_muted(dest.runtime) });
        assert!(unsafe { hirari_track_runtime_is_phase_inverted(dest.runtime) });
        // The automation store sorts by time as it publishes.
        assert_eq!(
            points(dest.automation, 0),
            vec![
                AutomationPoint { time: 1.0, value: 0.8, curve: 0.5 },
                AutomationPoint { time: 4.0, value: 0.2, curve: 0.0 },
            ]
        );
        assert_eq!(points(dest.automation, 1), Vec::new());
        assert_eq!(
            points(dest.automation, 2),
            vec![AutomationPoint { time: 2.0, value: 0.5, curve: 0.0 }]
        );
        let mut lane_count = 0usize;
        let views =
            unsafe { hirari_track_automation_store_plugin_lane_views(dest.automation, &mut lane_count) };
        assert_eq!(lane_count, 1);
        let lane = unsafe { &*views };
        assert_eq!((lane.processor_index, lane.parameter_id, lane.point_count), (1, 7, 1));
        assert_eq!(unsafe { hirari_effect_chain_count(dest.chain) }, 2);
        assert_eq!(
            unsafe {
                (
                    hirari_effect_chain_node_kind(dest.chain, 0),
                    hirari_effect_chain_node_kind(dest.chain, 1),
                )
            },
            (1, 4)
        );
        assert_eq!(chain_state(dest.chain, 1), chain_state(source.chain, 1));
        assert!(!unsafe { hirari_effect_chain_bypass(dest.chain, 0) });
        assert!(unsafe { hirari_effect_chain_bypass(dest.chain, 1) });
    }

    #[test]
    fn cloning_state_without_a_source_chain_copies_nothing() {
        let dest = Stores::new(2);
        assert!(!unsafe {
            hirari_track_clone_state(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            )
        });
    }

    #[test]
    fn a_name_that_cannot_take_the_suffix_aborts_the_clone() {
        let source = Stores::new(1);
        let dest = Stores::new(2);
        source.set_name(&vec![b'a'; crate::track_name_store::MAX_TRACK_NAME_BYTES]);
        dest.set_name(b"keep");
        unsafe { hirari_channel_strip_set_gain(source.strip, 0.5) };
        assert!(!unsafe {
            hirari_track_clone_state(
                source.strip,
                source.runtime,
                source.automation,
                source.chain,
                source.name,
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            )
        });
        // Nothing crossed, because the name is settled before the stores are.
        assert_eq!(unsafe { hirari_channel_strip_gain(dest.strip) }, 1.0);
        assert_eq!(dest.name(), b"keep".to_vec());
    }
}
