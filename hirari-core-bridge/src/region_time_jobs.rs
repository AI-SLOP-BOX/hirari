//! Audio-region timing jobs: align-to-reference and sync-group quantize.
//!
//! The analysis kernels already live in Rust (`region_alignment`,
//! `audio_quantizer`). This module owns everything around them: request
//! validation, the per-reference job registry, conflict rejection, worker
//! threads, staleness checks, the marker commit and its undo entry. The native
//! engine only retains immutable audio buffers, applies marker lists and
//! records the undo pair.
//!
//! Every entry point runs on the control thread. Worker threads only read
//! retained audio and then publish their marker lists back into the job.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio_quantizer::{
    hirari_audio_quantizer_build_group_warp_maps, hirari_audio_quantizer_group_warp_map_copy,
    hirari_audio_quantizer_group_warp_map_count, hirari_audio_quantizer_group_warp_maps_destroy,
    hirari_audio_quantizer_settings_validate, HirariAudioQuantizeChannelView,
    HirariAudioQuantizerTargetView, HirariAudioQuantizerTempoEvent,
};
use crate::region_alignment::{
    hirari_region_align_audio, hirari_region_alignment_target_ids_valid,
    HirariRegionAlignmentTargetId,
};
use crate::region_edit_host::{
    host_ops, record, warp_markers_of, HirariRegionEditPayload, HirariRegionHostOps,
    HirariRegionState, HIRARI_REGION_EDIT_MARKERS,
};
use crate::region_warp::HirariWarpMarker;
use crate::tempo::{
    hirari_tempo_map_copy_events, hirari_tempo_map_get_current_bpm,
    hirari_tempo_map_get_event_count,
};
use crate::undo_transaction::hirari_undo_manager_transaction_active;

const STATUS_ANALYZING: u8 = 1;
const STATUS_READY: u8 = 2;
const STATUS_APPLIED: u8 = 3;
const STATUS_FAILED: u8 = 4;

const MAX_ALIGNMENT_TARGETS: usize = 32;
const MIN_ALIGNMENT_SPAN: u64 = 1024;
const MAX_ALIGNMENT_SPAN: u64 = 16 * 1024 * 1024;
const MAX_QUANTIZE_SPAN: u64 = 16_777_216;
const MAX_CHANNELS: u32 = 32;
const MAX_ALIGNMENT_MARKERS: usize = 257;

fn region_span(state: &HirariRegionState) -> u64 {
    if state.source_length > 0 {
        state.source_length
    } else {
        state.len
    }
}

/// A retained, immutable view of a region's decoded audio. The pointers stay
/// valid while the owning `RetainedAudio` lives, because the host keeps the
/// decoded buffer alive through the handle.
struct AudioView {
    channels: Vec<*const f32>,
    samples: u64,
    channel_count: u32,
}

struct RetainedAudio {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

// The handle points at host-owned immutable audio; the worker only reads it.
unsafe impl Send for RetainedAudio {}
unsafe impl Sync for RetainedAudio {}

impl RetainedAudio {
    fn retain(
        ops: &HirariRegionHostOps,
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
    ) -> Option<Self> {
        let retain = ops.retain_audio?;
        let handle = unsafe { retain(host, track_id, region_id) };
        if handle.is_null() {
            return None;
        }
        Some(Self { ops: *ops, handle })
    }

    fn view(&self) -> Option<AudioView> {
        let view = self.ops.audio_view?;
        let mut samples = 0_u64;
        let channel_count = unsafe { view(self.handle, ptr::null_mut(), 0, &mut samples) };
        if channel_count == 0 || channel_count > MAX_CHANNELS || samples == 0 {
            return None;
        }
        let mut channels = vec![ptr::null(); channel_count as usize];
        let written = unsafe {
            view(
                self.handle,
                channels.as_mut_ptr(),
                channels.len(),
                &mut samples,
            )
        };
        if written != channel_count || channels.iter().any(|channel| channel.is_null()) {
            return None;
        }
        Some(AudioView {
            channels,
            samples,
            channel_count,
        })
    }
}

impl Drop for RetainedAudio {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

/// One region captured for a job: the state and undo markers are snapshots,
/// the audio handle keeps the buffers alive for as long as the job exists.
struct CapturedRegion {
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

enum JobKind {
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

struct Job {
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
type Registry = Mutex<HashMap<JobKey, Arc<Job>>>;

fn alignment_registry() -> &'static Registry {
    static JOBS: OnceLock<Registry> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn quantize_registry() -> &'static Registry {
    static JOBS: OnceLock<Registry> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn region_key(track_id: u32, region_id: u32) -> u64 {
    ((track_id as u64) << 32) | region_id as u64
}

fn lock_registry(
    registry: &'static Registry,
) -> std::sync::MutexGuard<'static, HashMap<JobKey, Arc<Job>>> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn drop_finished(jobs: &mut HashMap<JobKey, Arc<Job>>) {
    jobs.retain(|_, job| {
        let status = job.status();
        status != STATUS_APPLIED && status != STATUS_FAILED
    });
}

fn track_token(
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

fn find_state(
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

fn capture_region(
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

fn audio_matches(state: &HirariRegionState, view: &AudioView) -> bool {
    let span = region_span(state);
    span <= view.samples
        && state.source_offset <= view.samples
        && span <= view.samples - state.source_offset
        && (1..=MAX_CHANNELS).contains(&view.channel_count)
}

fn alignable(state: &HirariRegionState, view: &AudioView, expected_length: u64) -> bool {
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

fn quantizable_source(state: &HirariRegionState) -> bool {
    !state.locked
        && !state.reverse
        && state.loop_count == 1
        && (state.warp_ratio - 1.0).abs() <= 1.0e-5
}

fn markers_payload(
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
    }
}

fn apply_markers(
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

fn compute_alignment(
    reference: &CapturedRegion,
    targets: &[CapturedRegion],
) -> Option<Vec<Vec<HirariWarpMarker>>> {
    let reference_view = reference.audio.view()?;
    let reference_span = region_span(&reference.state);
    let mut after = Vec::with_capacity(targets.len());
    for target in targets {
        let view = target.audio.view()?;
        let span = region_span(&target.state);
        let mut markers = [HirariWarpMarker::default(); MAX_ALIGNMENT_MARKERS];
        let mut count = 0_usize;
        let aligned = unsafe {
            hirari_region_align_audio(
                reference_view.channels.as_ptr(),
                reference_view.channels.len(),
                reference.state.source_offset as usize,
                reference_span as usize,
                view.channels.as_ptr(),
                view.channels.len(),
                target.state.source_offset as usize,
                span as usize,
                target.state.len,
                markers.as_mut_ptr(),
                markers.len(),
                &mut count,
            )
        };
        if !aligned || count < 2 || count > markers.len() {
            return None;
        }
        after.push(markers[..count].to_vec());
    }
    Some(after)
}

fn compute_quantize(
    targets: &[CapturedRegion],
    timeline_span: u64,
    timeline_start: u64,
    grid_beats: f64,
    strength: f32,
    swing: f32,
    sample_rate: f64,
    base_tempo: f64,
    tempo_events: &[HirariAudioQuantizerTempoEvent],
) -> Option<Vec<Vec<HirariWarpMarker>>> {
    let mut channels = Vec::new();
    let mut views = Vec::with_capacity(targets.len());
    for target in targets {
        let view = target.audio.view()?;
        let span = region_span(&target.state);
        let rate = target.state.source_frames_per_timeline_frame();
        for channel in &view.channels {
            channels.push(HirariAudioQuantizeChannelView {
                samples: unsafe { channel.add(target.state.source_offset as usize) },
                span,
                source_frames_per_timeline_frame: rate,
            });
        }
        views.push(HirariAudioQuantizerTargetView {
            source_span: span,
            source_frames_per_timeline_frame: rate,
        });
    }
    let projected = unsafe {
        hirari_audio_quantizer_build_group_warp_maps(
            channels.as_ptr(),
            channels.len(),
            views.as_ptr(),
            views.len(),
            timeline_span,
            timeline_start,
            grid_beats,
            strength,
            swing,
            sample_rate,
            base_tempo,
            tempo_events.as_ptr(),
            tempo_events.len(),
        )
    };
    if projected.is_null() {
        return None;
    }
    let mut after = Vec::with_capacity(targets.len());
    let mut ok = true;
    for index in 0..targets.len() {
        let count = unsafe { hirari_audio_quantizer_group_warp_map_count(projected, index) };
        if count < 2 {
            ok = false;
            break;
        }
        let mut markers = vec![HirariWarpMarker::default(); count];
        if !unsafe {
            hirari_audio_quantizer_group_warp_map_copy(
                projected,
                index,
                markers.as_mut_ptr(),
                markers.len(),
            )
        } {
            ok = false;
            break;
        }
        after.push(markers);
    }
    unsafe { hirari_audio_quantizer_group_warp_maps_destroy(projected) };
    ok.then_some(after)
}

fn run_job(job: Arc<Job>) {
    let result = match &job.kind {
        JobKind::Align { reference, targets } => compute_alignment(reference, targets),
        JobKind::Quantize {
            targets,
            timeline_span,
            timeline_start,
            grid_beats,
            strength,
            swing,
            sample_rate,
            base_tempo,
            tempo_events,
        } => compute_quantize(
            targets,
            *timeline_span,
            *timeline_start,
            *grid_beats,
            *strength,
            *swing,
            *sample_rate,
            *base_tempo,
            tempo_events,
        ),
    };
    let mut slot = job
        .after
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match result {
        Some(after) => {
            *slot = Some(after);
            job.status.store(STATUS_READY, Ordering::Release);
        }
        None => job.status.store(STATUS_FAILED, Ordering::Release),
    }
}

fn spawn_job(job: &Arc<Job>) -> bool {
    let worker = Arc::clone(job);
    std::thread::Builder::new()
        .name("hirari-region-timing".to_owned())
        .spawn(move || run_job(worker))
        .is_ok()
}

fn regions_alignment_available(ops: &HirariRegionHostOps) -> bool {
    ops.find_region.is_some()
        && ops.apply_edit.is_some()
        && ops.record_undo.is_some()
        && ops.warp_markers.is_some()
        && ops.retain_audio.is_some()
        && ops.audio_view.is_some()
        && ops.release_audio.is_some()
        && ops.track_token.is_some()
}

fn regions_quantize_available(ops: &HirariRegionHostOps) -> bool {
    regions_alignment_available(ops)
        && ops.list_tracks.is_some()
        && ops.list_regions.is_some()
        && ops.tempo_state.is_some()
        && ops.undo_state.is_some()
        && ops.sample_rate.is_some()
}

fn align_with(
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

fn tempo_snapshot(state: *mut c_void) -> Vec<HirariAudioQuantizerTempoEvent> {
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

fn list_track_ids(ops: &HirariRegionHostOps, host: *mut c_void) -> Option<Vec<u32>> {
    let list = ops.list_tracks?;
    let count = unsafe { list(host, ptr::null_mut(), 0) };
    let mut ids = vec![0_u32; count];
    let written = unsafe { list(host, ids.as_mut_ptr(), ids.len()) };
    if written != count {
        return None;
    }
    Some(ids)
}

fn list_region_ids(
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

struct QuantizeCapture {
    /// Every region the job covers, in engine storage order. The selected
    /// region is one of them.
    targets: Vec<CapturedRegion>,
    source_index: usize,
}

/// Collects the regions a quantize request covers. The selected region is
/// always included; a non-zero sync group adds every other region in that
/// group. Any member that cannot be quantized rejects the whole request.
fn collect_quantize_targets(
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

fn quantize_with(
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

fn lookup_job(
    registry: &'static Registry,
    host: *mut c_void,
    key: u64,
) -> Option<Arc<Job>> {
    let jobs = lock_registry(registry);
    jobs.get(&(host as usize, key)).cloned()
}

fn status_of(registry: &'static Registry, host: *mut c_void, key: u64) -> u8 {
    lookup_job(registry, host, key).map_or(0, |job| job.status())
}

fn undo_locked(ops: &HirariRegionHostOps, host: *mut c_void) -> bool {
    let Some(state) = ops.undo_state.map(|f| unsafe { f(host) }) else {
        return false;
    };
    !state.is_null() && unsafe { hirari_undo_manager_transaction_active(state) }
}

fn finalize_common(
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

fn align_finalize(
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

fn quantize_finalize(
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

/// Starts an align-to-reference analysis. The targets are validated and
/// captured before a worker thread is allowed to run.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect, and
/// `targets` must point to `target_count` initialized entries.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_align_to_reference(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
    targets: *const HirariRegionAlignmentTargetId,
    target_count: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    if target_count != 0 && targets.is_null() {
        return false;
    }
    let targets: Vec<(u32, u32)> = if target_count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(targets, target_count) }
            .iter()
            .map(|target| (target.track_id, target.region_id))
            .collect()
    };
    align_with(&ops, host, reference_track, reference_region, &targets)
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_alignment_status(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
) -> u8 {
    status_of(
        alignment_registry(),
        host,
        region_key(reference_track, reference_region),
    )
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_finalize_alignment(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    align_finalize(&ops, host, reference_track, reference_region)
}

/// Starts a synchronized quantize. Returns false when the settings, the sync
/// group or any member region would make the result ambiguous.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_quantize_sync_group(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    strength: f32,
    grid_beats: f64,
    swing: f32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    quantize_with(&ops, host, track_id, region_id, strength, grid_beats, swing)
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_quantize_status(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> u8 {
    status_of(quantize_registry(), host, region_key(track_id, region_id))
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_finalize_quantize(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    quantize_finalize(&ops, host, track_id, region_id)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct FakeRegion {
        id: u32,
        state: HirariRegionState,
        markers: Vec<HirariWarpMarker>,
        samples: Vec<f32>,
    }

    struct FakeTrack {
        id: u32,
        generation: u64,
        snapshot: usize,
        regions: Vec<FakeRegion>,
    }

    #[derive(Default)]
    struct FakeInner {
        tracks: Vec<FakeTrack>,
        applied: Vec<(u32, u32, usize)>,
        undo_entries: Vec<(String, usize, usize)>,
    }

    struct FakeHost {
        inner: Mutex<FakeInner>,
        tempo: *mut c_void,
        sample_rate: f64,
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    impl FakeHost {
        fn inner(&self) -> std::sync::MutexGuard<'_, FakeInner> {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    fn host_of<'a>(host: *mut c_void) -> &'a FakeHost {
        unsafe { &*(host as *const FakeHost) }
    }

    unsafe extern "C" fn fake_find(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
        out: *mut HirariRegionState,
    ) -> bool {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id == region_id {
                    unsafe { out.write(region.state) };
                    return true;
                }
            }
        }
        false
    }

    fn payload_markers(payload: &HirariRegionEditPayload) -> Vec<HirariWarpMarker> {
        if payload.blob.is_null() || payload.blob_len == 0 {
            return Vec::new();
        }
        unsafe {
            std::slice::from_raw_parts(
                payload.blob.cast::<HirariWarpMarker>(),
                payload.blob_len / std::mem::size_of::<HirariWarpMarker>(),
            )
            .to_vec()
        }
    }

    unsafe extern "C" fn fake_apply(
        host: *mut c_void,
        payload: *const HirariRegionEditPayload,
    ) -> bool {
        let payload = unsafe { ptr::read(payload) };
        let host = host_of(host);
        let mut guard = host.inner();
        if payload.op != HIRARI_REGION_EDIT_MARKERS {
            return false;
        }
        let markers = payload_markers(&payload);
        let mut applied = None;
        for track in &mut guard.tracks {
            if track.id != payload.track_id {
                continue;
            }
            for region in &mut track.regions {
                if region.id == payload.region_id {
                    region.markers = markers;
                    applied = Some(region.markers.len());
                    break;
                }
            }
            break;
        }
        match applied {
            Some(count) => {
                guard.applied.push((payload.track_id, payload.region_id, count));
                true
            }
            None => false,
        }
    }

    unsafe extern "C" fn fake_record(
        _host: *mut c_void,
        name: *const std::ffi::c_char,
        undo: *const HirariRegionEditPayload,
        undo_count: usize,
        redo: *const HirariRegionEditPayload,
        redo_count: usize,
        _coalesce: bool,
        _perform: bool,
    ) -> bool {
        let name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let _ = unsafe { std::slice::from_raw_parts(undo, undo_count) };
        let _ = unsafe { std::slice::from_raw_parts(redo, redo_count) };
        let host = host_of(_host);
        host.inner().undo_entries.push((name, undo_count, redo_count));
        true
    }

    unsafe extern "C" fn fake_markers(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
        out: *mut HirariWarpMarker,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id != region_id {
                    continue;
                }
                let count = region.markers.len();
                if !out.is_null() && capacity >= count {
                    unsafe { ptr::copy_nonoverlapping(region.markers.as_ptr(), out, count) };
                }
                return count;
            }
        }
        0
    }

    struct FakeAudio {
        samples: Box<[f32]>,
    }

    unsafe extern "C" fn fake_retain_audio(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
    ) -> *mut c_void {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id == region_id {
                    let samples = region.samples.clone().into_boxed_slice();
                    return Box::into_raw(Box::new(FakeAudio { samples })).cast();
                }
            }
        }
        ptr::null_mut()
    }

    unsafe extern "C" fn fake_audio_view(
        handle: *mut c_void,
        out: *mut *const f32,
        capacity: usize,
        samples: *mut u64,
    ) -> u32 {
        if handle.is_null() {
            return 0;
        }
        let audio = unsafe { &*(handle as *const FakeAudio) };
        if !samples.is_null() {
            unsafe { *samples = audio.samples.len() as u64 };
        }
        if !out.is_null() && capacity >= 1 {
            unsafe { out.write(audio.samples.as_ptr()) };
        }
        1
    }

    unsafe extern "C" fn fake_release_audio(handle: *mut c_void) {
        if !handle.is_null() {
            drop(unsafe { Box::from_raw(handle as *mut FakeAudio) });
        }
    }

    unsafe extern "C" fn fake_track_token(
        host: *mut c_void,
        track_id: u32,
        generation: *mut u64,
        snapshot: *mut usize,
    ) -> bool {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            if !generation.is_null() {
                unsafe { *generation = track.generation };
            }
            if !snapshot.is_null() {
                unsafe { *snapshot = track.snapshot };
            }
            return true;
        }
        false
    }

    unsafe extern "C" fn fake_list_tracks(
        host: *mut c_void,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for (index, track) in guard.tracks.iter().enumerate() {
            if !out.is_null() && index < capacity {
                unsafe { out.add(index).write(track.id) };
            }
        }
        guard.tracks.len()
    }

    unsafe extern "C" fn fake_list_regions(
        host: *mut c_void,
        track_id: u32,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for (index, region) in track.regions.iter().enumerate() {
                if !out.is_null() && index < capacity {
                    unsafe { out.add(index).write(region.id) };
                }
            }
            return track.regions.len();
        }
        0
    }

    unsafe extern "C" fn fake_tempo(host: *mut c_void) -> *mut c_void {
        host_of(host).tempo
    }

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        ptr::null_mut()
    }

    unsafe extern "C" fn fake_sample_rate(host: *mut c_void) -> f64 {
        host_of(host).sample_rate
    }

    fn ops() -> HirariRegionHostOps {
        HirariRegionHostOps {
            find_region: Some(fake_find),
            apply_edit: Some(fake_apply),
            record_undo: Some(fake_record),
            warp_markers: Some(fake_markers),
            retain_audio: Some(fake_retain_audio),
            audio_view: Some(fake_audio_view),
            release_audio: Some(fake_release_audio),
            track_token: Some(fake_track_token),
            list_tracks: Some(fake_list_tracks),
            list_regions: Some(fake_list_regions),
            tempo_state: Some(fake_tempo),
            undo_state: Some(fake_undo_state),
            sample_rate: Some(fake_sample_rate),
            ..Default::default()
        }
    }

    /// Alternating loud/quiet blocks give the alignment and quantize kernels a
    /// stable envelope to follow.
    fn pattern(len: usize, period: usize) -> Vec<f32> {
        (0..len)
            .map(|index| if (index / period).is_multiple_of(2) { 0.1 } else { 0.8 })
            .collect()
    }

    fn state(start: u64, len: u64, group: u32) -> HirariRegionState {
        HirariRegionState {
            start,
            len,
            source_length: len,
            loop_count: 1,
            warp_ratio: 1.0,
            sync_group: group,
            source_sample_rate: 44_100.0,
            timeline_sample_rate: 44_100.0,
            ..Default::default()
        }
    }

    struct Fixture {
        host: *mut c_void,
        ops: HirariRegionHostOps,
    }

    impl Fixture {
        fn new(tracks: Vec<FakeTrack>) -> Self {
            let tempo = crate::tempo::hirari_tempo_map_create();
            unsafe {
                crate::tempo::hirari_tempo_map_clear(tempo, 44_100.0, 120.0);
            }
            Self {
                host: Box::into_raw(Box::new(FakeHost {
                    inner: Mutex::new(FakeInner {
                        tracks,
                        ..Default::default()
                    }),
                    tempo,
                    sample_rate: 44_100.0,
                }))
                .cast(),
                ops: ops(),
            }
        }

        fn inner(&self) -> std::sync::MutexGuard<'_, FakeInner> {
            host_of(self.host).inner()
        }

        fn wait(&self, registry: &'static Registry, key: u64) -> u8 {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let status = status_of(registry, self.host, key);
                if status == STATUS_READY || status == STATUS_FAILED || Instant::now() > deadline {
                    return status;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                crate::tempo::hirari_tempo_map_destroy(host_of(self.host).tempo);
            }
            // The host handle itself stays allocated: the process-global job
            // registries are keyed by host address, so a reused address could
            // expose a previous fixture's finished job.
        }
    }

    fn track(id: u32, regions: Vec<FakeRegion>) -> FakeTrack {
        FakeTrack {
            id,
            generation: 1,
            snapshot: id as usize * 1_000,
            regions,
        }
    }

    fn region(id: u32, state: HirariRegionState, samples: Vec<f32>) -> FakeRegion {
        FakeRegion {
            id,
            state,
            markers: Vec::new(),
            samples,
        }
    }

    #[test]
    fn alignment_applies_markers_to_every_target_and_records_one_undo_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![
                region(2, state(0, 8192, 0), pattern(8192, 512)),
                region(3, state(0, 8192, 0), pattern(8192, 512)),
            ],
        )]);
        let ops = fixture.ops;

        assert!(align_with(&ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(
            fixture.wait(alignment_registry(), region_key(1, 2)),
            STATUS_READY
        );
        assert!(align_finalize(&ops, fixture.host, 1, 2));

        let inner = fixture.inner();
        let applied = &inner.applied;
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, 1);
        assert_eq!(applied[0].1, 3);
        assert!(applied[0].2 >= 2);
        assert_eq!(inner.undo_entries.len(), 1);
        assert_eq!(inner.undo_entries[0].0, "Align Audio Regions to Reference");
        assert_eq!(inner.undo_entries[0].1, 1);
        assert_eq!(inner.undo_entries[0].2, 1);
        drop(inner);
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), STATUS_APPLIED);
    }

    #[test]
    fn alignment_rejects_targets_that_session_changed_before_finalize() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![
                region(2, state(0, 8192, 0), pattern(8192, 512)),
                region(3, state(0, 8192, 0), pattern(8192, 512)),
            ],
        )]);
        let ops = fixture.ops;

        assert!(align_with(&ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(
            fixture.wait(alignment_registry(), region_key(1, 2)),
            STATUS_READY
        );
        fixture.inner().tracks[0].generation += 1;

        assert!(!align_finalize(&ops, fixture.host, 1, 2));
        let inner = fixture.inner();
        assert!(inner.applied.is_empty());
        assert!(inner.undo_entries.is_empty());
        drop(inner);
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), STATUS_FAILED);
    }

    #[test]
    fn alignment_rejects_a_reference_that_already_has_markers() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mut reference = region(2, state(0, 8192, 0), pattern(8192, 512));
        reference.markers.push(HirariWarpMarker::default());
        let fixture = Fixture::new(vec![track(
            1,
            vec![reference, region(3, state(0, 8192, 0), pattern(8192, 512))],
        )]);

        assert!(!align_with(&fixture.ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), 0);
    }

    #[test]
    fn quantize_marks_every_sync_group_member_and_records_one_grouped_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(0, 8192, 7), pattern(8192, 512))]),
        ]);
        let ops = fixture.ops;

        assert!(quantize_with(&ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(fixture.wait(quantize_registry(), region_key(1, 2)), STATUS_READY);
        assert!(quantize_finalize(&ops, fixture.host, 1, 2));

        let inner = fixture.inner();
        assert_eq!(inner.applied.len(), 2);
        assert!(inner.applied.iter().all(|(_, _, count)| *count >= 2));
        assert_eq!(inner.undo_entries.len(), 1);
        assert_eq!(inner.undo_entries[0].0, "Quantize Synchronized Audio Regions");
        assert_eq!(inner.undo_entries[0].1, 2);
        assert_eq!(inner.undo_entries[0].2, 2);
        drop(inner);
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), STATUS_APPLIED);
    }

    #[test]
    fn quantize_rejects_a_sync_group_member_at_a_different_position() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(512, 8192, 7), pattern(8192, 512))]),
        ]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), 0);
    }

    #[test]
    fn quantize_rejects_a_locked_sync_group_member() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mut member = region(3, state(0, 8192, 7), pattern(8192, 512));
        member.state.locked = true;
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![member]),
        ]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert!(fixture.inner().applied.is_empty());
    }

    #[test]
    fn quantize_ignores_group_members_without_audio() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(0, 8192, 7), Vec::new())]),
        ]);
        let ops = fixture.ops;

        assert!(quantize_with(&ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(fixture.wait(quantize_registry(), region_key(1, 2)), STATUS_READY);
        assert!(quantize_finalize(&ops, fixture.host, 1, 2));
        assert_eq!(fixture.inner().applied.len(), 1);
    }

    #[test]
    fn quantize_rejects_settings_outside_the_shared_bounds() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![region(2, state(0, 8192, 7), pattern(8192, 512))],
        )]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.5, 0.25, 0.0));
        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.3, 0.0));
        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 2.0));
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), 0);
    }
}
