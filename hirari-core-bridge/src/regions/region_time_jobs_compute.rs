//! Region align/quantize analysis kernels, split out of region_time_jobs_execute.rs.
use super::*;

pub(in crate::region_time_jobs) fn compute_alignment(
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

pub(in crate::region_time_jobs) fn compute_quantize(
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

pub(in crate::region_time_jobs) fn run_job(job: Arc<Job>) {
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

pub(in crate::region_time_jobs) fn spawn_job(job: &Arc<Job>) -> bool {
    let worker = Arc::clone(job);
    std::thread::Builder::new()
        .name("hirari-region-timing".to_owned())
        .spawn(move || run_job(worker))
        .is_ok()
}

pub(in crate::region_time_jobs) fn regions_alignment_available(ops: &HirariRegionHostOps) -> bool {
    ops.find_region.is_some()
        && ops.apply_edit.is_some()
        && crate::region_edit_host::can_record(ops)
        && ops.warp_markers.is_some()
        && ops.retain_audio.is_some()
        && ops.audio_view.is_some()
        && ops.release_audio.is_some()
        && ops.track_token.is_some()
}

pub(in crate::region_time_jobs) fn regions_quantize_available(ops: &HirariRegionHostOps) -> bool {
    regions_alignment_available(ops)
        && ops.list_tracks.is_some()
        && ops.list_regions.is_some()
        && ops.tempo_state.is_some()
        && ops.undo_state.is_some()
        && ops.sample_rate.is_some()
}
