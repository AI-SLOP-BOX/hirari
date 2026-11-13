//! Layout snapshot session API: revision tokens, the JSON exporter, the
//! background request/poll/cancel path and the save-from-host entry,
//! split out of layout_gather.rs.
use super::*;

/// The layout change token of the engine behind this host address.
pub fn project_layout_revision(host: *const c_void) -> u64 {
    unsafe { hirari_project_layout_revision(host) }
}

/// The combined native change token of the engine behind this host address.
pub fn project_state_revision(host: *const c_void) -> u64 {
    unsafe { hirari_project_state_revision(host) }
}

/// One background layout snapshot: the engine host the worker reads through
/// its session bag, and the flag the UI raises when it loses interest.
pub(crate) struct LayoutSession {
    engine: *mut c_void,
    cancelled: std::sync::atomic::AtomicBool,
}

// Only the snapshot worker dereferences the engine, and the owning
// `AudioEngine` joins that worker before its graph can be destroyed.
unsafe impl Send for LayoutSession {}

pub(crate) struct RecordedLayout {
    before: u64,
    after: u64,
    layout: Vec<u8>,
}

extern "C" fn session_cancelled(context: *const c_void) -> bool {
    let session = context.cast::<LayoutSession>();
    !session.is_null() && unsafe { (*session).cancelled.load(std::sync::atomic::Ordering::Acquire) }
}

extern "C" fn session_set_cancelled(context: *mut c_void) {
    let session = context.cast::<LayoutSession>();
    if !session.is_null() {
        unsafe { (*session).cancelled.store(true, std::sync::atomic::Ordering::Release) };
    }
}

/// Gathers and serializes the layout, bracketing it with the project state
/// revision so the poller can tell a snapshot that aged out from a usable one.
extern "C" fn session_compute(context: *mut c_void, output: *mut *mut c_void) -> bool {
    let (Some(session), Some(output)) = (unsafe {
        context.cast::<LayoutSession>().as_mut()
    }, unsafe { output.as_mut() })
    else {
        return false;
    };
    if session.cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let before = unsafe { hirari_project_state_revision(session.engine) };
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0;
    if !unsafe {
        hirari_layout_json_from_host(
            session.engine,
            Some(session_cancelled),
            (session as *mut LayoutSession).cast::<c_void>(),
            &mut data,
            &mut length,
        )
    } {
        return false;
    }
    let layout = if data.is_null() || length == 0 {
        Vec::new()
    } else {
        unsafe { Vec::from_raw_parts(data, length, length) }
    };
    let after = unsafe { hirari_project_state_revision(session.engine) };
    *output = Box::into_raw(Box::new(RecordedLayout {
        before,
        after,
        layout,
    }))
    .cast();
    true
}

extern "C" fn session_read(
    result: *mut c_void,
    before: *mut u64,
    after: *mut u64,
    data: *mut *const u8,
    length: *mut usize,
) -> bool {
    let Some(recorded) = (unsafe { result.cast::<RecordedLayout>().as_ref() }) else {
        return false;
    };
    if before.is_null() || after.is_null() || data.is_null() || length.is_null() {
        return false;
    }
    unsafe {
        *before = recorded.before;
        *after = recorded.after;
        *data = recorded.layout.as_ptr();
        *length = recorded.layout.len();
    }
    true
}

extern "C" fn session_destroy_result(result: *mut c_void) {
    if !result.is_null() {
        drop(unsafe { Box::from_raw(result.cast::<RecordedLayout>()) });
    }
}

extern "C" fn session_destroy_context(context: *mut c_void) {
    if !context.is_null() {
        drop(unsafe { Box::from_raw(context.cast::<LayoutSession>()) });
    }
}

extern "C" fn visit_snapshot_bytes(context: *mut c_void, bytes: *const u8, length: usize) {
    let Some(output) = (unsafe { context.cast::<Vec<u8>>().as_mut() }) else {
        return;
    };
    if bytes.is_null() && length != 0 {
        return;
    }
    output.clear();
    if length != 0 {
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
    }
}

/// Snapshot owners are keyed by engine address, so a session belongs to exactly
/// one engine and a reused address starts a clean owner.
pub(crate) static LAYOUT_OWNERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, u64>>,
> = std::sync::OnceLock::new();

pub(crate) fn layout_owners() -> &'static std::sync::Mutex<std::collections::HashMap<usize, u64>> {
    LAYOUT_OWNERS.get_or_init(std::sync::Mutex::default)
}

pub(crate) fn owner_for(host: usize) -> u64 {
    let mut owners = layout_owners().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(owner) = owners.get(&host) {
        return *owner;
    }
    let owner = crate::async_snapshots::hirari_layout_snapshot_owner_create();
    owners.insert(host, owner);
    owner
}

pub(crate) fn existing_owner(host: usize) -> Option<u64> {
    layout_owners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&host)
        .copied()
}

/// The live project layout as JSON. The gather is Rust and reads the engine's
/// own stores through its session bag; the host contributes only its address.
pub fn project_layout_json(core: *const crate::HirariUnifiedEngine) -> String {
    if core.is_null() {
        return String::new();
    }
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0;
    if !unsafe {
        hirari_layout_json_from_host(
            core.cast_mut().cast::<c_void>(),
            None,
            std::ptr::null(),
            &mut data,
            &mut length,
        )
    } || data.is_null()
        || length == 0
    {
        return String::new();
    }
    let bytes = unsafe { Vec::from_raw_parts(data, length, length) };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Starts the background save's layout snapshot and returns its request id.
pub fn request_layout_snapshot(engine: *mut crate::HirariUnifiedEngine) -> u64 {
    let owner = owner_for(engine as usize);
    let session = Box::into_raw(Box::new(LayoutSession {
        engine: engine.cast(),
        cancelled: std::sync::atomic::AtomicBool::new(false),
    }))
    .cast::<c_void>();
    unsafe {
        crate::async_snapshots::hirari_layout_snapshot_request(
            owner,
            session,
            Some(session_compute),
            Some(session_read),
            Some(session_destroy_result),
            Some(session_destroy_context),
            Some(session_set_cancelled),
            Some(session_cancelled),
        )
    }
}

/// Empty while the snapshot is still being prepared, otherwise
/// `READY <revision>\n<layout-json>` or a failure marker.
pub fn poll_layout_snapshot(
    engine: *const crate::HirariUnifiedEngine,
    request_id: u64,
) -> String {
    if request_id == 0 {
        return "ERROR".to_owned();
    }
    let owner = existing_owner(engine as usize).unwrap_or(0);
    let mut bytes = Vec::new();
    let status = unsafe {
        crate::async_snapshots::hirari_layout_snapshot_poll(
            request_id,
            owner,
            (&mut bytes as *mut Vec<u8>).cast::<c_void>(),
            Some(visit_snapshot_bytes),
        )
    };
    match status {
        0 => String::new(),
        1 => String::from_utf8_lossy(&bytes).into_owned(),
        _ => "ERROR".to_owned(),
    }
}

pub fn cancel_layout_snapshot(
    engine: *const crate::HirariUnifiedEngine,
    request_id: u64,
) -> bool {
    if request_id == 0 {
        return false;
    }
    let Some(owner) = existing_owner(engine as usize) else {
        return false;
    };
    unsafe { crate::async_snapshots::hirari_layout_snapshot_cancel(request_id, owner) }
}

/// The saver path: gathers with the save-only fields and encodes the project
/// container directly, without a C++ snapshot in between. The tempo map, the
/// routing graph, the arrangement document, its bpm and the project tonality all
/// come from the host's own session bag, so the caller names nothing but the
/// path it wants written.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_save_from_host(
    host: *mut c_void,
    path: *const u8,
    path_len: usize,
) -> bool {
    if host.is_null() {
        return false;
    }
    let tempo = host_store(host, crate::engine_session::H_TEMPO);
    let routing = host_store(host, crate::engine_session::H_ROUTING);
    let arrangement = host_store(host, crate::engine_session::H_ARRANGEMENT_DOCUMENT);
    let block = host_store(host, crate::engine_session::H_ENGINE_BLOCK);
    let bpm = unsafe { crate::tempo::hirari_tempo_map_get_current_bpm(tempo) } as f64;
    let root_note = unsafe { crate::engine_block_host::hirari_engine_block_root_note(block) };
    let scale_type = unsafe { crate::engine_block_host::hirari_engine_block_scale_type(block) };
    let Some(owned) = (unsafe { gather(host, &NATIVE_OPS, true, None) }) else {
        return false;
    };
    let snapshot = owned.snapshot();
    let saved = unsafe {
        crate::project_save_layout::hirari_project_save_from_layout(
            path,
            path_len,
            &snapshot,
            tempo,
            routing,
            arrangement as usize,
            bpm,
            root_note,
            scale_type,
        )
    };
    drop(owned);
    saved
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_release(storage: *mut c_void) {
    if !storage.is_null() {
        drop(unsafe { Box::from_raw(storage.cast::<LayoutOwned>()) });
    }
}
