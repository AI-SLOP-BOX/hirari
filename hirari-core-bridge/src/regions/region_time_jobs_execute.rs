//! Align/quantize job implementations — validation, capture, run, finalize.
use super::*;

/// the audio handle keeps the buffers alive for as long as the job exists.
pub(in crate::region_time_jobs) struct CapturedRegion {
    track_id: u32,
    region_id: u32,
    generation: u64,
    snapshot: usize,
    state: HirariRegionState,
    audio: Arc<RetainedAudio>,
    before: Vec<HirariWarpMarker>,
}

impl CapturedRegion {
    fn key(&self) -> (u32, u32) {
        (self.track_id, self.region_id)
    }
}

pub(in crate::region_time_jobs) enum JobKind {
    Align {
        reference: CapturedRegion,
        targets: Vec<CapturedRegion>,
    },
    Quantize {
        targets: Vec<CapturedRegion>,
        timeline_span: u64,
        timeline_start: u64,
        grid_beats: f64,
        strength: f32,
        swing: f32,
        sample_rate: f64,
        base_tempo: f64,
        tempo_events: Vec<HirariAudioQuantizerTempoEvent>,
    },
}

pub(in crate::region_time_jobs) struct Job {
    status: AtomicU8,
    kind: JobKind,
    after: Mutex<Option<Vec<Vec<HirariWarpMarker>>>>,
}

impl Job {
    fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }

    fn is_active(&self) -> bool {
        matches!(self.status(), STATUS_ANALYZING | STATUS_READY)
    }

    fn regions(&self) -> Vec<(u32, u32)> {
        match &self.kind {
            JobKind::Align { reference, targets } => std::iter::once(reference.key())
                .chain(targets.iter().map(CapturedRegion::key))
                .collect(),
            JobKind::Quantize { targets, .. } => {
                targets.iter().map(CapturedRegion::key).collect()
            }
        }
    }
}

type JobKey = (usize, u64);
pub(in crate::region_time_jobs) type Registry = Mutex<HashMap<JobKey, Arc<Job>>>;

pub(in crate::region_time_jobs) fn alignment_registry() -> &'static Registry {
    static JOBS: OnceLock<Registry> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::region_time_jobs) fn quantize_registry() -> &'static Registry {
    static JOBS: OnceLock<Registry> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::region_time_jobs) fn region_key(track_id: u32, region_id: u32) -> u64 {
    ((track_id as u64) << 32) | region_id as u64
}

pub(in crate::region_time_jobs) fn lock_registry(
    registry: &'static Registry,
) -> std::sync::MutexGuard<'static, HashMap<JobKey, Arc<Job>>> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(in crate::region_time_jobs) fn drop_finished(jobs: &mut HashMap<JobKey, Arc<Job>>) {
    jobs.retain(|_, job| {
        let status = job.status();
        status != STATUS_APPLIED && status != STATUS_FAILED
    });
}

pub(in crate::region_time_jobs) fn track_token(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
) -> Option<(u64, usize)> {
    let token = ops.track_token?;
    let mut generation = 0_u64;
    let mut snapshot = 0_usize;
    if unsafe { token(host, track_id, &mut generation, &mut snapshot) } {
        Some((generation, snapshot))
    } else {
        None
    }
}

pub(in crate::region_time_jobs) fn find_state(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<HirariRegionState> {
    let find = ops.find_region?;
    let mut state = HirariRegionState::default();
    if unsafe { find(host, track_id, region_id, &mut state) } {
        Some(state)
    } else {
        None
    }
}

pub(in crate::region_time_jobs) fn capture_region(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<CapturedRegion> {
    let state = find_state(ops, host, track_id, region_id)?;
    let audio = RetainedAudio::retain(ops, host, track_id, region_id)?;
    let (generation, snapshot) = track_token(ops, host, track_id)?;
    let before = warp_markers_of(ops, host, track_id, region_id)?;
    Some(CapturedRegion {
        track_id,
        region_id,
        generation,
        snapshot,
        state,
        audio: Arc::new(audio),
        before,
    })
}

pub(in crate::region_time_jobs) fn audio_matches(state: &HirariRegionState, view: &AudioView) -> bool {
    let span = region_span(state);
    span <= view.samples
        && state.source_offset <= view.samples
        && span <= view.samples - state.source_offset
        && (1..=MAX_CHANNELS).contains(&view.channel_count)
}

pub(in crate::region_time_jobs) fn alignable(state: &HirariRegionState, view: &AudioView, expected_length: u64) -> bool {
    let span = region_span(state);
    !state.locked
        && !state.reverse
        && state.loop_count == 1
        && (state.warp_ratio - 1.0).abs() <= 1.0e-5
        && span >= MIN_ALIGNMENT_SPAN
        && state.len == expected_length
        && span <= MAX_ALIGNMENT_SPAN
        && audio_matches(state, view)
}

pub(in crate::region_time_jobs) fn quantizable_source(state: &HirariRegionState) -> bool {
    !state.locked
        && !state.reverse
        && state.loop_count == 1
        && (state.warp_ratio - 1.0).abs() <= 1.0e-5
}

pub(in crate::region_time_jobs) fn markers_payload(
    region: &CapturedRegion,
    markers: &[HirariWarpMarker],
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id: region.track_id,
        region_id: region.region_id,
        op: HIRARI_REGION_EDIT_MARKERS,
        blob: if markers.is_empty() {
            ptr::null()
        } else {
            markers.as_ptr().cast()
        },
        blob_len: std::mem::size_of_val(markers),
        state: HirariRegionState::default(),
        ..Default::default()
    }
}

pub(in crate::region_time_jobs) fn apply_markers(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    region: &CapturedRegion,
    markers: &[HirariWarpMarker],
) -> bool {
    let Some(apply) = ops.apply_edit else {
        return false;
    };
    let payload = markers_payload(region, markers);
    unsafe { apply(host, &payload) }
}

#[path = "region_time_jobs_compute.rs"]
mod compute;
pub(in crate::region_time_jobs) use compute::*;

pub(in crate::region_time_jobs) fn align_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
    targets: &[(u32, u32)],
) -> bool {
    if !regions_alignment_available(ops)
        || targets.is_empty()
        || targets.len() > MAX_ALIGNMENT_TARGETS
    {
        return false;
    }
    let ids: Vec<HirariRegionAlignmentTargetId> = targets
        .iter()
        .map(|&(track_id, region_id)| HirariRegionAlignmentTargetId {
            track_id,
            region_id,
        })
        .collect();
    if !unsafe {
        hirari_region_alignment_target_ids_valid(
            reference_track,
            reference_region,
            ids.as_ptr(),
            ids.len(),
        )
    } {
        return false;
    }
    let Some(reference) = capture_region(ops, host, reference_track, reference_region) else {
        return false;
    };
    let Some(reference_view) = reference.audio.view() else {
        return false;
    };
    if !alignable(&reference.state, &reference_view, reference.state.len)
        || !reference.before.is_empty()
    {
        return false;
    }
    let mut captured = Vec::with_capacity(targets.len());
    for &(track_id, region_id) in targets {
        let Some(target) = capture_region(ops, host, track_id, region_id) else {
            return false;
        };
        let Some(view) = target.audio.view() else {
            return false;
        };
        if !alignable(&target.state, &view, reference.state.len) {
            return false;
        }
        captured.push(target);
    }

    let key = region_key(reference_track, reference_region);
    let host_key = host as usize;
    let requested: Vec<(u32, u32)> = std::iter::once((reference_track, reference_region))
        .chain(targets.iter().copied())
        .collect();
    let mut jobs = lock_registry(alignment_registry());
    drop_finished(&mut jobs);
    let mut active = 0;
    for (existing_key, job) in jobs.iter() {
        if !job.is_active() {
            continue;
        }
        active += 1;
        let conflicts = job
            .regions()
            .iter()
            .any(|region| requested.contains(region));
        if *existing_key == (host_key, key) || conflicts {
            return false;
        }
    }
    if active >= 2 {
        return false;
    }
    let job = Arc::new(Job {
        status: AtomicU8::new(STATUS_ANALYZING),
        kind: JobKind::Align {
            reference,
            targets: captured,
        },
        after: Mutex::new(None),
    });
    jobs.insert((host_key, key), Arc::clone(&job));
    drop(jobs);
    if spawn_job(&job) {
        return true;
    }
    lock_registry(alignment_registry()).remove(&(host_key, key));
    false
}

pub(in crate::region_time_jobs) fn tempo_snapshot(state: *mut c_void) -> Vec<HirariAudioQuantizerTempoEvent> {
    if state.is_null() {
        return Vec::new();
    }
    let mut events =
        vec![HirariAudioQuantizerTempoEvent::default(); unsafe { hirari_tempo_map_get_event_count(state) }];
    loop {
        let count =
            unsafe { hirari_tempo_map_copy_events(state, events.as_mut_ptr().cast(), events.len()) };
        if count <= events.len() {
            events.resize(count, HirariAudioQuantizerTempoEvent::default());
            return events;
        }
        events.resize(count, HirariAudioQuantizerTempoEvent::default());
    }
}

pub(in crate::region_time_jobs) fn list_track_ids(ops: &HirariRegionHostOps, host: *mut c_void) -> Option<Vec<u32>> {
    let list = ops.list_tracks?;
    let count = unsafe { list(host, ptr::null_mut(), 0) };
    let mut ids = vec![0_u32; count];
    let written = unsafe { list(host, ids.as_mut_ptr(), ids.len()) };
    if written != count {
        return None;
    }
    Some(ids)
}

pub(in crate::region_time_jobs) fn list_region_ids(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
) -> Option<Vec<u32>> {
    let list = ops.list_regions?;
    let count = unsafe { list(host, track_id, ptr::null_mut(), 0) };
    let mut ids = vec![0_u32; count];
    let written = unsafe { list(host, track_id, ids.as_mut_ptr(), ids.len()) };
    if written != count {
        return None;
    }
    Some(ids)
}

pub(in crate::region_time_jobs) struct QuantizeCapture {
    /// Every region the job covers, in engine storage order. The selected
    /// region is one of them.
    targets: Vec<CapturedRegion>,
    source_index: usize,
}

/// Collects the regions a quantize request covers. The selected region is
/// always included; a non-zero sync group adds every other region in that
/// group. Any member that cannot be quantized rejects the whole request.
pub(in crate::region_time_jobs) fn collect_quantize_targets(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<QuantizeCapture> {
    let source_state = find_state(ops, host, track_id, region_id)?;
    if !quantizable_source(&source_state) {
        return None;
    }
    let group = source_state.sync_group;
    let track_ids = list_track_ids(ops, host)?;
    let mut source_index = None;
    let mut targets = Vec::new();
    for candidate_track in track_ids {
        for candidate_region in list_region_ids(ops, host, candidate_track)? {
            let selected = candidate_track == track_id && candidate_region == region_id;
            if !selected {
                if group == 0 {
                    continue;
                }
                let state = find_state(ops, host, candidate_track, candidate_region)?;
                if state.sync_group != group {
                    continue;
                }
            }
            let Some(captured) =
                capture_region(ops, host, candidate_track, candidate_region)
            else {
                // A group region without decoded audio is outside the job, the
                // same way the engine used to skip it before validation.
                if selected {
                    return None;
                }
                continue;
            };
            let Some(view) = captured.audio.view() else {
                if selected {
                    return None;
                }
                continue;
            };
            if selected {
                if !audio_matches(&captured.state, &view) {
                    return None;
                }
                source_index = Some(targets.len());
            } else if !quantizable_source(&captured.state)
                || captured.state.start != source_state.start
                || captured.state.len != source_state.len
                || !audio_matches(&captured.state, &view)
            {
                return None;
            }
            targets.push(captured);
        }
    }
    Some(QuantizeCapture {
        targets,
        source_index: source_index?,
    })
}

pub(in crate::region_time_jobs) fn quantize_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    strength: f32,
    grid_beats: f64,
    swing: f32,
) -> bool {
    if !regions_quantize_available(ops)
        || !hirari_audio_quantizer_settings_validate(
            track_id, region_id, strength, grid_beats, swing,
        )
    {
        return false;
    }
    let Some(capture) = collect_quantize_targets(ops, host, track_id, region_id) else {
        return false;
    };
    let source_state = capture.targets[capture.source_index].state;
    let timeline_span = source_state.len;
    if capture.targets.is_empty()
        || !(2..=MAX_QUANTIZE_SPAN).contains(&timeline_span)
        || !source_state.source_frames_per_timeline_frame().is_finite()
        || source_state.source_frames_per_timeline_frame() <= 0.0
    {
        return false;
    }
    let mut total_channels = 0_usize;
    for target in &capture.targets {
        let Some(view) = target.audio.view() else {
            return false;
        };
        let span = region_span(&target.state);
        let rate = target.state.source_frames_per_timeline_frame();
        if span < 2
            || view.channel_count == 0
            || view.channel_count as usize > MAX_CHANNELS as usize - total_channels
            || !rate.is_finite()
            || rate <= 0.0
        {
            return false;
        }
        total_channels += view.channel_count as usize;
    }
    let sample_rate = ops.sample_rate.map_or(0.0, |f| unsafe { f(host) });
    let tempo_state = ops
        .tempo_state
        .map_or(ptr::null_mut(), |f| unsafe { f(host) });
    let base_tempo = unsafe { hirari_tempo_map_get_current_bpm(tempo_state) } as f64;
    let tempo_events = tempo_snapshot(tempo_state);
    let timeline_start = source_state.start;
    let key = region_key(track_id, region_id);
    let host_key = host as usize;
    let mut jobs = lock_registry(quantize_registry());
    drop_finished(&mut jobs);
    if jobs.values().any(|job| job.is_active()) {
        return false;
    }
    let job = Arc::new(Job {
        status: AtomicU8::new(STATUS_ANALYZING),
        kind: JobKind::Quantize {
            targets: capture.targets,
            timeline_span,
            timeline_start,
            grid_beats,
            strength,
            swing,
            sample_rate,
            base_tempo,
            tempo_events,
        },
        after: Mutex::new(None),
    });
    jobs.insert((host_key, key), Arc::clone(&job));
    drop(jobs);
    if spawn_job(&job) {
        return true;
    }
    // The entry stays, marked failed, until the next request reaps it.
    job.status.store(STATUS_FAILED, Ordering::Release);
    false
}

pub(in crate::region_time_jobs) fn lookup_job(
    registry: &'static Registry,
    host: *mut c_void,
    key: u64,
) -> Option<Arc<Job>> {
    let jobs = lock_registry(registry);
    jobs.get(&(host as usize, key)).cloned()
}

pub(in crate::region_time_jobs) fn status_of(registry: &'static Registry, host: *mut c_void, key: u64) -> u8 {
    lookup_job(registry, host, key).map_or(0, |job| job.status())
}

pub(in crate::region_time_jobs) fn undo_locked(ops: &HirariRegionHostOps, host: *mut c_void) -> bool {
    let Some(state) = ops.undo_state.map(|f| unsafe { f(host) }) else {
        return false;
    };
    !state.is_null() && unsafe { hirari_undo_manager_transaction_active(state) }
}

pub(in crate::region_time_jobs) fn finalize_common(
    job: &Job,
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    rollback_on_failure: bool,
) -> bool {
    if job.status() != STATUS_READY {
        return false;
    }
    if undo_locked(ops, host) {
        return false;
    }
    let after = {
        let slot = job
            .after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match slot.as_ref() {
            Some(after) => after.clone(),
            None => {
                job.status.store(STATUS_FAILED, Ordering::Release);
                return false;
            }
        }
    };
    let targets: &[CapturedRegion] = match &job.kind {
        JobKind::Align { targets, .. } => targets,
        JobKind::Quantize { targets, .. } => targets,
    };
    if after.len() != targets.len() {
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    }
    for target in targets {
        let Some((generation, snapshot)) = track_token(ops, host, target.track_id) else {
            job.status.store(STATUS_FAILED, Ordering::Release);
            return false;
        };
        let matches = match &job.kind {
            JobKind::Align { .. } => generation == target.generation,
            JobKind::Quantize { .. } => snapshot == target.snapshot,
        };
        if !matches {
            job.status.store(STATUS_FAILED, Ordering::Release);
            return false;
        }
    }
    if let JobKind::Align { reference, .. } = &job.kind {
        let Some((generation, _)) = track_token(ops, host, reference.track_id) else {
            job.status.store(STATUS_FAILED, Ordering::Release);
            return false;
        };
        if generation != reference.generation {
            job.status.store(STATUS_FAILED, Ordering::Release);
            return false;
        }
    }
    let mut applied = 0;
    for (index, target) in targets.iter().enumerate() {
        if !apply_markers(ops, host, target, &after[index]) {
            break;
        }
        applied += 1;
    }
    if applied != targets.len() {
        for index in (0..applied).rev() {
            let _ = apply_markers(ops, host, &targets[index], &targets[index].before);
        }
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    }
    let mut undo = Vec::with_capacity(targets.len());
    let mut redo = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        undo.push(markers_payload(target, &target.before));
        redo.push(markers_payload(target, &after[index]));
    }
    if !record(ops, host, name, &undo, &redo, false) {
        if rollback_on_failure {
            for target in targets {
                let _ = apply_markers(ops, host, target, &target.before);
            }
        }
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    }
    job.status.store(STATUS_APPLIED, Ordering::Release);
    true
}

pub(in crate::region_time_jobs) fn align_finalize(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(job) = lookup_job(alignment_registry(), host, region_key(track_id, region_id)) else {
        return false;
    };
    finalize_common(
        &job,
        ops,
        host,
        b"Align Audio Regions to Reference\0",
        true,
    )
}

pub(in crate::region_time_jobs) fn quantize_finalize(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(job) = lookup_job(quantize_registry(), host, region_key(track_id, region_id)) else {
        return false;
    };
    if job.status() != STATUS_READY {
        return false;
    }
    // A tempo or sample-rate change pins the previous map to a timeline the
    // project no longer has; the user has to re-run the quantize.
    if undo_locked(ops, host) {
        return false;
    }
    let JobKind::Quantize {
        base_tempo,
        sample_rate,
        tempo_events,
        ..
    } = &job.kind
    else {
        return false;
    };
    let Some(tempo_state) = ops.tempo_state.map(|f| unsafe { f(host) }) else {
        return false;
    };
    let current_bpm = unsafe { hirari_tempo_map_get_current_bpm(tempo_state) } as f64;
    let current_rate = ops.sample_rate.map_or(f64::NAN, |f| unsafe { f(host) });
    let current_events = tempo_snapshot(tempo_state);
    if (current_bpm - base_tempo).abs() > 1.0e-9
        || (current_rate - sample_rate).abs() > 1.0e-9
        || current_events.len() != tempo_events.len()
        || !current_events.iter().zip(tempo_events).all(|(left, right)| {
            left.sample_pos == right.sample_pos && left.bpm == right.bpm && left.ramp == right.ramp
        })
    {
        job.status.store(STATUS_FAILED, Ordering::Release);
        return false;
    }
    finalize_common(
        &job,
        ops,
        host,
        b"Quantize Synchronized Audio Regions\0",
        false,
    )
}
