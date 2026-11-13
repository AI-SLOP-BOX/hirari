//! Audio replacement entry points: rate detection, snapshot save/restore,
//! and replacing a region's audio with an external file, split out of
//! region_edit_host.rs.
use super::*;

pub(crate) fn audio_rate_supported(rate: f64) -> bool {
    rate.is_finite() && (8_000.0..=384_000.0).contains(&rate)
}

pub(crate) fn restore_payload(
    track_id: u32,
    region_id: u32,
    snapshot: *mut c_void,
) -> HirariRegionEditPayload {
    let mut payload = state_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_RESTORE,
        HirariRegionState::default(),
    );
    payload.blob = snapshot as *const u8;
    payload
}

/// Decodes `path` and swaps it into the region, recording one undo entry.
///
/// The native setter rejected an empty path, an unreadable file and rates
/// outside 8 kHz..384 kHz, and snapshotted the region before and after the
/// swap; all of that lives here now. The host still performs the swap and
/// publishes the snapshot.
pub(crate) fn replace_audio_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    path: &[u8],
) -> bool {
    if path.is_empty() {
        return false;
    }
    let (Some(find), Some(apply), Some(snapshot)) = (
        ops.find_region,
        ops.apply_edit,
        ops.snapshot_region,
    ) else {
        return false;
    };
    let Some(read_rate) = ops.sample_rate else {
        return false;
    };
    // Decode before the lookup, as the native setter did, so an unreadable
    // file fails the same way whether or not the region exists.
    let Some((handle, source_sample_rate, _, decoded_samples)) =
        crate::audio_import_cache::import_audio_file(path)
    else {
        return false;
    };
    let audio = DecodedAudio { ops: *ops, handle };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let timeline_sample_rate = unsafe { read_rate(host) };
    if !audio_rate_supported(source_sample_rate) || !audio_rate_supported(timeline_sample_rate) {
        return false;
    }
    let before = unsafe { snapshot(host, track_id, region_id) };
    if before.is_null() {
        return false;
    }
    let release_snapshot = |handle: *mut c_void| {
        if let Some(release) = ops.release_snapshot {
            unsafe { release(host, handle) };
        }
    };
    // The decoded length and the source/timeline mapping give the replacement
    // its timeline geometry, so the native setter only stores what is resolved
    // here. Rates are installed first: the mapping is derived from them.
    state.source_sample_rate = source_sample_rate;
    state.timeline_sample_rate = timeline_sample_rate;
    let frames_per_timeline = state.source_frames_per_timeline_frame();
    let length = (decoded_samples as f64 / frames_per_timeline).ceil();
    if !length.is_finite() || length < 1.0 || length >= u64::MAX as f64 {
        return false;
    }
    state.source_offset = 0;
    state.base_source_offset = 0;
    state.source_length = decoded_samples;
    state.base_start = state.start;
    state.base_length = decoded_samples;
    state.len = length as u64;
    let mut blob = Vec::with_capacity(REPLACE_AUDIO_HEADER_BYTES + path.len());
    blob.extend_from_slice(&(audio.into_raw() as usize).to_ne_bytes());
    blob.extend_from_slice(path);
    let payload = HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_REPLACE_AUDIO,
        blob: blob.as_ptr(),
        blob_len: blob.len(),
        state,
        ..Default::default()
    };
    if !unsafe { apply(host, &payload) } {
        release_snapshot(before);
        return false;
    }
    let after = unsafe { snapshot(host, track_id, region_id) };
    if after.is_null() {
        release_snapshot(before);
        return false;
    }
    if can_record(ops) {
        // Both handles move to the undo entry.
        record(
            ops,
            host,
            b"Replace Region Audio\0",
            &[restore_payload(track_id, region_id, before)],
            &[restore_payload(track_id, region_id, after)],
            false,
        );
    } else {
        release_snapshot(before);
        release_snapshot(after);
    }
    true
}

/// Replaces a region's audio from a file path.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, the
/// installed table must outlive the call, and `path` must be readable for
/// `path_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_replace_audio(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    if path.is_null() && path_length != 0 {
        return false;
    }
    let path = if path_length == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(path, path_length) }
    };
    replace_audio_with(&ops, host, track_id, region_id, path)
}
