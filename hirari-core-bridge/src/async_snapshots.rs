//! Rust-owned request state for asynchronous UI snapshots.
//!
//! The native engine still gathers its own live track data through callbacks;
//! request identity, cancellation, worker lifetime, and result publication are
//! coordinated here so they cannot race with polling or engine-handle release.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;

type ComputeLayout = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> bool;
type ReadLayout = unsafe extern "C" fn(
    *mut c_void, *mut u64, *mut u64, *mut *const u8, *mut usize,
) -> bool;
type DestroyObject = unsafe extern "C" fn(*mut c_void);
type SetCancelled = unsafe extern "C" fn(*mut c_void);
type IsCancelled = unsafe extern "C" fn(*const c_void) -> bool;
type VisitBytes = unsafe extern "C" fn(*mut c_void, *const u8, usize);
type ComputeWaveform = unsafe extern "C" fn(
    *mut c_void, u32, u32, *mut f32, usize,
) -> usize;
type VisitWaveform = unsafe extern "C" fn(*mut c_void, *const f32, usize);

struct ActiveLayout {
    context: usize,
    cancel: SetCancelled,
    owner: u64,
}

#[derive(Default)]
struct LayoutState {
    active: HashMap<u64, ActiveLayout>,
    results: HashMap<u64, (u64, String)>,
    released_owners: HashSet<u64>,
}

#[derive(Default)]
struct WaveformState {
    pending: HashSet<u64>,
    results: HashMap<u64, Vec<f32>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static LAYOUTS: OnceLock<Mutex<LayoutState>> = OnceLock::new();
static WAVEFORMS: OnceLock<Mutex<WaveformState>> = OnceLock::new();

fn layouts() -> &'static Mutex<LayoutState> {
    LAYOUTS.get_or_init(|| Mutex::new(LayoutState::default()))
}

fn waveforms() -> &'static Mutex<WaveformState> {
    WAVEFORMS.get_or_init(|| Mutex::new(WaveformState::default()))
}

fn next_id() -> u64 {
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

struct ContextGuard {
    context: usize,
    destroy: DestroyObject,
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.context as *mut c_void) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot_request(
    owner: u64,
    context: *mut c_void,
    compute: Option<ComputeLayout>,
    read: Option<ReadLayout>,
    destroy_result: Option<DestroyObject>,
    destroy_context: Option<DestroyObject>,
    set_cancelled: Option<SetCancelled>,
    is_cancelled: Option<IsCancelled>,
) -> u64 {
    let Some(destroy_context) = destroy_context else { return 0 };
    if context.is_null() {
        return 0;
    }
    let (Some(compute), Some(read), Some(destroy_result), Some(set_cancelled), Some(is_cancelled)) =
        (compute, read, destroy_result, set_cancelled, is_cancelled)
    else {
        unsafe { destroy_context(context) };
        return 0;
    };
    if owner == 0 {
        unsafe { destroy_context(context) };
        return 0;
    }
    let id = next_id();
    let context_id = context as usize;
    {
        let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
        if state.released_owners.contains(&owner)
            || state.active.values().any(|active| active.owner == owner)
            || state.results.values().any(|(result_owner, _)| *result_owner == owner)
        {
            unsafe { destroy_context(context) };
            return 0;
        }
        state.active.insert(id, ActiveLayout { context: context_id, cancel: set_cancelled, owner });
    }
    let worker = thread::Builder::new().name("hirari-layout-snapshot".into()).spawn(move || {
        let _context = ContextGuard { context: context_id, destroy: destroy_context };
        let mut native_result = std::ptr::null_mut();
        let mut serialized = None;
        if unsafe { compute(context_id as *mut c_void, &mut native_result) }
            && !native_result.is_null()
        {
            let mut before = 0;
            let mut after = 0;
            let mut data = std::ptr::null();
            let mut length = 0;
            if unsafe { read(native_result, &mut before, &mut after, &mut data, &mut length) }
                && (length == 0 || !data.is_null())
            {
                let bytes = if length == 0 { &[] } else {
                    unsafe { std::slice::from_raw_parts(data, length) }
                };
                serialized = Some((before, after, String::from_utf8_lossy(bytes).into_owned()));
            }
        }
        if !native_result.is_null() {
            unsafe { destroy_result(native_result) };
        }
        let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
        let active = state.active.remove(&id);
        let cancelled = unsafe { is_cancelled(context_id as *const c_void) };
        let result = if cancelled {
            "CANCELLED".to_owned()
        } else if serialized.is_none() {
            "STALE".to_owned()
        } else {
            let (before, after, layout) = serialized.unwrap();
            if before == after && !layout.is_empty() {
                format!("READY {after}\n{layout}")
            } else {
                "STALE".to_owned()
            }
        };
        if let Some(active) = active {
            if !state.released_owners.remove(&active.owner) {
                state.results.insert(id, (active.owner, result));
            }
        }
    });
    if worker.is_err() {
        let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(active) = state.active.remove(&id) {
            unsafe { destroy_context(active.context as *mut c_void) };
        }
        return 0;
    }
    id
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot_poll(
    id: u64,
    owner: u64,
    context: *mut c_void,
    visit: Option<VisitBytes>,
) -> u8 {
    let Some(visit) = visit else { return 2 };
    let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((result_owner, _)) = state.results.get(&id) {
        if *result_owner == owner {
            let result = state.results.remove(&id).unwrap().1;
            unsafe { visit(context, result.as_ptr(), result.len()) };
            return 1;
        }
    }
    if state.active.get(&id).is_some_and(|active| active.owner == owner) { 0 } else { 2 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot_cancel(id: u64, owner: u64) -> bool {
    let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(active) = state.active.get(&id).filter(|active| active.owner == owner) {
        unsafe { (active.cancel)(active.context as *mut c_void) };
        return true;
    }
    state.results.get(&id).is_some_and(|(result_owner, _)| *result_owner == owner)
        && state.results.remove(&id).is_some()
}

#[no_mangle]
pub extern "C" fn hirari_layout_snapshot_owner_create() -> u64 {
    next_id()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_layout_snapshot_owner_release(owner: u64) {
    if owner == 0 { return; }
    let mut state = layouts().lock().unwrap_or_else(|e| e.into_inner());
    state.results.retain(|_, (result_owner, _)| *result_owner != owner);
    let active = state.active.values().find(|active| active.owner == owner)
        .map(|active| (active.context, active.cancel));
    if let Some((context, cancel)) = active {
        state.released_owners.insert(owner);
        unsafe { cancel(context as *mut c_void) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_snapshot_request(
    context: *mut c_void,
    track_id: u32,
    region_id: u32,
    compute: Option<ComputeWaveform>,
    destroy_context: Option<DestroyObject>,
) -> u64 {
    let (Some(compute), Some(destroy_context)) = (compute, destroy_context) else { return 0 };
    if context.is_null() || track_id == 0 || region_id == 0 {
        if !context.is_null() { unsafe { destroy_context(context) } };
        return 0;
    }
    let id = next_id();
    let context_id = context as usize;
    waveforms().lock().unwrap_or_else(|e| e.into_inner()).pending.insert(id);
    let worker = thread::Builder::new().name("hirari-waveform-snapshot".into()).spawn(move || {
        let _context = ContextGuard { context: context_id, destroy: destroy_context };
        let mut samples = vec![0.0f32; 128];
        let count = unsafe {
            compute(context_id as *mut c_void, track_id, region_id, samples.as_mut_ptr(), samples.len())
        }.min(samples.len());
        samples.truncate(count);
        let mut state = waveforms().lock().unwrap_or_else(|e| e.into_inner());
        state.pending.remove(&id);
        state.results.insert(id, samples);
    });
    if worker.is_err() {
        let mut state = waveforms().lock().unwrap_or_else(|e| e.into_inner());
        state.pending.remove(&id);
        unsafe { destroy_context(context_id as *mut c_void) };
        return 0;
    }
    id
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_snapshot_poll(
    id: u64,
    context: *mut c_void,
    visit: Option<VisitWaveform>,
) -> bool {
    let Some(visit) = visit else { return false };
    let mut state = waveforms().lock().unwrap_or_else(|e| e.into_inner());
    let Some(samples) = state.results.remove(&id) else { return false };
    unsafe { visit(context, samples.as_ptr(), samples.len()) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_snapshot_pending(id: u64) -> bool {
    waveforms().lock().unwrap_or_else(|e| e.into_inner()).pending.contains(&id)
}
