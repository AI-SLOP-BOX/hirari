//! Offline bounce render driver and the bounce session state machine.
//!
//! A bounce is a control-plane transaction, so all of its state lives here:
//! offline-render ownership, the async worker lifecycle, progress and state
//! publication, the cancel/pause control plane and the publish boundary — and
//! so does its plan, gathered from the Rust stores the graph already owns. C++
//! keeps only the audio graph, so [`HirariBounceHostOps`] is the short list of
//! operations that genuinely need the native track list.

use crate::bounce_plan::{
    hirari_bounce_output_path_supported, plan_extent, BOUNCE_PLAN_INVALID_RANGE,
    BOUNCE_STATE_CANCELLED, BOUNCE_STATE_COMPLETED, BOUNCE_STATE_FAILED, BOUNCE_STATE_PAUSED,
    BOUNCE_STATE_QUEUED, BOUNCE_STATE_RENDERING, BouncePlanInput, HirariBouncePlan, RegionSpan,
};
use crate::bounce_plan::{
    hirari_bounce_state_cancellable, hirari_bounce_state_pausable, hirari_bounce_state_resumable,
    hirari_bounce_state_terminal,
};
use crate::engine_block_host::hirari_engine_block_device_sample_rate;
use crate::engine_track_table::{hirari_engine_track_count, hirari_engine_track_ptrs};
use crate::export::{
    hirari_bounce_stream_create, hirari_bounce_stream_finish, hirari_bounce_stream_free,
    hirari_bounce_stream_write,
};
use crate::log_buffer::{hirari_log_buffer_default_state, hirari_log_buffer_post};
use crate::master_limiter::hirari_master_limiter_tail;
use crate::midi_note_edit::{
    hirari_midi_notes_snapshot_take, hirari_midi_notes_snapshot_take_free,
    hirari_midi_scheduled_note_end, ScheduledMidiNoteFfi,
};
use crate::region_store::{hirari_region_store_count, hirari_region_store_span_at};
use crate::track_session;
use crate::tpdf_dither::{
    hirari_tpdf_dither_create_with_seed, hirari_tpdf_dither_destroy, hirari_tpdf_dither_next,
};
use std::cell::Cell;
use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Left/right noise seeds stay fixed so two bounces of the same project are
/// byte-identical.
const TPDF_DITHER_LEFT_SEED: u32 = 0x1234_5678;
const TPDF_DITHER_RIGHT_SEED: u32 = 0x9E37_79B9;
/// Largest render block; the host lowers this to the prepared block size.
const MAX_RENDER_CHUNK: u32 = 1024;
const CONTROL_CANCEL: u8 = 1;
const CONTROL_PAUSE: u8 = 2;
/// Pause polling interval while the control plane holds the render.
const PAUSE_POLL: Duration = Duration::from_millis(10);
/// Idle lifecycle state: no render has ever been attempted by this session.
const BOUNCE_STATE_IDLE: u32 = 0;
/// `begin` outcomes: ownership acquired, cancelled while waiting, or the
/// realtime graph never released it.
const OWNERSHIP_ACQUIRED: u8 = 0;
const OWNERSHIP_CANCELLED: u8 = 1;
const OWNERSHIP_BUSY: u8 = 2;
/// A bounded failure is recoverable; an immortal render is not.
const OWNERSHIP_DEADLINE: Duration = Duration::from_secs(5);

fn log(level: u32, message: &[u8]) {
    // SAFETY: the default diagnostic ring is process-lifetime storage.
    unsafe {
        hirari_log_buffer_post(
            hirari_log_buffer_default_state(),
            level,
            0,
            message.as_ptr(),
            message.len(),
        );
    }
}

/// A callback-safe thread identity. The realtime callback and an offline
/// render worker share this token space so ownership comparisons are exact.
static NEXT_THREAD_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static CALLBACK_THREAD_TOKEN: Cell<u64> = const { Cell::new(0) };
    /// Marks the thread that is deliberately driving an offline bounce.
    static OFFLINE_RENDER_THREAD: Cell<bool> = const { Cell::new(false) };
    /// The flag value the owning render has to restore on exit.
    static OFFLINE_RENDER_THREAD_PREV: Cell<bool> = const { Cell::new(false) };
}

fn offline_thread() -> bool {
    OFFLINE_RENDER_THREAD.with(Cell::get)
}

/// Declares the calling thread the offline renderer, remembering the value its
/// driver frame restores on exit.
fn mark_offline_thread() {
    OFFLINE_RENDER_THREAD_PREV.set(offline_thread());
    OFFLINE_RENDER_THREAD.set(true);
}

fn unmark_offline_thread() {
    OFFLINE_RENDER_THREAD.set(OFFLINE_RENDER_THREAD_PREV.with(Cell::get));
}

#[no_mangle]
pub extern "C" fn hirari_callback_thread_token() -> u64 {
    CALLBACK_THREAD_TOKEN.with(|token| {
        let mut id = token.get();
        if id == 0 {
            id = NEXT_THREAD_TOKEN.fetch_add(1, Ordering::Relaxed);
            token.set(id);
        }
        id
    })
}

/// The engine stores the Rust plan walk visits, as `store` numbers them.
pub const STORE_TRACK_TABLE: u32 = 0;
pub const STORE_ENGINE_BLOCK: u32 = 1;
pub const STORE_MIDI_SNAPSHOT: u32 = 2;
pub const STORE_MASTER_LIMITER: u32 = 3;

/// One of the engine's stores by `STORE_*` kind.
pub type BounceStoreFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;

/// Engine operations the driver still needs from C++. Every entry is
/// mandatory; a missing operation fails the render instead of guessing.
#[repr(C)]
pub struct HirariBounceHostOps {
    /// Runs one complete synchronous bounce, as a worker thread's body.
    pub run: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, u32) -> u8>,
    /// Resets the master limiter and every track for a reproducible render.
    pub reset: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Resolves the Rust stores the plan walk reads — the track table, the
    /// engine block, the MIDI snapshot and the master limiter.
    pub store: Option<BounceStoreFn>,
    pub render: Option<unsafe extern "C" fn(*mut c_void, *mut f32, *mut f32, u32)>,
    pub render_chunk: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub read_transport: Option<unsafe extern "C" fn(*mut c_void, *mut u64, *mut u8)>,
    pub set_transport: Option<unsafe extern "C" fn(*mut c_void, u64, u8)>,
}

fn missing(ops: &HirariBounceHostOps) -> bool {
    ops.run.is_none()
        || ops.reset.is_none()
        || ops.store.is_none()
        || ops.render.is_none()
        || ops.render_chunk.is_none()
        || ops.read_transport.is_none()
        || ops.set_transport.is_none()
}

/// One engine's bounce state. The engine holds the opaque handle and answers
/// its realtime and control-plane questions from these fields; nothing here is
/// shared between engines.
pub struct BounceSession {
    offline_active: AtomicBool,
    offline_owner: AtomicU64,
    target_active: AtomicBool,
    target_id: AtomicU32,
    tail_millis: AtomicU32,
    pre_fader: AtomicBool,
    include_inserts: AtomicBool,
    running: AtomicBool,
    progress: AtomicU32,
    state: AtomicU32,
    cancel: AtomicBool,
    pause: AtomicBool,
    /// Serializes cancellation with the final temporary-file publish so a
    /// cancel cannot race a stale render into the destination path.
    publish_lock: Mutex<()>,
    /// Serializes render-thread retirement with a subsequent start. The worker
    /// publishes its terminal state before its guard clears `running`, so
    /// callers must be able to join a terminal worker before accepting the
    /// next render.
    lifecycle_lock: Mutex<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl BounceSession {
    fn new() -> Self {
        Self {
            offline_active: AtomicBool::new(false),
            offline_owner: AtomicU64::new(0),
            target_active: AtomicBool::new(false),
            target_id: AtomicU32::new(0),
            tail_millis: AtomicU32::new(0),
            pre_fader: AtomicBool::new(false),
            include_inserts: AtomicBool::new(true),
            running: AtomicBool::new(false),
            progress: AtomicU32::new(0.0_f32.to_bits()),
            state: AtomicU32::new(BOUNCE_STATE_IDLE),
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
            publish_lock: Mutex::new(()),
            lifecycle_lock: Mutex::new(()),
            worker: Mutex::new(None),
        }
    }

    /// Locks are never left poisoned by a bounce: a panic in the render loop
    /// must not permanently disable cancellation or retirement.
    fn publish_guard(&self) -> MutexGuard<'_, ()> {
        self.publish_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lifecycle_guard(&self) -> MutexGuard<'_, ()> {
        self.lifecycle_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn worker_slot(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.worker.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state_value(&self) -> u32 {
        self.state.load(Ordering::Acquire)
    }

    fn publish(&self, progress: f32, state: u32) {
        self.progress.store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Release);
        self.state.store(state, Ordering::Release);
    }

    /// Acquires offline-render ownership. `OWNERSHIP_ACQUIRED` means the graph
    /// is ours; both failures already published their terminal state.
    fn begin(&self) -> u8 {
        // Async callers clear these before publishing Queued so a cancellation
        // requested while queued is preserved; direct synchronous callers get
        // a fresh flag per invocation.
        if !self.running.load(Ordering::Acquire) {
            self.cancel.store(false, Ordering::Release);
            self.pause.store(false, Ordering::Release);
        }
        self.publish(0.0, BOUNCE_STATE_RENDERING);
        mark_offline_thread();
        let token = hirari_callback_thread_token();
        let deadline = Instant::now() + OWNERSHIP_DEADLINE;
        loop {
            if self
                .offline_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            if self.cancel.load(Ordering::Acquire) {
                unmark_offline_thread();
                self.state.store(BOUNCE_STATE_CANCELLED, Ordering::Release);
                return OWNERSHIP_CANCELLED;
            }
            if Instant::now() >= deadline {
                unmark_offline_thread();
                self.publish(0.0, BOUNCE_STATE_FAILED);
                log(0, b"BOUNCE_OFFLINE_RENDER_BUSY_TIMEOUT");
                return OWNERSHIP_BUSY;
            }
            std::thread::yield_now();
        }
        self.offline_owner.store(token, Ordering::Release);
        OWNERSHIP_ACQUIRED
    }

    fn end(&self) {
        self.offline_owner.store(0, Ordering::Release);
        self.offline_active.store(false, Ordering::Release);
        unmark_offline_thread();
    }

    /// True when the calling thread must not touch the shared graph: an offline
    /// render owns it and this thread is neither the owner nor the renderer.
    fn blocks_callback(&self, token: u64) -> bool {
        self.offline_active.load(Ordering::Acquire)
            && self.offline_owner.load(Ordering::Acquire) != token
            && !offline_thread()
    }

    /// True while an offline render is feeding this engine's graph, either
    /// because one holds ownership or because this thread is driving it.
    fn engaged(&self) -> bool {
        self.offline_active.load(Ordering::Acquire) || offline_thread()
    }

    fn control_bits(&self) -> u8 {
        let mut bits = 0;
        if self.cancel.load(Ordering::Acquire) {
            bits |= CONTROL_CANCEL;
        }
        if self.pause.load(Ordering::Acquire) {
            bits |= CONTROL_PAUSE;
        }
        bits
    }

    /// A streaming writer only publishes a complete, synced file. The cancel
    /// check and the publish share one lock so a cancel either precedes the
    /// publish or loses to it. 0 = cancelled, 1 = published, 2 = failed.
    fn finish_stream(&self, stream: *mut c_void) -> u8 {
        let _guard = self.publish_guard();
        if self.cancel.load(Ordering::Acquire) {
            return 0;
        }
        // SAFETY: the driver hands back the stream it opened for this render.
        if unsafe { hirari_bounce_stream_finish(stream) } != 0 {
            1
        } else {
            2
        }
    }

    fn request_cancel(&self) -> bool {
        if !hirari_bounce_state_cancellable(self.state_value()) {
            return false;
        }
        let _guard = self.publish_guard();
        self.cancel.store(true, Ordering::Release);
        self.pause.store(false, Ordering::Release);
        true
    }

    fn request_pause(&self) -> bool {
        if !hirari_bounce_state_pausable(self.state_value()) {
            return false;
        }
        self.pause.store(true, Ordering::Release);
        true
    }

    fn request_resume(&self) -> bool {
        if !hirari_bounce_state_resumable(self.state_value()) {
            return false;
        }
        self.pause.store(false, Ordering::Release);
        true
    }

    /// Queues a render, retiring a terminal worker first so a retry is never
    /// rejected while its thread is still finishing.
    fn start_async(
        &self,
        host: *mut c_void,
        ops: &HirariBounceHostOps,
        path: &[u8],
        format: u32,
    ) -> bool {
        let _guard = self.lifecycle_guard();
        let mut slot = self.worker_slot();
        if self.running.load(Ordering::Acquire) {
            if !hirari_bounce_state_terminal(self.state_value()) {
                return false;
            }
            if let Some(worker) = slot.take() {
                let _ = worker.join();
            }
            self.running.store(false, Ordering::Release);
        } else if let Some(worker) = slot.take() {
            let _ = worker.join();
        }
        self.running.store(true, Ordering::Release);
        self.cancel.store(false, Ordering::Release);
        self.pause.store(false, Ordering::Release);
        self.publish(0.0, BOUNCE_STATE_QUEUED);
        let job = RenderJob {
            session: self as *const BounceSession,
            host,
            ops: ops as *const HirariBounceHostOps,
            path: path.to_vec(),
            format,
        };
        match std::thread::Builder::new().spawn(move || job.run()) {
            Ok(worker) => {
                *slot = Some(worker);
                true
            }
            Err(_) => {
                self.running.store(false, Ordering::Release);
                self.publish(0.0, BOUNCE_STATE_FAILED);
                false
            }
        }
    }

    /// Cancels and joins whatever the session still owns. Teardown must not
    /// wait on a render that is still publishing blocks.
    fn join(&self) {
        self.cancel.store(true, Ordering::Release);
        let _guard = self.lifecycle_guard();
        if let Some(worker) = self.worker_slot().take() {
            let _ = worker.join();
        }
        self.running.store(false, Ordering::Release);
    }
}

#[path = "bounce_driver_render.rs"]
mod render_;
pub(crate) use render_::*;
#[path = "bounce_driver_ffi.rs"]
mod ffi;
pub use ffi::*;

#[cfg(test)]
#[path = "bounce_driver_tests.rs"]
mod bounce_driver_tests;

