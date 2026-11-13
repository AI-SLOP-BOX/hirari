//! F0 pitch-analysis machinery for region jobs, split out of region_jobs.rs.
use super::*;

pub(crate) struct PitchJob {
    identity: usize,
    status: AtomicU8,
    track: RetainedTrack,
    before: Mutex<Option<NoteStore>>,
    after: Mutex<Option<NoteStore>>,
    pub(crate) thread: Mutex<Option<JoinHandle<()>>>,
}

impl PitchJob {
    pub(crate) fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }
}

type PitchRegistry = Mutex<HashMap<(usize, u64), Arc<PitchJob>>>;

pub(crate) fn pitch_registry() -> &'static PitchRegistry {
    static REGISTRY: OnceLock<PitchRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn region_key(track_id: u32, region_id: u32) -> u64 {
    (u64::from(track_id) << 32) | u64::from(region_id)
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
