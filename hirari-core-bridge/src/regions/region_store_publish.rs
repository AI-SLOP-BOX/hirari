//! Region store bookkeeping: track position, entry index, insert
//! finalisation and the publish step, split out of region_store.rs.
use super::*;

pub(crate) fn track_position(tracks: &[TrackRegions], key: *mut c_void) -> Option<usize> {
    tracks.iter().position(|track| track.host == key)
}

pub(crate) fn entry_index(track: &TrackRegions, region_id: u32) -> Option<usize> {
    track.entries.iter().position(|entry| entry.id == region_id)
}

/// Runs the insert normalization for one region: metadata from the decoded
/// buffer, then the capped edit tables, then a sorted insert by timeline start.
/// Publication is the caller's, so a batch of adds can publish once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_insert(
    track: &mut TrackRegions,
    id: u32,
    path: &[u8],
    name: &[u8],
    state: HirariRegionState,
    audio: *mut c_void,
    mut clip: RegionClipEdits,
    mut notes: HirariRegionVariAudioStore,
    history: *mut HirariRegionProcessingHistoryStore,
) -> bool {
    let (samples, channels) = audio_dimensions(audio);
    let mut state = state;
    if !unsafe {
        hirari_region_normalize_add_metadata(&mut state, !audio.is_null(), samples, channels)
    } {
        unsafe { hirari_region_processing_history_clear(history) };
        return false;
    }
    let warp_span = if state.source_length > 0 {
        state.source_length
    } else {
        state.len
    };
    unsafe {
        hirari_region_clip_edits_normalize_warp_markers(&mut clip, warp_span, state.len);
        hirari_region_clip_edits_normalize_range_edits(&mut clip, state.len);
        hirari_region_vari_audio_store_normalize(&mut notes);
    }
    let entry = RegionEntry {
        id,
        path: path.to_vec(),
        name: name.to_vec(),
        state,
        audio,
        clip,
        notes,
        history,
        stretch: None,
    };
    let position = track
        .entries
        .binary_search_by_key(&entry.state.start, |region| region.state.start)
        .unwrap_or_else(|position| position);
    track.entries.insert(position, entry);
    true
}

/// Drops the stretchers, then gives every pitch-preserving region its own
/// instance prepared for the current rate and block size. Streaming processors
/// are mutable, so a retired generation must not advance a newer one's state.
pub(crate) fn rebuild_stretchers(track: &mut TrackRegions) {
    for entry in track.entries.iter_mut() {
        entry.stretch = None;
        if !entry.state.pitch_preserve_warp {
            continue;
        }
        let handle = StretchHandle(hirari_region_stretch_create());
        unsafe {
            hirari_region_stretch_prepare(handle.0, track.sample_rate, track.block_size.max(512));
        }
        entry.stretch = Some(handle);
    }
}

/// Builds the generation the callback reads and installs it on the track.
pub(crate) fn publish(track: &mut TrackRegions) {
    track.publications += 1;
    rebuild_stretchers(track);
    let mut clips: Vec<Box<RegionClipEditsFrozen>> = Vec::with_capacity(track.entries.len());
    let mut notes: Vec<Box<HirariVariAudioFrozen>> = Vec::with_capacity(track.entries.len());
    for entry in track.entries.iter() {
        clips.push(unsafe {
            Box::from_raw(hirari_region_clip_edits_freeze(ptr::from_ref(&entry.clip)))
        });
        notes.push(unsafe {
            Box::from_raw(hirari_region_vari_audio_freeze(ptr::from_ref(&entry.notes)))
        });
    }
    let mut sources: Vec<HirariRegionRenderSource> = Vec::with_capacity(track.entries.len());
    for (index, entry) in track.entries.iter().enumerate() {
        let mut left = ptr::null();
        let mut right = ptr::null();
        let mut samples = 0u64;
        audio_view(entry.audio, &mut left, &mut right, &mut samples);
        let frozen_notes = &*notes[index];
        let mut range_count = 0usize;
        let note_ranges = unsafe { hirari_region_vari_audio_frozen_ranges(frozen_notes, &mut range_count) };
        let mut curve_count = 0usize;
        let note_curves = unsafe { hirari_region_vari_audio_frozen_curves(frozen_notes, &mut curve_count) };
        sources.push(HirariRegionRenderSource {
            state: entry.state,
            id: entry.id,
            muted: u8::from(entry.state.muted),
            audio_left: left,
            audio_right: right,
            audio_samples: samples,
            clip: ptr::from_ref(&(*clips[index]).view),
            note_ranges,
            note_range_count: range_count,
            note_curves,
            note_curve_count: curve_count,
            note_cache_segments: ptr::from_ref(&*notes[index]) as usize,
            stretch_handle: entry.stretch.as_ref().map_or(ptr::null_mut(), |handle| handle.0),
            crossfade_in: 0,
            crossfade_out: 0,
        });
    }
    let capacity = track.entries.len().max(1);
    let layout = match Layout::array::<HirariTrackRegionRenderInput>(capacity) {
        Ok(layout) => layout,
        Err(_) => {
            track.publications -= 1;
            return;
        }
    };
    let buffer = unsafe { alloc_zeroed(layout).cast::<HirariTrackRegionRenderInput>() };
    if buffer.is_null() {
        track.publications -= 1;
        return;
    }
    let count = sources.len();
    let written = unsafe {
        hirari_track_build_region_render_inputs(
            sources.as_ptr(),
            count,
            track.sample_rate,
            hirari_region_resampler_prepare(),
            wsola_window().as_ptr(),
            track.host as usize,
            track.publications as usize,
            buffer,
            count,
        )
    };
    if written != count {
        unsafe { dealloc(buffer.cast(), layout) };
        track.publications -= 1;
        return;
    }
    let generation = Arc::new(Generation {
        inputs: buffer,
        length: count,
        clips,
        notes,
    });
    if let Some(previous) = track.current.replace(generation.clone()) {
        track.retired.push(previous);
    }
    install_regions(track.host, generation.inputs, generation.length, 1);
    mark_layout(track.host);
}
