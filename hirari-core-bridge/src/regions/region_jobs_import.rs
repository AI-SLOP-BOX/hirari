//! Import machinery for region jobs, split out of region_jobs.rs.
use super::*;

pub(crate) struct ImportJob {
    host: HostPtr,
    track_id: u32,
    identity: usize,
    start_sample: u64,
    path: Vec<u8>,
    status: AtomicU8,
    audio: Mutex<Option<AudioHandle>>,
    source_rate: Mutex<f64>,
    pub(crate) thread: Mutex<Option<JoinHandle<()>>>,
}

impl ImportJob {
    pub(crate) fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self.status(), STATUS_DECODING | STATUS_READY)
    }
}

type ImportRegistry = Mutex<HashMap<(usize, u64), Arc<ImportJob>>>;

pub(crate) fn import_registry() -> &'static ImportRegistry {
    static REGISTRY: OnceLock<ImportRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

pub(crate) fn valid_sample_rate(rate: f64) -> bool {
    rate.is_finite() && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate)
}

pub(crate) fn edit_payload(
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
        ..Default::default()
    }
}

/// Validates the rates, derives the region's timeline length from the decoded
/// frame count, asks the host to insert the region, then records exactly one
/// "Add Region" undo step around it. The audio handle is always consumed by the
/// host, whether or not the insert succeeds.
pub(crate) fn commit_import(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    identity: usize,
    path: &[u8],
    start_sample: u64,
    audio: AudioHandle,
    source_rate: f64,
) -> bool {
    let (Some(insert), Some(identity_fn), Some(sample_rate), Some(snapshot), Some(view)) = (
        ops.insert_region,
        ops.track_identity,
        ops.sample_rate,
        ops.snapshot_region,
        ops.audio_view,
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
    let mut samples = 0u64;
    let channels = unsafe { view(audio.raw(), ptr::null_mut(), 0, &mut samples) };
    if channels == 0 || samples == 0 {
        return false;
    }
    // Round up so a trailing partial source frame is never dropped.
    let length = ((samples as f64) * timeline_rate / source_rate).ceil();
    if !length.is_finite() || length < 1.0 || length >= u64::MAX as f64 {
        return false;
    }
    let new_region_id = crate::id_host::next_region_id(ops.list_tracks, ops.list_regions, host);
    if new_region_id == 0 {
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
            length as u64,
            new_region_id,
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

pub(crate) fn import_commit_job(job: &ImportJob) -> bool {
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
    let Some((audio, source_rate)) = AudioHandle::decode(&ops, path) else {
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
                let decoded = AudioHandle::decode(&ops, &worker.path);
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
