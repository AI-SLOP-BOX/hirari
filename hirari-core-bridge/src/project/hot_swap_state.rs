//! Rust-owned synchronization state for the native hot-swap watchdog.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const IDLE: u8 = 0;
const WAITING_FOR_QUIESCE: u8 = 1;
const EXECUTION: u8 = 2;
const ABORTING: u8 = 3;
const RESERVED: u8 = 4;
const MAX_QUIESCE_BLOCKS: u32 = 256;
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
pub extern "C" fn hirari_hot_swap_state_create(
    context: *mut c_void,
    log: Option<LogCallback>,
    cpu_load: Option<CpuLoadCallback>,
) -> *mut c_void {
    let (Some(log), Some(cpu_load)) = (log, cpu_load) else { return std::ptr::null_mut() };
    Box::into_raw(Box::new(HotSwapState::new(context, Some(log), Some(cpu_load)))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { hirari_hot_swap_state_stop(state) };
        drop(unsafe { Box::from_raw(state.cast::<HotSwapState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_is_idle(state: *const c_void) -> bool {
    !state.is_null()
        && unsafe { &*state.cast::<HotSwapState>() }.phase.load(Ordering::Acquire) == IDLE
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_begin(
    state: *mut c_void,
    target: usize,
    context: *mut c_void,
    invoke: Option<NativeCallback>,
    destroy: Option<NativeCallback>,
) -> bool {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return false };
    let (Some(invoke), Some(destroy)) = (invoke, destroy) else { return false };
    if state.phase.compare_exchange(IDLE, RESERVED, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return false;
    }
    state.acknowledged.store(false, Ordering::Release);
    state.quiesce_blocks.store(0, Ordering::Release);
    *state.pending.lock().unwrap_or_else(|error| error.into_inner()) = Some(PendingFallback {
        target, context, invoke, destroy,
    });
    *state.deadline.lock().unwrap_or_else(|error| error.into_inner()) =
        Some(Instant::now() + Duration::from_millis(5));
    state.quiesce_requested.store(true, Ordering::Release);
    state.phase.store(WAITING_FOR_QUIESCE, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_start(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return };
    let mut daemon = state.daemon.lock().unwrap_or_else(|error| error.into_inner());
    if state.daemon_running.swap(true, Ordering::AcqRel) { return; }
    let state_ptr = state as *const HotSwapState as usize;
    let thread = thread::spawn(move || {
        let state = unsafe { &*(state_ptr as *const HotSwapState) };
        let mut audit_counter = 0_u32;
        while state.daemon_running.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
            if !state.daemon_running.load(Ordering::Acquire) { break; }
            let action = unsafe { hirari_hot_swap_state_poll(state as *const _ as *mut c_void) };
            if action != 0 { finish_hot_swap(state, action); }
            audit_counter += 1;
            if audit_counter >= 100 {
                audit_counter = 0;
                if let Some(cpu_load) = state.cpu_load {
                    let load = unsafe { cpu_load(state.callback_context as *mut c_void) };
                    if load > 98.0 {
                        log(state, 1, 0xFF, b"WATCHDOG | CPU_OVERLOAD | REBALANCE_TRIGGERED");
                    }
                }
            }
        }
    });
    *daemon = Some(thread);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_stop(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return };
    state.daemon_running.store(false, Ordering::Release);
    if let Some(thread) = state.daemon.lock().unwrap_or_else(|error| error.into_inner()).take() {
        let _ = thread.join();
    }
}

fn log(state: &HotSwapState, level: u32, code: u32, message: &[u8]) {
    if let Some(log) = state.log {
        unsafe { log(state.callback_context as *mut c_void, level, code,
            message.as_ptr(), message.len()) };
    }
}

fn finish_hot_swap(state: &HotSwapState, action: u8) {
    let pending = state.pending.lock().unwrap_or_else(|error| error.into_inner()).take();
    if let Some(pending) = pending {
        if action == 1 {
            unsafe { (pending.invoke)(pending.context) };
            log(state, 0, pending.target as u32, b"IMMORTALITY | HOT_SWAP_SUCCESS");
        } else {
            unsafe { (pending.destroy)(pending.context) };
            log(state, 2, 0xDEAD, b"IMMORTALITY | HOT_SWAP_ABORTED | QUIESCE_TIMEOUT");
        }
    }
    unsafe { hirari_hot_swap_state_finish(state as *const _ as *mut c_void) };
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
pub unsafe extern "C" fn hirari_hot_swap_state_acknowledge(state: *mut c_void) {
    let Some(state) = (unsafe { state.cast::<HotSwapState>().as_ref() }) else { return };
    state.acknowledged.store(true, Ordering::Release);
    state.quiesce_blocks.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_quiesce_requested(state: *const c_void) -> bool {
    !state.is_null() && unsafe { &*state.cast::<HotSwapState>() }
        .quiesce_requested.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_quiesce_blocks(state: *const c_void) -> u32 {
    if state.is_null() { return 0; }
    unsafe { &*state.cast::<HotSwapState>() }.quiesce_blocks.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_hot_swap_state_exceeded_budget(state: *const c_void) -> bool {
    unsafe { hirari_hot_swap_state_quiesce_blocks(state) > MAX_QUIESCE_BLOCKS }
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
