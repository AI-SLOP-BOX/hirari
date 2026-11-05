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
use crate::track_session::{self, HirariTrackSessionOps};
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

/// The engine's tracks lock, held for one plan walk. The lock is not recursive,
/// so nothing that takes it itself may run inside the guard.
struct TracksLock<'a> {
    ops: &'a HirariTrackSessionOps,
    host: *mut c_void,
}

impl Drop for TracksLock<'_> {
    fn drop(&mut self) {
        self.ops.unlock_tracks(self.host);
    }
}

/// Every region span in the live graph, in track order. The region store takes
/// its own lock per track, so the spans are read while the table is pinned.
fn arrangement_spans(
    session: &HirariTrackSessionOps,
    host: *mut c_void,
    table: *mut c_void,
) -> Vec<RegionSpan> {
    let mut spans = Vec::new();
    session.lock_tracks(host);
    let _guard = TracksLock {
        ops: session,
        host,
    };
    let count = unsafe { hirari_engine_track_count(table) };
    let pointers = unsafe { hirari_engine_track_ptrs(table) };
    if pointers.is_null() {
        return spans;
    }
    // SAFETY: the table's buffer holds `count` entries while its lock is held.
    for &track in unsafe { slice::from_raw_parts(pointers, count) } {
        if track.is_null() {
            continue;
        }
        for index in 0..hirari_region_store_count(track) {
            let (mut start, mut len, mut loop_count) = (0_u64, 0_u64, 0_u32);
            // SAFETY: the three out-parameters are live locals.
            if unsafe {
                hirari_region_store_span_at(track, index, &mut start, &mut len, &mut loop_count)
            } {
                spans.push(RegionSpan {
                    start,
                    len,
                    loop_count,
                });
            }
        }
    }
    spans
}

/// The end of every scheduled note. A MIDI-only project has no regions, so its
/// extent comes from the notes the playback snapshot publishes.
fn scheduled_note_ends(store: *mut c_void) -> Vec<u64> {
    let mut count = 0_usize;
    // SAFETY: `count` is a live local and the store may be unknown (null).
    let notes: *const ScheduledMidiNoteFfi =
        unsafe { hirari_midi_notes_snapshot_take(store as *const c_void, &mut count) };
    if notes.is_null() {
        return Vec::new();
    }
    let mut ends = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: the taken buffer holds `count` notes and belongs to this call
        // until it is freed below.
        ends.push(unsafe { hirari_midi_scheduled_note_end(notes.add(index)) });
    }
    unsafe { hirari_midi_notes_snapshot_take_free(notes as *mut ScheduledMidiNoteFfi, count) };
    ends
}

/// Plans the render extent from the engine's own Rust stores. The caller logs a
/// rejection, because a rejected plan must publish a terminal state rather than
/// look like an immortal render.
fn gather_plan(
    host: *mut c_void,
    store: BounceStoreFn,
    session: &BounceSession,
) -> Result<HirariBouncePlan, i32> {
    // SAFETY: the host supplies the store handles its own graph owns.
    let (table, block, midi, limiter) = unsafe {
        (
            store(host, STORE_TRACK_TABLE),
            store(host, STORE_ENGINE_BLOCK),
            store(host, STORE_MIDI_SNAPSHOT),
            store(host, STORE_MASTER_LIMITER),
        )
    };
    // Without a graph — or without the engine's track session to pin it — there
    // is nothing to walk, and the arrangement falls back to one second.
    let regions = if table.is_null() {
        Vec::new()
    } else {
        track_session::engine_ops()
            .map_or_else(Vec::new, |tracks| arrangement_spans(&tracks, host, table))
    };
    let note_ends = scheduled_note_ends(midi);
    plan_extent(&BouncePlanInput {
        regions: &regions,
        midi_note_ends: &note_ends,
        // SAFETY: the block handle is the engine's own.
        sample_rate: unsafe { hirari_engine_block_device_sample_rate(block as *const c_void) },
        range_active: false,
        range_start: 0,
        range_end: 0,
        tail_millis: session.tail_millis.load(Ordering::Acquire),
        limiter_tail: u64::from(if limiter.is_null() {
            0
        } else {
            // SAFETY: the handle is the engine's master limiter.
            unsafe { hirari_master_limiter_tail(limiter as *const c_void) }
        }),
    })
}

/// A started render's body. The engine owns the session and outlives the
/// worker because teardown joins through [`BounceSession::join`], and the
/// operation table the worker calls through has static storage.
struct RenderJob {
    session: *const BounceSession,
    host: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: Vec<u8>,
    format: u32,
}

unsafe impl Send for RenderJob {}

/// Clears `running` when the worker leaves, including while unwinding, so the
/// retirement window the control plane joins against always closes.
struct RetireGuard<'a>(&'a BounceSession);

impl Drop for RetireGuard<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

impl RenderJob {
    fn run(self) {
        // SAFETY: see the ownership contract on [`RenderJob`].
        let session = unsafe { &*self.session };
        let _retire = RetireGuard(session);
        let Some(run) = (unsafe { &*self.ops }).run else {
            return;
        };
        // SAFETY: `path` outlives the call and the host pointer is the engine
        // that built this table.
        let ok =
            unsafe { run(self.host, self.path.as_ptr(), self.path.len(), self.format) } != 0;
        let cancelled = session.state_value() == BOUNCE_STATE_CANCELLED;
        log(
            if ok {
                2
            } else if cancelled {
                1
            } else {
                0
            },
            if ok {
                b"BOUNCE_COMPLETE"
            } else if cancelled {
                b"BOUNCE_CANCELLED"
            } else {
                b"BOUNCE_FAILED"
            },
        );
    }
}

fn bounce_session(handle: *mut c_void) -> Option<&'static BounceSession> {
    unsafe { (handle as *const BounceSession).as_ref() }
}

struct StreamGuard(*mut c_void);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        unsafe { hirari_bounce_stream_free(self.0) };
    }
}

struct Dither {
    left: *mut c_void,
    right: *mut c_void,
}

impl Dither {
    fn new() -> Self {
        Self {
            left: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_LEFT_SEED),
            right: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_RIGHT_SEED),
        }
    }

    fn next(&mut self) -> (f32, f32) {
        unsafe { (hirari_tpdf_dither_next(self.left), hirari_tpdf_dither_next(self.right)) }
    }
}

impl Drop for Dither {
    fn drop(&mut self) {
        unsafe {
            hirari_tpdf_dither_destroy(self.left);
            hirari_tpdf_dither_destroy(self.right);
        }
    }
}

/// Renders one project bounce. Returns true only when a complete file was
/// published; every other path leaves a terminal state behind.
///
/// # Safety
/// `host` must be the engine pointer the operations expect, `session` the
/// engine's own bounce session, `ops` a complete table and `path` `path_len`
/// readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_driver(
    host: *mut c_void,
    session: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: *const u8,
    path_len: usize,
    format: u32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let Some(session) = bounce_session(session) else {
        return false;
    };
    // SAFETY: the caller guarantees a complete table for this call.
    let ops = unsafe { &*ops };
    if missing(ops) {
        return false;
    }
    let (reset, store, render, render_chunk) = (
        ops.reset.unwrap(),
        ops.store.unwrap(),
        ops.render.unwrap(),
        ops.render_chunk.unwrap(),
    );
    let (read_transport, set_transport) = (ops.read_transport.unwrap(), ops.set_transport.unwrap());

    if !hirari_bounce_output_path_supported(path, path_len, format) {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        log(1, b"BOUNCE_UNSUPPORTED_FORMAT_OR_PATH");
        return false;
    }

    match session.begin() {
        OWNERSHIP_ACQUIRED => {}
        OWNERSHIP_CANCELLED => {
            session.publish(0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        _ => return false,
    }

    let plan_result = match gather_plan(host, store, session) {
        Ok(plan) => plan,
        Err(reason) => {
            log(
                1,
                if reason == BOUNCE_PLAN_INVALID_RANGE {
                    b"BOUNCE_INVALID_RANGE"
                } else {
                    b"BOUNCE_INVALID_SAMPLE_RATE"
                },
            );
            session.publish(0.0, BOUNCE_STATE_FAILED);
            session.end();
            return false;
        }
    };

    // Public formats map onto the Rust stream ABI: PCM16, WAVE64 float,
    // float32 RIFF/RF64, PCM24.
    const STREAM_FORMATS: [u8; 4] = [1, 3, 0, 2];
    let stream = StreamGuard(unsafe {
        hirari_bounce_stream_create(
            path,
            path_len,
            plan_result.total_samples,
            plan_result.output_rate,
            STREAM_FORMATS[format as usize],
        )
    });
    if stream.0.is_null() {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        log(0, b"BOUNCE_STREAM_OPEN_FAILED");
        session.end();
        return false;
    }

    let mut original_playhead = 0_u64;
    let mut original_playing = 0_u8;
    // SAFETY: both out-pointers are live locals; the transport belongs to the
    // engine that owns this session.
    unsafe { read_transport(host, &mut original_playhead, &mut original_playing) };
    unsafe { set_transport(host, plan_result.playhead_start, 1) };
    unsafe { reset(host) };

    let chunk = unsafe { render_chunk(host) }.clamp(1, MAX_RENDER_CHUNK) as usize;
    let mut left = vec![0.0_f32; chunk];
    let mut right = vec![0.0_f32; chunk];
    let dither_depth = match format {
        0 => 256.0_f32,
        3 => 1.0_f32,
        _ => 0.0_f32,
    };
    let mut dither = (dither_depth != 0.0).then(Dither::new);
    let total = plan_result.total_samples.min(u64::from(u32::MAX));
    let mut progress = 0.0_f32;
    let mut offset = 0_u64;
    while offset < total {
        let bits = session.control_bits();
        if bits & CONTROL_CANCEL != 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            session.end();
            session.publish(0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        if bits & CONTROL_PAUSE != 0 {
            session.publish(progress, BOUNCE_STATE_PAUSED);
            std::thread::sleep(PAUSE_POLL);
            continue;
        }
        session.publish(progress, BOUNCE_STATE_RENDERING);
        let frames = (total - offset).min(chunk as u64) as u32;
        left[..frames as usize].fill(0.0);
        right[..frames as usize].fill(0.0);
        // SAFETY: the host renders into the `frames` entries of each buffer.
        unsafe { render(host, left.as_mut_ptr(), right.as_mut_ptr(), frames) };
        if let Some(dither) = dither.as_mut() {
            for sample in 0..frames as usize {
                let (noise_left, noise_right) = dither.next();
                left[sample] += noise_left * dither_depth;
                right[sample] += noise_right * dither_depth;
            }
        }
        let channels: [*const f32; 2] = [left.as_ptr(), right.as_ptr()];
        if unsafe { hirari_bounce_stream_write(stream.0, channels.as_ptr(), frames) } == 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            session.end();
            session.publish(0.0, BOUNCE_STATE_FAILED);
            log(0, b"BOUNCE_STREAM_WRITE_FAILED");
            return false;
        }
        offset += u64::from(frames);
        progress = offset as f32 / total as f32;
    }

    unsafe { set_transport(host, original_playhead, original_playing) };
    match session.finish_stream(stream.0) {
        1 => {
            session.end();
            session.publish(1.0, BOUNCE_STATE_COMPLETED);
            true
        }
        published => {
            session.end();
            session.publish(0.0, BOUNCE_STATE_FAILED);
            log(
                0,
                if published == 0 {
                    b"BOUNCE_CANCELLED"
                } else {
                    b"BOUNCE_STREAM_PUBLISH_FAILED"
                },
            );
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_session_create() -> *mut c_void {
    Box::into_raw(Box::new(BounceSession::new())).cast()
}

/// # Safety
/// `handle` must come from [`hirari_bounce_session_create`] and have no live
/// borrower. Callers join the worker before destroying the session.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_session_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle.cast::<BounceSession>()) });
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_thread() -> u8 {
    u8::from(offline_thread())
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_active(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.offline_active.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_engaged(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(BounceSession::engaged))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_offline_blocks(handle: *mut c_void, token: u64) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.blocks_callback(token)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_target_active(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.target_active.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_target_id(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(0, |s| s.target_id.load(Ordering::Acquire))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_pre_fader(handle: *mut c_void) -> u8 {
    u8::from(bounce_session(handle).is_some_and(|s| s.pre_fader.load(Ordering::Acquire)))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_include_inserts(handle: *mut c_void) -> u8 {
    u8::from(
        bounce_session(handle)
            .map_or(true, |s| s.include_inserts.load(Ordering::Acquire)),
    )
}

#[no_mangle]
pub extern "C" fn hirari_bounce_tail_millis(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(0, |s| s.tail_millis.load(Ordering::Acquire))
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_offline_target(handle: *mut c_void, track_id: u32) {
    if let Some(session) = bounce_session(handle) {
        session.target_id.store(track_id, Ordering::Release);
        session.target_active.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_clear_offline_target(handle: *mut c_void) {
    if let Some(session) = bounce_session(handle) {
        session.target_active.store(false, Ordering::Release);
        session.target_id.store(0, Ordering::Release);
        session.tail_millis.store(0, Ordering::Release);
        session.pre_fader.store(false, Ordering::Release);
        session.include_inserts.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_tail_seconds(handle: *mut c_void, seconds: f32) {
    if let Some(session) = bounce_session(handle) {
        // Bounds and non-finite rejection are enforced by the bridge.
        session
            .tail_millis
            .store((seconds * 1000.0) as u32, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_set_render_options(
    handle: *mut c_void,
    pre_fader: u8,
    include_inserts: u8,
) {
    if let Some(session) = bounce_session(handle) {
        session
            .pre_fader
            .store(pre_fader != 0, Ordering::Release);
        session
            .include_inserts
            .store(include_inserts != 0, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_bounce_progress(handle: *mut c_void) -> f32 {
    bounce_session(handle).map_or(0.0, |s| {
        f32::from_bits(s.progress.load(Ordering::Acquire)).clamp(0.0, 1.0)
    })
}

#[no_mangle]
pub extern "C" fn hirari_bounce_state(handle: *mut c_void) -> u32 {
    bounce_session(handle).map_or(BOUNCE_STATE_IDLE, BounceSession::state_value)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_control_bits(handle: *mut c_void) -> u8 {
    bounce_session(handle).map_or(0, BounceSession::control_bits)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_cancel(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_cancel)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_pause(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_pause)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_resume(handle: *mut c_void) -> bool {
    bounce_session(handle).is_some_and(BounceSession::request_resume)
}

/// # Safety
/// `host`, `ops` and `path` follow the same contract as
/// [`hirari_bounce_render_driver`]; the engine must outlive the worker it
/// starts, which it does because teardown joins through [`hirari_bounce_join`].
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_start_async(
    handle: *mut c_void,
    host: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: *const u8,
    path_len: usize,
    format: u32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let Some(session) = bounce_session(handle) else {
        return false;
    };
    // SAFETY: the caller guarantees a complete table with static storage.
    let ops = unsafe { &*ops };
    if ops.run.is_none() {
        return false;
    }
    if format > 3 || path_len == 0 {
        session.publish(0.0, BOUNCE_STATE_FAILED);
        return false;
    }
    // SAFETY: the caller passes `path_len` readable bytes for this call.
    let path = unsafe { slice::from_raw_parts(path, path_len) };
    session.start_async(host, ops, path, format)
}

#[no_mangle]
pub extern "C" fn hirari_bounce_join(handle: *mut c_void) {
    if let Some(session) = bounce_session(handle) {
        session.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounce_plan::BOUNCE_PLAN_INVALID_RATE;
    use crate::engine_block_host::{
        hirari_engine_block_create, hirari_engine_block_destroy, HirariEngineBlockHandles,
    };
    use crate::master_limiter::{hirari_master_limiter_create, hirari_master_limiter_destroy};

    struct Handle(*mut c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { hirari_bounce_session_destroy(self.0) };
        }
    }

    fn new_session() -> Handle {
        Handle(hirari_bounce_session_create())
    }

    fn borrow(handle: &Handle) -> &'static BounceSession {
        unsafe { &*(handle.0.cast::<BounceSession>()) }
    }

    /// A host that owns the Rust stores the plan walk reads, so the extent can
    /// be checked against real handles instead of a scripted callback.
    struct FakeEngine {
        block: *mut c_void,
        limiter: *mut c_void,
    }

    impl FakeEngine {
        fn new() -> Self {
            Self {
                // SAFETY: the handles are read once and all null.
                block: unsafe { hirari_engine_block_create(HirariEngineBlockHandles::default()) },
                limiter: hirari_master_limiter_create(44_100.0),
            }
        }

        fn host(&self) -> *mut c_void {
            self as *const Self as *mut c_void
        }

        fn plan(&self, session: &BounceSession) -> Result<HirariBouncePlan, i32> {
            gather_plan(self.host(), fake_store, session)
        }
    }

    impl Drop for FakeEngine {
        fn drop(&mut self) {
            unsafe {
                hirari_engine_block_destroy(self.block);
                hirari_master_limiter_destroy(self.limiter);
            }
        }
    }

    /// The graph stores stay absent: an empty arrangement is the fallback case.
    unsafe extern "C" fn fake_store(host: *mut c_void, kind: u32) -> *mut c_void {
        let engine = unsafe { &*(host as *const FakeEngine) };
        match kind {
            STORE_ENGINE_BLOCK => engine.block,
            STORE_MASTER_LIMITER => engine.limiter,
            _ => std::ptr::null_mut(),
        }
    }

    /// The fake host doubles as the session: the worker body only publishes.
    fn start(handle: &Handle, ops: &HirariBounceHostOps, path: &[u8], format: u32) -> bool {
        unsafe {
            hirari_bounce_start_async(
                handle.0,
                handle.0,
                ops,
                path.as_ptr(),
                path.len(),
                format,
            )
        }
    }

    #[test]
    fn thread_tokens_are_stable_per_thread_and_unique_across_threads() {
        let first = hirari_callback_thread_token();
        assert_eq!(first, hirari_callback_thread_token());
        let other = std::thread::spawn(|| hirari_callback_thread_token())
            .join()
            .expect("token thread");
        assert_ne!(first, other);
    }

    #[test]
    fn missing_operations_fail_closed() {
        let ops = HirariBounceHostOps {
            run: None,
            reset: None,
            store: None,
            render: None,
            render_chunk: None,
            read_transport: None,
            set_transport: None,
        };
        assert!(missing(&ops));
        let guard = new_session();
        let path = b"/tmp/hirari-bounce.wav";
        let ok = unsafe {
            hirari_bounce_render_driver(
                &mut 0_u8 as *mut u8 as *mut c_void,
                guard.0,
                &ops,
                path.as_ptr(),
                path.len(),
                0,
            )
        };
        assert!(!ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_IDLE);
    }

    #[test]
    fn the_plan_extends_the_device_rate_with_the_session_and_limiter_tails() {
        let engine = FakeEngine::new();
        let guard = new_session();
        let session = borrow(&guard);
        let baseline = engine.plan(session).expect("empty arrangement");
        assert_eq!(baseline.output_rate, 44_100);
        assert_eq!(baseline.playhead_start, 0);
        // The fallback one second plus the master limiter's own release tail.
        assert_eq!(
            baseline.total_samples,
            44_100 + u64::from(unsafe { hirari_master_limiter_tail(engine.limiter as *const c_void) })
        );
        hirari_bounce_set_tail_seconds(guard.0, 0.5);
        let extended = engine.plan(session).expect("tailed plan");
        assert_eq!(extended.total_samples, baseline.total_samples + 22_050);
    }

    #[test]
    fn an_unsupported_device_rate_rejects_the_render() {
        unsafe extern "C" fn zero_rate(_: *mut c_void, _: u32) -> *mut c_void {
            std::ptr::null_mut()
        }
        let guard = new_session();
        // No engine block means no known rate, which is a rejected plan rather
        // than a silent one-second render.
        let error = gather_plan(
            &mut 0_u8 as *mut u8 as *mut c_void,
            zero_rate,
            borrow(&guard),
        )
        .expect_err("rate must be read from the graph");
        assert_eq!(error, BOUNCE_PLAN_INVALID_RATE);
    }

    #[test]
    fn unsupported_output_publishes_a_failed_state() {
        unsafe extern "C" fn run(_: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            1
        }
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn store(_: *mut c_void, _: u32) -> *mut c_void {
            std::ptr::null_mut()
        }
        unsafe extern "C" fn render(_: *mut c_void, _: *mut f32, _: *mut f32, _: u32) {}
        unsafe extern "C" fn chunk(_: *mut c_void) -> u32 {
            64
        }
        unsafe extern "C" fn read(_: *mut c_void, playhead: *mut u64, playing: *mut u8) {
            unsafe {
                *playhead = 7;
                *playing = 0;
            }
        }
        unsafe extern "C" fn set(_: *mut c_void, _: u64, _: u8) {}
        let ops = HirariBounceHostOps {
            run: Some(run),
            reset: Some(reset),
            store: Some(store),
            render: Some(render),
            render_chunk: Some(chunk),
            read_transport: Some(read),
            set_transport: Some(set),
        };
        let guard = new_session();
        let path = b"/tmp/hirari-bounce.txt";
        let ok = unsafe {
            hirari_bounce_render_driver(
                &mut 0_u8 as *mut u8 as *mut c_void,
                guard.0,
                &ops,
                path.as_ptr(),
                path.len(),
                0,
            )
        };
        assert!(!ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_FAILED);
        assert_eq!(hirari_bounce_progress(guard.0), 0.0);
        // The rejected render never took graph ownership.
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
    }

    #[test]
    fn ownership_gate_publishes_progress_state_and_protects_the_graph() {
        let guard = new_session();
        let state = borrow(&guard);
        assert_eq!(hirari_bounce_offline_blocks(guard.0, 1), 0);
        assert_eq!(state.begin(), OWNERSHIP_ACQUIRED);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_RENDERING);
        assert_eq!(hirari_bounce_offline_active(guard.0), 1);
        assert_eq!(hirari_bounce_offline_thread(), 1);
        assert_eq!(hirari_bounce_offline_engaged(guard.0), 1);
        // The owning thread is never blocked by its own render.
        assert_eq!(hirari_bounce_offline_blocks(guard.0, hirari_callback_thread_token()), 0);
        // Any other thread must keep out of the temporary offline graph.
        let address = guard.0 as usize;
        let foreign = std::thread::spawn(move || {
            hirari_bounce_offline_blocks(address as *mut c_void, 9_999)
        })
        .join()
        .expect("callback thread");
        assert_eq!(foreign, 1);
        state.end();
        assert_eq!(hirari_bounce_offline_thread(), 0);
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
    }

    #[test]
    fn a_cancel_requested_while_the_graph_is_busy_abandons_the_render() {
        let guard = new_session();
        let state = borrow(&guard);
        // A queued async render keeps its cancellation request; begin() only
        // discards a stale flag for a fresh synchronous bounce.
        state.running.store(true, Ordering::Release);
        assert_eq!(state.begin(), OWNERSHIP_ACQUIRED);
        // A second render on another thread can only wait for this one.
        let address = guard.0 as usize;
        let other = std::thread::spawn(move || {
            let other = unsafe { &*(address as *const BounceSession) };
            other.cancel.store(true, Ordering::Release);
            other.begin()
        })
        .join()
        .expect("waiting render");
        assert_eq!(other, OWNERSHIP_CANCELLED);
        // The abandoned render must not have released the first one's gate.
        assert_eq!(hirari_bounce_offline_active(guard.0), 1);
        state.end();
    }

    #[test]
    fn control_requests_follow_the_lifecycle() {
        let guard = new_session();
        let state = borrow(&guard);
        assert!(!hirari_bounce_pause(guard.0));
        assert!(!hirari_bounce_resume(guard.0));
        assert!(!hirari_bounce_cancel(guard.0));
        state.publish(0.4, BOUNCE_STATE_RENDERING);
        assert!(hirari_bounce_progress(guard.0) > 0.39);
        assert!(hirari_bounce_pause(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), CONTROL_PAUSE);
        state.publish(0.4, BOUNCE_STATE_PAUSED);
        assert!(hirari_bounce_resume(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), 0);
        // Cancelling a running render clears a pending pause so the worker
        // cannot stall on the pause gate before it sees the cancellation.
        state.publish(0.4, BOUNCE_STATE_RENDERING);
        assert!(hirari_bounce_pause(guard.0));
        assert!(hirari_bounce_cancel(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), CONTROL_CANCEL);
        // A finished render accepts no further control requests.
        state.publish(1.0, BOUNCE_STATE_COMPLETED);
        assert!(!hirari_bounce_cancel(guard.0));
        assert!(!hirari_bounce_pause(guard.0));
    }

    #[test]
    fn cancellation_beats_the_publish_boundary() {
        let guard = new_session();
        let state = borrow(&guard);
        // A stream that cannot be finished reports a publish failure.
        assert_eq!(state.finish_stream(std::ptr::null_mut()), 2);
        state.cancel.store(true, Ordering::Release);
        assert_eq!(state.finish_stream(std::ptr::null_mut()), 0);
    }

    #[test]
    fn offline_target_options_round_trip() {
        let guard = new_session();
        hirari_bounce_set_offline_target(guard.0, 12);
        assert_eq!(hirari_bounce_target_active(guard.0), 1);
        assert_eq!(hirari_bounce_target_id(guard.0), 12);
        hirari_bounce_set_tail_seconds(guard.0, 1.5);
        assert_eq!(hirari_bounce_tail_millis(guard.0), 1_500);
        hirari_bounce_set_render_options(guard.0, 1, 0);
        assert_eq!(hirari_bounce_pre_fader(guard.0), 1);
        assert_eq!(hirari_bounce_include_inserts(guard.0), 0);
        hirari_bounce_clear_offline_target(guard.0);
        assert_eq!(hirari_bounce_target_active(guard.0), 0);
        assert_eq!(hirari_bounce_target_id(guard.0), 0);
        assert_eq!(hirari_bounce_tail_millis(guard.0), 0);
        assert_eq!(hirari_bounce_pre_fader(guard.0), 0);
        assert_eq!(hirari_bounce_include_inserts(guard.0), 1);
    }

    #[test]
    fn an_async_render_is_retired_before_the_next_start() {
        static DONE: AtomicU32 = AtomicU32::new(0);
        static RELEASE: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn run(host: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            while !RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            DONE.fetch_add(1, Ordering::SeqCst);
            // Stand in for the synchronous driver: publish a terminal state.
            let session = unsafe { &*(host as *const BounceSession) };
            session.publish(1.0, BOUNCE_STATE_COMPLETED);
            1
        }
        static OPS: HirariBounceHostOps = HirariBounceHostOps {
            run: Some(run),
            reset: None,
            store: None,
            render: None,
            render_chunk: None,
            read_transport: None,
            set_transport: None,
        };
        DONE.store(0, Ordering::SeqCst);
        RELEASE.store(false, Ordering::SeqCst);
        let guard = new_session();
        // A rejected path or format publishes the failure without a worker.
        assert!(!start(&guard, &OPS, b"", 0));
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_FAILED);
        let path = b"/tmp/hirari-async.wav";
        assert!(start(&guard, &OPS, path, 0));
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_QUEUED);
        // A running render is refused until it retires.
        assert!(!start(&guard, &OPS, path, 0));
        RELEASE.store(true, Ordering::SeqCst);
        hirari_bounce_join(guard.0);
        assert_eq!(DONE.load(Ordering::SeqCst), 1);
        assert!(!borrow(&guard).running.load(Ordering::Acquire));
        // The retired slot accepts the next render.
        RELEASE.store(false, Ordering::SeqCst);
        assert!(start(&guard, &OPS, path, 0));
        RELEASE.store(true, Ordering::SeqCst);
        hirari_bounce_join(guard.0);
        assert_eq!(DONE.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_completed_render_publishes_the_whole_file() {
        unsafe extern "C" fn run(_: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            1
        }
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn render(_: *mut c_void, left: *mut f32, right: *mut f32, frames: u32) {
            for index in 0..frames as usize {
                unsafe {
                    *left.add(index) = 0.25;
                    *right.add(index) = -0.25;
                }
            }
        }
        unsafe extern "C" fn chunk(_: *mut c_void) -> u32 {
            1_024
        }
        unsafe extern "C" fn read(_: *mut c_void, playhead: *mut u64, playing: *mut u8) {
            unsafe {
                *playhead = 3;
                *playing = 0;
            }
        }
        unsafe extern "C" fn set(_: *mut c_void, playhead: u64, _: u8) {
            LAST_TRANSPORT.store(playhead, Ordering::SeqCst);
        }
        static LAST_TRANSPORT: AtomicU64 = AtomicU64::new(0);
        let engine = FakeEngine::new();
        let ops = HirariBounceHostOps {
            run: Some(run),
            reset: Some(reset),
            store: Some(fake_store),
            render: Some(render),
            render_chunk: Some(chunk),
            read_transport: Some(read),
            set_transport: Some(set),
        };
        let guard = new_session();
        let plan = engine
            .plan(borrow(&guard))
            .expect("the fake graph plans a render");
        let directory = std::env::temp_dir().join(format!("hirari-bounce-{:p}", guard.0));
        std::fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("mix.wav");
        let raw = path.to_string_lossy().into_owned();
        let ok = unsafe {
            hirari_bounce_render_driver(
                engine.host(),
                guard.0,
                &ops,
                raw.as_ptr().cast(),
                raw.len(),
                0,
            )
        };
        assert!(ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_COMPLETED);
        assert_eq!(hirari_bounce_progress(guard.0), 1.0);
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
        assert_eq!(hirari_bounce_offline_thread(), 0);
        // The transport is handed back to whatever the engine had before.
        assert_eq!(LAST_TRANSPORT.load(Ordering::SeqCst), 3);
        // The published file is exactly the extent the Rust plan measured.
        let payload = plan.total_samples * 4;
        let written = std::fs::metadata(&path).expect("rendered file").len();
        assert!(
            written >= payload && written <= payload + 4_096,
            "{written} bytes for {payload} samples of payload"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
