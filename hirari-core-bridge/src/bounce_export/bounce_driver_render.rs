//! Render-side machinery for the bounce driver, split out of bounce_driver.rs.
use super::*;

/// Every region span in the live graph, in track order. The region store takes
/// its own lock per track, so the spans are read while the table is pinned.
pub(crate) fn arrangement_spans(host: *mut c_void, table: *mut c_void) -> Vec<RegionSpan> {
    let mut spans = Vec::new();
    let _guard = match track_session::GraphLock::acquire(host) {
        Some(guard) => guard,
        None => return spans,
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
pub(crate) fn scheduled_note_ends(store: *mut c_void) -> Vec<u64> {
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
pub(crate) fn gather_plan(
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
        arrangement_spans(host, table)
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
pub(crate) struct RenderJob {
    pub(crate) session: *const BounceSession,
    pub(crate) host: *mut c_void,
    pub(crate) ops: *const HirariBounceHostOps,
    pub(crate) path: Vec<u8>,
    pub(crate) format: u32,
}

unsafe impl Send for RenderJob {}

/// Clears `running` when the worker leaves, including while unwinding, so the
/// retirement window the control plane joins against always closes.
pub(crate) struct RetireGuard<'a>(pub(crate) &'a BounceSession);

impl Drop for RetireGuard<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

impl RenderJob {
    pub(crate) fn run(self) {
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

pub(crate) fn bounce_session(handle: *mut c_void) -> Option<&'static BounceSession> {
    unsafe { (handle as *const BounceSession).as_ref() }
}

pub(crate) struct StreamGuard(pub(crate) *mut c_void);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        unsafe { hirari_bounce_stream_free(self.0) };
    }
}

pub(crate) struct Dither {
    left: *mut c_void,
    right: *mut c_void,
}

impl Dither {
    pub(crate) fn new() -> Self {
        Self {
            left: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_LEFT_SEED),
            right: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_RIGHT_SEED),
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
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
