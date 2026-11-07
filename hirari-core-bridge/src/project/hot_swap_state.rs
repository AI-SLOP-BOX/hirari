//! Rust-owned synchronization state for the native hot-swap watchdog.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

const IDLE: u8 = 0;
const WAITING_FOR_QUIESCE: u8 = 1;
const EXECUTION: u8 = 2;
const ABORTING: u8 = 3;
type NativeCallback = unsafe extern "C" fn(*mut c_void);
type LogCallback = unsafe extern "C" fn(*mut c_void, u32, u32, *const u8, usize);
type CpuLoadCallback = unsafe extern "C" fn(*mut c_void) -> f32;

struct PendingFallback {
    target: usize,
    context: *mut c_void,
    invoke: NativeCallback,
    destroy: NativeCallback,
}

unsafe impl Send for PendingFallback {}

struct HotSwapState {
    phase: AtomicU8,
    quiesce_requested: AtomicBool,
    acknowledged: AtomicBool,
    quiesce_blocks: AtomicU32,
    deadline: Mutex<Option<Instant>>,
    pending: Mutex<Option<PendingFallback>>,
    daemon_running: AtomicBool,
    daemon: Mutex<Option<JoinHandle<()>>>,
    callback_context: usize,
    log: Option<LogCallback>,
    cpu_load: Option<CpuLoadCallback>,
}

unsafe impl Send for HotSwapState {}
unsafe impl Sync for HotSwapState {}

impl HotSwapState {
    fn new(
        callback_context: *mut c_void,
        log: Option<LogCallback>,
        cpu_load: Option<CpuLoadCallback>,
    ) -> Self {
        Self {
            phase: AtomicU8::new(IDLE),
            quiesce_requested: AtomicBool::new(false),
            acknowledged: AtomicBool::new(false),
            quiesce_blocks: AtomicU32::new(0),
            deadline: Mutex::new(None),
            pending: Mutex::new(None),
            daemon_running: AtomicBool::new(false),
            daemon: Mutex::new(None),
            callback_context: callback_context as usize,
            log,
            cpu_load,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_stop(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return };
    state.daemon_running.store(false, Ordering::Release);
    if let Some(thread) = state.daemon.lock().unwrap_or_else(|error| error.into_inner()).take() {
        let _ = thread.join();
    }
}

/// Returns 1 when the fallback may execute, 2 when the quiesce deadline has
/// expired, and 0 while the request is still waiting.
#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_poll(state: *mut c_void) -> u8 {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return 0 };
    if state.phase.load(Ordering::Acquire) != WAITING_FOR_QUIESCE { return 0; }
    if state.acknowledged.load(Ordering::Acquire) {
        if state.phase.compare_exchange(
            WAITING_FOR_QUIESCE, EXECUTION, Ordering::AcqRel, Ordering::Acquire,
        ).is_ok() { return 1; }
        return 0;
    }
    let expired = state.deadline.lock().unwrap_or_else(|error| error.into_inner())
        .is_some_and(|deadline| Instant::now() > deadline);
    if expired && state.phase.compare_exchange(
        WAITING_FOR_QUIESCE, ABORTING, Ordering::AcqRel, Ordering::Acquire,
    ).is_ok() { 2 } else { 0 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_finish(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return };
    state.quiesce_requested.store(false, Ordering::Release);
    state.quiesce_blocks.store(0, Ordering::Release);
    *state.deadline.lock().unwrap_or_else(|error| error.into_inner()) = None;
    state.phase.store(IDLE, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_quiesce_blocks(state: *const c_void) -> u32 {
    if state.is_null() { return 0; }
    unsafe { &*state.cast::<HotSwapState>() }.quiesce_blocks.load(Ordering::Acquire)
}

static DEFAULT_STATE: OnceLock<Box<HotSwapState>> = OnceLock::new();

fn default_state() -> &'static HotSwapState {
    DEFAULT_STATE.get_or_init(|| Box::new(HotSwapState::new(std::ptr::null_mut(), None, None)))
}

/// Quiesce state shared by the engine when no hot-swap host is installed.
#[no_mangle]
pub extern "C" fn hirari_hot_swap_default_quiesce_requested() -> bool {
    default_state().quiesce_requested.load(Ordering::Acquire)
}

#[no_mangle]
pub extern "C" fn hirari_hot_swap_default_acknowledge() {
    let state = default_state();
    state.acknowledged.store(true, Ordering::Release);
    state.quiesce_blocks.fetch_add(1, Ordering::Relaxed);
}
