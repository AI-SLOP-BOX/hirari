//! Asynchronous region jobs: media import and audio-pitch analysis.
//!
//! Decoding and F0 analysis are slow, so both run on a worker thread while the
//! control thread keeps polling. This module owns the whole lifecycle the
//! native engine used to carry: request validation, the job registry and its
//! limits, worker threads, staleness checks, the commit and the undo capture.
//! The native engine only decodes media, inserts a region, snapshots a note
//! table and runs the analysis kernel it already owns.
//!
//! Every public entry point runs on the control thread. Workers only touch
//! retained, immutable handles (decoded audio and a retained track).

use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use crate::region_edit_host::{
    host_ops, record, HirariRegionEditPayload, HirariRegionHostOps, HirariRegionState,
    HIRARI_REGION_EDIT_NOTE_STORE, HIRARI_REGION_EDIT_REMOVE, HIRARI_REGION_EDIT_RESTORE,
};
use crate::undo_transaction::hirari_undo_manager_transaction_active;
use crate::vari_audio::{hirari_region_vari_audio_store_clear, HirariRegionVariAudioStore};

const STATUS_DECODING: u8 = 1;
const STATUS_READY: u8 = 2;
const STATUS_APPLIED: u8 = 3;
const STATUS_FAILED: u8 = 4;

const MAX_ACTIVE_IMPORTS: usize = 2;
const MAX_TRACKED_IMPORTS: usize = 8;
const MAX_ACTIVE_ANALYSES: usize = 2;
const MAX_PATH_BYTES: usize = 4096;
const MIN_SAMPLE_RATE: f64 = 8000.0;
const MAX_SAMPLE_RATE: f64 = 384_000.0;

/// Import result codes shared with the native wrapper, which turns them into
/// the user-visible diagnostic posts.
pub const IMPORT_OK: i32 = 0;
pub const IMPORT_TRACK_MISSING: i32 = 1;
pub const IMPORT_DECODE_FAILED: i32 = 2;
/// The media decoded but the commit was refused (rates out of range, or the
/// track was replaced while decoding). The caller reports this silently, as
/// the native commit path always did.
pub const IMPORT_COMMIT_REFUSED: i32 = 3;

/// The host pointer is only ever handed back to the host table it came from.
struct HostPtr(*mut c_void);

// The engine outlives every job: shutdown joins the workers first.
unsafe impl Send for HostPtr {}
unsafe impl Sync for HostPtr {}

fn host_key(host: *mut c_void) -> usize {
    host as usize
}

fn transaction_active(ops: &HirariRegionHostOps, host: *mut c_void) -> bool {
    let Some(undo_state) = ops.undo_state else {
        return false;
    };
    let state = unsafe { undo_state(host) };
    !state.is_null() && unsafe { hirari_undo_manager_transaction_active(state) }
}

/// An owned decoded buffer. Dropping it releases the native reference; the
/// worker only retains it until the import commits or fails.
struct AudioHandle {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

unsafe impl Send for AudioHandle {}
unsafe impl Sync for AudioHandle {}

impl AudioHandle {
    fn decode(ops: &HirariRegionHostOps, host: *mut c_void, path: &[u8]) -> Option<(Self, f64)> {
        let decode = ops.decode_audio?;
        let mut handle = ptr::null_mut();
        let mut sample_rate = 0.0;
        if !unsafe { decode(host, path.as_ptr(), path.len(), &mut handle, &mut sample_rate) }
            || handle.is_null()
        {
            return None;
        }
        Some((
            Self {
                ops: *ops,
                handle,
            },
            sample_rate,
        ))
    }

    /// Hands the buffer to the native region. The host takes ownership.
    fn into_raw(self) -> *mut c_void {
        let handle = self.handle;
        std::mem::forget(self);
        handle
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

/// An owned note-table snapshot used for the analysis undo pair.
struct NoteStore {
    handle: *mut HirariRegionVariAudioStore,
}

unsafe impl Send for NoteStore {}
unsafe impl Sync for NoteStore {}

impl NoteStore {
    fn into_raw(self) -> *mut u8 {
        let handle = self.handle;
        std::mem::forget(self);
        handle as *mut u8
    }
}

impl Drop for NoteStore {
    fn drop(&mut self) {
        unsafe { hirari_region_vari_audio_store_clear(self.handle) };
    }
}

/// A track kept alive for the duration of an analysis worker, so removing the
/// track from the project cannot free memory the worker is still reading.
struct RetainedTrack {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

unsafe impl Send for RetainedTrack {}
unsafe impl Sync for RetainedTrack {}

impl RetainedTrack {
    fn retain(ops: &HirariRegionHostOps, host: *mut c_void, track_id: u32) -> Option<Self> {
        let retain = ops.retain_track?;
        let handle = unsafe { retain(host, track_id) };
        if handle.is_null() {
            return None;
        }
        Some(Self {
            ops: *ops,
            handle,
        })
    }

    fn identity(ops: &HirariRegionHostOps, host: *mut c_void, track_id: u32) -> usize {
        match ops.track_identity {
            Some(identity) => unsafe { identity(host, track_id) },
            None => 0,
        }
    }

    fn pitch_begin(&self) -> bool {
        match self.ops.pitch_begin {
            Some(begin) => unsafe { begin(self.handle) },
            None => false,
        }
    }

    fn pitch_analyze(&self, region_id: u32, sample_rate: f64) -> bool {
        match self.ops.pitch_analyze {
            Some(analyze) => unsafe { analyze(self.handle, region_id, sample_rate) },
            None => false,
        }
    }

    fn pitch_finish(&self) {
        if let Some(finish) = self.ops.pitch_finish {
            unsafe { finish(self.handle) };
        }
    }

    fn note_snapshot(&self, region_id: u32) -> Option<NoteStore> {
        let snapshot = self.ops.note_snapshot?;
        let handle = unsafe { snapshot(self.handle, region_id) };
        if handle.is_null() {
            return None;
        }
        Some(NoteStore {
            handle: handle as *mut HirariRegionVariAudioStore,
        })
    }
}

impl Drop for RetainedTrack {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_track {
            unsafe { release(self.handle) };
        }
    }
}

struct ImportJob {
    host: HostPtr,
    track_id: u32,
    identity: usize,
    start_sample: u64,
    path: Vec<u8>,
    status: AtomicU8,
    audio: Mutex<Option<AudioHandle>>,
    source_rate: Mutex<f64>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ImportJob {
    fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }

    fn is_active(&self) -> bool {
        matches!(self.status(), STATUS_DECODING | STATUS_READY)
    }
}

type ImportRegistry = Mutex<HashMap<(usize, u64), Arc<ImportJob>>>;

fn import_registry() -> &'static ImportRegistry {
    static REGISTRY: OnceLock<ImportRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

struct PitchJob {
    identity: usize,
    status: AtomicU8,
    track: RetainedTrack,
    before: Mutex<Option<NoteStore>>,
    after: Mutex<Option<NoteStore>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl PitchJob {
    fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }
}

type PitchRegistry = Mutex<HashMap<(usize, u64), Arc<PitchJob>>>;

fn pitch_registry() -> &'static PitchRegistry {
    static REGISTRY: OnceLock<PitchRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn region_key(track_id: u32, region_id: u32) -> u64 {
    (u64::from(track_id) << 32) | u64::from(region_id)
}

fn valid_sample_rate(rate: f64) -> bool {
    rate.is_finite() && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate)
}

fn edit_payload(
    track_id: u32,
    region_id: u32,
    op: i32,
    blob: *const u8,
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op,
        blob,
        blob_len: 0,
        state: HirariRegionState::default(),
    }
}

/// Validates the rates and asks the host to insert the region, then records
/// exactly one "Add Region" undo step around it. The audio handle is always
/// consumed by the host, whether or not the insert succeeds.
fn commit_import(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    identity: usize,
    path: &[u8],
    start_sample: u64,
    audio: AudioHandle,
    source_rate: f64,
) -> bool {
    let (Some(insert), Some(identity_fn), Some(sample_rate), Some(snapshot)) = (
        ops.insert_region,
        ops.track_identity,
        ops.sample_rate,
        ops.snapshot_region,
    ) else {
        return false;
    };
    if identity == 0 || unsafe { identity_fn(host, track_id) } != identity {
        return false;
    }
    let timeline_rate = unsafe { sample_rate(host) };
    if !valid_sample_rate(source_rate) || !valid_sample_rate(timeline_rate) {
        return false;
    }
    let region_id = unsafe {
        insert(
            host,
            track_id,
            identity,
            path.as_ptr(),
            path.len(),
            start_sample,
            source_rate,
            timeline_rate,
            audio.into_raw(),
        )
    };
    if region_id == 0 {
        return false;
    }
    let snapshot_handle = unsafe { snapshot(host, track_id, region_id) };
    if snapshot_handle.is_null() {
        // The region exists; without a snapshot the edit cannot be undone,
        // but the import itself succeeded.
        return true;
    }
    let undo = [edit_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_REMOVE,
        ptr::null(),
    )];
    let redo = [edit_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_RESTORE,
        snapshot_handle as *const u8,
    )];
    record(ops, host, b"Add Region\0", &undo, &redo, false);
    true
}

fn import_commit_job(job: &ImportJob) -> bool {
    let Some(audio) = job.audio.lock().ok().and_then(|mut slot| slot.take()) else {
        return false;
    };
    let source_rate = job
        .source_rate
        .lock()
        .map(|rate| *rate)
        .unwrap_or(0.0);
    let Some(ops) = host_ops() else {
        return false;
    };
    let HostPtr(host) = job.host;
    commit_import(
        &ops,
        host,
        job.track_id,
        job.identity,
        &job.path,
        job.start_sample,
        audio,
        source_rate,
    )
}

/// Starts a synchronous import. Returns one of the `IMPORT_*` codes.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect, and
/// `path` must point to `path_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_import_add(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_len: usize,
    start_sample: f64,
) -> i32 {
    if host.is_null() || track_id == 0 {
        return IMPORT_TRACK_MISSING;
    }
    if path.is_null() || path_len == 0 {
        return IMPORT_DECODE_FAILED;
    }
    let Some(ops) = host_ops() else {
        return IMPORT_DECODE_FAILED;
    };
    let identity = RetainedTrack::identity(&ops, host, track_id);
    if identity == 0 {
        return IMPORT_TRACK_MISSING;
    }
    let path = unsafe { std::slice::from_raw_parts(path, path_len) };
    let Some((audio, source_rate)) = AudioHandle::decode(&ops, host, path) else {
        return IMPORT_DECODE_FAILED;
    };
    let start = if start_sample.is_nan() || start_sample < 0.0 {
        0
    } else {
        start_sample as u64
    };
    if commit_import(&ops, host, track_id, identity, path, start, audio, source_rate) {
        IMPORT_OK
    } else {
        IMPORT_COMMIT_REFUSED
    }
}

/// Queues an asynchronous import and returns its ticket, or 0 when the track
/// is missing or the registry is full.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect, and
/// `path` must point to `path_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_import_queue(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_len: usize,
    start_sample: u64,
) -> u64 {
    if host.is_null()
        || track_id == 0
        || path.is_null()
        || path_len == 0
        || path_len > MAX_PATH_BYTES
    {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, path_len) };
    if bytes.contains(&0) {
        return 0;
    }
    let Some(ops) = host_ops() else {
        return 0;
    };
    let identity = RetainedTrack::identity(&ops, host, track_id);
    if identity == 0 {
        return 0;
    }
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    if ticket == 0 {
        return 0;
    }
    let job = Arc::new(ImportJob {
        host: HostPtr(host),
        track_id,
        identity,
        start_sample,
        path: bytes.to_vec(),
        status: AtomicU8::new(STATUS_DECODING),
        audio: Mutex::new(None),
        source_rate: Mutex::new(0.0),
        thread: Mutex::new(None),
    });
    {
        let Ok(mut registry) = import_registry().lock() else {
            return 0;
        };
        let mut active = 0;
        let mut total = 0;
        for ((registry_host, _), existing) in registry.iter() {
            if *registry_host != host_key(host) {
                continue;
            }
            total += 1;
            if existing.is_active() {
                active += 1;
            }
        }
        if active >= MAX_ACTIVE_IMPORTS || total >= MAX_TRACKED_IMPORTS {
            return 0;
        }
        registry.insert((host_key(host), ticket), Arc::clone(&job));
    }
    let worker = Arc::clone(&job);
    let spawned = std::thread::Builder::new()
        .name("hirari-region-import".to_owned())
        .spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let HostPtr(host) = worker.host;
                let decoded = AudioHandle::decode(&ops, host, &worker.path);
                match decoded {
                    Some((audio, rate)) => {
                        if let Ok(mut slot) = worker.audio.lock() {
                            *slot = Some(audio);
                        }
                        if let Ok(mut stored_rate) = worker.source_rate.lock() {
                            *stored_rate = rate;
                        }
                        worker.status.store(STATUS_READY, Ordering::Release);
                    }
                    None => worker.status.store(STATUS_FAILED, Ordering::Release),
                }
            }));
            if worker.status() == STATUS_DECODING {
                worker.status.store(STATUS_FAILED, Ordering::Release);
            }
        });
    match spawned {
        Ok(thread) => {
            if let Ok(mut slot) = job.thread.lock() {
                *slot = Some(thread);
            }
            ticket
        }
        Err(_) => {
            if let Ok(mut registry) = import_registry().lock() {
                registry.remove(&(host_key(host), ticket));
            }
            0
        }
    }
}

/// Polls and, once the decode is ready, commits a queued import.
/// 0 = pending, 1 = applied, 2 = failed, 3 = unknown ticket.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_import_complete(host: *mut c_void, ticket: u64) -> u8 {
    if host.is_null() {
        return 3;
    }
    let Some(ops) = host_ops() else {
        return 3;
    };
    let job = {
        let Ok(mut registry) = import_registry().lock() else {
            return 2;
        };
        let Some(existing) = registry.get(&(host_key(host), ticket)).cloned() else {
            return 3;
        };
        match existing.status() {
            STATUS_DECODING => return 0,
            STATUS_READY if transaction_active(&ops, host) => return 0,
            STATUS_READY => {}
            _ => {
                registry.remove(&(host_key(host), ticket));
                return 2;
            }
        }
        registry.remove(&(host_key(host), ticket));
        existing
    };
    if import_commit_job(&job) {
        1
    } else {
        2
    }
}

/// Starts an asynchronous F0 analysis. Returns false when the region is
/// missing, an analysis is already running for the track, or the registry is
/// full.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_analysis_start(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    sample_rate: f64,
) -> bool {
    if host.is_null() || track_id == 0 || region_id == 0 || !valid_sample_rate(sample_rate) {
        return false;
    }
    let Some(ops) = host_ops() else {
        return false;
    };
    let Some(track) = RetainedTrack::retain(&ops, host, track_id) else {
        return false;
    };
    if !track.pitch_begin() {
        return false;
    }
    let Some(before) = track.note_snapshot(region_id) else {
        track.pitch_finish();
        return false;
    };
    let key = region_key(track_id, region_id);
    let job = Arc::new(PitchJob {
        identity: RetainedTrack::identity(&ops, host, track_id),
        status: AtomicU8::new(STATUS_DECODING),
        track,
        before: Mutex::new(Some(before)),
        after: Mutex::new(None),
        thread: Mutex::new(None),
    });
    {
        let Ok(mut registry) = pitch_registry().lock() else {
            job.track.pitch_finish();
            return false;
        };
        let mut active = 0;
        for ((registry_host, _), existing) in registry.iter() {
            if *registry_host != host_key(host) {
                continue;
            }
            if matches!(existing.status.load(Ordering::Acquire), STATUS_DECODING | STATUS_READY) {
                active += 1;
            }
        }
        let duplicate = registry
            .get(&(host_key(host), key))
            .is_some_and(|existing| {
                matches!(existing.status.load(Ordering::Acquire), STATUS_DECODING | STATUS_READY)
            });
        if active >= MAX_ACTIVE_ANALYSES || duplicate {
            job.track.pitch_finish();
            return false;
        }
        registry.insert((host_key(host), key), Arc::clone(&job));
    }
    let worker = Arc::clone(&job);
    let spawned = std::thread::Builder::new()
        .name("hirari-pitch-analysis".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let analyzed = worker.track.pitch_analyze(region_id, sample_rate);
                let after = if analyzed {
                    worker.track.note_snapshot(region_id)
                } else {
                    None
                };
                (analyzed, after)
            }));
            worker.track.pitch_finish();
            match result {
                Ok((true, Some(after))) => {
                    if let Ok(mut slot) = worker.after.lock() {
                        *slot = Some(after);
                    }
                    worker.status.store(STATUS_READY, Ordering::Release);
                }
                _ => worker.status.store(STATUS_FAILED, Ordering::Release),
            }
        });
    match spawned {
        Ok(thread) => {
            if let Ok(mut slot) = job.thread.lock() {
                *slot = Some(thread);
            }
            true
        }
        Err(_) => {
            job.track.pitch_finish();
            if let Ok(mut registry) = pitch_registry().lock() {
                registry.remove(&(host_key(host), key));
            }
            false
        }
    }
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_analysis_status(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> u8 {
    if host.is_null() {
        return 0;
    }
    let Ok(registry) = pitch_registry().lock() else {
        return 0;
    };
    registry
        .get(&(host_key(host), region_key(track_id, region_id)))
        .map_or(0, |job| job.status.load(Ordering::Acquire))
}

/// Records the undo pair for a finished analysis. 0 = never started.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_analysis_finalize(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    if host.is_null() {
        return false;
    }
    let Some(ops) = host_ops() else {
        return false;
    };
    let Ok(registry) = pitch_registry().lock() else {
        return false;
    };
    let Some(job) = registry
        .get(&(host_key(host), region_key(track_id, region_id)))
        .cloned()
    else {
        return false;
    };
    if job.status() != STATUS_READY {
        return false;
    }
    if RetainedTrack::identity(&ops, host, track_id) != job.identity {
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    }
    let Some(before) = job.before.lock().ok().and_then(|mut slot| slot.take()) else {
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    };
    let Some(after) = job.after.lock().ok().and_then(|mut slot| slot.take()) else {
        drop(before);
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    };
    let undo = [edit_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_NOTE_STORE,
        before.into_raw(),
    )];
    let redo = [edit_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_NOTE_STORE,
        after.into_raw(),
    )];
    let name = format!("Analyze Audio Pitch {track_id}:{region_id}");
    let label = CString::new(name).ok();
    if let Some(label) = label {
        record(
            &ops,
            host,
            label.as_bytes_with_nul(),
            &undo,
            &redo,
            false,
        );
    }
    job.status.store(STATUS_APPLIED, Ordering::Release);
    true
}

/// Drops a project's jobs without waiting for their workers. The workers keep
/// their own references alive and their results become unreachable.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_jobs_clear(host: *mut c_void) {
    let key = host_key(host);
    if let Ok(mut registry) = import_registry().lock() {
        registry.retain(|(registry_host, _), _| *registry_host != key);
    }
    if let Ok(mut registry) = pitch_registry().lock() {
        registry.retain(|(registry_host, _), _| *registry_host != key);
    }
}

/// Joins every worker before the engine is destroyed. No job may outlive the
/// host pointer it was created with.
#[no_mangle]
pub extern "C" fn hirari_region_jobs_shutdown() {
    let import_threads = match import_registry().lock() {
        Ok(mut registry) => registry
            .drain()
            .map(|(_, job)| job.thread.lock().ok().and_then(|mut slot| slot.take()))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    let pitch_threads = match pitch_registry().lock() {
        Ok(mut registry) => registry
            .drain()
            .map(|(_, job)| job.thread.lock().ok().and_then(|mut slot| slot.take()))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    for thread in import_threads.into_iter().chain(pitch_threads) {
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_key_packs_track_and_region() {
        assert_eq!(region_key(0, 0), 0);
        assert_eq!(region_key(1, 2), (1 << 32) | 2);
        assert_ne!(region_key(2, 1), region_key(1, 2));
    }

    #[test]
    fn sample_rate_bounds_match_native_import() {
        assert!(!valid_sample_rate(f64::NAN));
        assert!(!valid_sample_rate(f64::INFINITY));
        assert!(!valid_sample_rate(7_999.0));
        assert!(valid_sample_rate(8_000.0));
        assert!(valid_sample_rate(384_000.0));
        assert!(!valid_sample_rate(384_001.0));
    }
}
