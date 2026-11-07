//! Sandbox recovery orchestration.
//!
//! The maintenance pass itself lives in `plugin_sandbox_maintenance`; this
//! module owns the thread that drives it. Moving the loop here deleted the
//! engine's C++ recovery thread together with its mutex, condition variable
//! and journal-start sequence: the engine now hands over its host pointer
//! once and keeps no synchronisation state for the loop.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::forensic_journaler::hirari_forensic_journaler_start;
use crate::log_buffer::{hirari_log_buffer_default_state, hirari_log_buffer_post};
use crate::plugin_sandbox_maintenance::hirari_plugin_sandbox_maintain;

/// Poll period of the recovery loop, matching the C++ condition-variable wait.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const JOURNAL_PATH: &[u8] = b"hirari_session_sovereign.journal";

struct RecoveryLoop {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

fn slot() -> &'static Mutex<Option<RecoveryLoop>> {
    static SLOT: OnceLock<Mutex<Option<RecoveryLoop>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn log(level: u32, component: u32, message: &str) {
    let state = hirari_log_buffer_default_state();
    if state.is_null() {
        return;
    }
    // SAFETY: the default ring outlives any engine that logs through it, and
    // the message borrow is only read during this call.
    unsafe {
        hirari_log_buffer_post(state, level, component, message.as_ptr(), message.len());
    }
}

/// Starts journaling and the periodic sandbox maintenance loop for `host`.
///
/// Idempotent: a second call while the loop is running is ignored. The
/// `HIRARI_DISABLE_RECOVERY_THREAD` opt-out is honoured before anything is
/// started, exactly as the removed C++ orchestrator did.
///
/// # Safety
/// `host` must stay valid until `hirari_sandbox_recovery_stop` returns.
#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_recovery_start(host: *mut c_void, journal: *mut c_void) {
    if host.is_null() {
        return;
    }
    let Ok(mut tail) = slot().lock() else {
        return;
    };
    if tail.is_some() {
        return;
    }
    if std::env::var_os("HIRARI_DISABLE_RECOVERY_THREAD")
        .is_some_and(|value| value.to_str() == Some("1"))
    {
        return;
    }
    if !journal.is_null() {
        // SAFETY: the caller owns the journaler handle and destroys it only
        // after this loop has been stopped and joined.
        unsafe {
            hirari_forensic_journaler_start(journal, JOURNAL_PATH.as_ptr(), JOURNAL_PATH.len());
        }
    }
    log(0, 0, "SYSTEM | RECOVERY | JOURNAL_STARTED");

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let host = host as usize;
    let spawned = thread::Builder::new()
        .name("hirari-sandbox-recovery".into())
        .spawn(move || {
            let host = host as *mut c_void;
            while !thread_stop.load(Ordering::Acquire) {
                // SAFETY: the engine keeps `host` alive until stop() joined
                // this thread, which is the same contract the C++ thread had.
                let recovered = unsafe { hirari_plugin_sandbox_maintain(host, true) };
                if recovered > 0 {
                    log(0, recovered, "SYSTEM | PLUGIN_SANDBOX | PROCESS_RECOVERED");
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
    if let Ok(thread) = spawned {
        *tail = Some(RecoveryLoop { stop, thread });
    }
}

/// Stops and joins the recovery loop. Safe to call when nothing is running.
#[no_mangle]
pub extern "C" fn hirari_sandbox_recovery_stop() {
    let Ok(mut tail) = slot().lock() else {
        return;
    };
    let Some(running) = tail.take() else {
        return;
    };
    running.stop.store(true, Ordering::Release);
    let _ = running.thread.join();
}
