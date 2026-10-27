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
use std::sync::{Mutex, OnceLock};

/// `Track::Type::Bus`, mirrored from `track_part_1a.inc`.
pub const TRACK_TYPE_BUS: u32 = 3;

/// Allocates the next free track id; 0 when none is available.
pub type AllocateTrackIdFn = unsafe extern "C" fn(*mut c_void) -> u32;
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
/// Deep-clones a track, appends the copy, writes its id and index, and returns
/// a handle. Returns null when the source track is missing.
pub type DuplicateTrackFn =
    unsafe extern "C" fn(*mut c_void, u32, *mut u32, *mut usize) -> *mut c_void;
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
    pub allocate_id: Option<AllocateTrackIdFn>,
    pub create: Option<CreateTrackFn>,
    pub duplicate: Option<DuplicateTrackFn>,
    pub take: Option<TakeTrackFn>,
    pub remove: Option<RemoveTrackFn>,
    pub restore: Option<RestoreTrackFn>,
    pub release_handle: Option<ReleaseTrackHandleFn>,
    pub record_replay: Option<RecordTrackReplayFn>,
    pub sync_structural_changes: Option<SyncStructuralChangesFn>,
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

/// Hands one lifecycle edit to the history. A missing recorder drops the entry
/// (freeing its handle) instead of leaking it.
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
        None => unsafe { release_track_edit(edit.cast()) },
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
    let (Some(allocate), Some(create)) = (ops.allocate_id, ops.create) else {
        return 0;
    };
    let id = unsafe { allocate(host) };
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
    let mut new_id = 0_u32;
    let mut index = 0_usize;
    let handle = unsafe { duplicate(host, track_id, &mut new_id, &mut index) };
    if handle.is_null() || new_id == 0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestState {
        calls: Vec<String>,
        undo: Option<(ReplayTrackEditFn, usize)>,
        redo: Option<(ReplayTrackEditFn, usize)>,
        release: Option<(ReplayTrackEditFn, usize)>,
    }

    static SERIAL: Mutex<()> = Mutex::new(());
    static STATE: Mutex<TestState> = Mutex::new(TestState {
        calls: Vec::new(),
        undo: None,
        redo: None,
        release: None,
    });

    fn reset() {
        let mut state = STATE.lock().unwrap();
        state.calls.clear();
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

    unsafe extern "C" fn fake_allocate(_host: *mut c_void) -> u32 {
        7
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
        out_id: *mut u32,
        out_index: *mut usize,
    ) -> *mut c_void {
        call(&format!("duplicate {track_id}"));
        unsafe {
            *out_id = 9;
            *out_index = 2;
        }
        0xCAFE as *mut c_void
    }

    unsafe extern "C" fn fake_take(_host: *mut c_void, track_id: u32) -> *mut c_void {
        call(&format!("take {track_id}"));
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
            allocate_id: Some(fake_allocate),
            create: Some(fake_create),
            duplicate: Some(fake_duplicate),
            take: Some(fake_take),
            remove: Some(fake_remove),
            restore: Some(fake_restore),
            release_handle: Some(fake_release),
            record_replay: Some(fake_record),
            sync_structural_changes: Some(fake_sync),
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

    #[test]
    fn add_names_tracks_and_replays_undo_and_redo() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = fake_ops();
        let host = std::ptr::null_mut();

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

        assert_eq!(add_track_impl(&ops, std::ptr::null_mut(), TRACK_TYPE_BUS), 7);
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
        let host = std::ptr::null_mut();

        assert_eq!(duplicate_track_impl(&ops, host, 4), 9);
        assert_eq!(drain(), vec!["duplicate 4", "record Duplicate Track", "sync"]);

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
        let host = std::ptr::null_mut();

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
    fn a_missing_track_records_nothing() {
        let _guard = SERIAL.lock().unwrap();
        reset();
        let ops = HirariTrackEditHostOps {
            take: Some(fake_take_missing),
            record_replay: Some(fake_record),
            sync_structural_changes: Some(fake_sync),
            ..Default::default()
        };
        assert!(!remove_track_impl(&ops, std::ptr::null_mut(), 4));
        assert_eq!(drain(), vec!["take 4"]);
    }

    unsafe extern "C" fn fake_take_missing(
        _host: *mut c_void,
        track_id: u32,
    ) -> *mut c_void {
        call(&format!("take {track_id}"));
        std::ptr::null_mut()
    }
}
