//! Region store insert-side FFI: snapshot capture, clone/copy/insert, layout
//! header/string/timing, split out of region_store.rs.
use super::*;

pub(crate) fn region_snapshot(track: &TrackRegions, region_id: u32) -> Option<RegionSnapshot> {
    let index = entry_index(track, region_id)?;
    let entry = track.entries.get(index)?;
    Some(RegionSnapshot {
        path: entry.path.clone(),
        name: entry.name.clone(),
        state: entry.state,
        audio: retain_audio(entry.audio),
        clip: unsafe { hirari_region_clip_edits_clone(ptr::from_ref(&entry.clip)) },
        notes: unsafe { hirari_region_vari_audio_store_clone(ptr::from_ref(&entry.notes)) },
        history: history_steps(entry.history as *const _),
    })
}

/// # Safety
/// `handle` is null or a handle from `hirari_region_store_snapshot_region`.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_release_snapshot(
    handle: *mut c_void,
) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle.cast::<RegionSnapshot>()) });
}

/// Inserts a freshly decoded region under the id the caller allocated. The
/// audio handle is consumed on every path.
///
/// # Safety
/// `path` is readable for `path_length` bytes or null when the length is 0, and
/// `audio` is an owned handle from the host table.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_insert_region(
    key: *mut c_void,
    path: *const u8,
    path_length: usize,
    start_sample: u64,
    source_sample_rate: f64,
    timeline_sample_rate: f64,
    length: u64,
    region_id: u32,
    audio: *mut c_void,
) -> u32 {
    let Ok(mut tracks) = store().lock() else {
        release_audio(audio);
        return 0;
    };
    let Some(index) = track_position(&tracks, key) else {
        release_audio(audio);
        return 0;
    };
    if audio.is_null() {
        release_audio(audio);
        return 0;
    }
    let source = bytes_of(path, path_length);
    let path = source.to_vec();
    let name = path.clone();
    // Only the fields the old host callback filled in; the base geometry is
    // left at zero so `finish_insert` derives it from the decoded source.
    let state = HirariRegionState {
        start: start_sample,
        len: length,
        warp_ratio: 1.0,
        source_sample_rate,
        timeline_sample_rate,
        clip_gain: 1.0,
        loop_count: 1,
        ..Default::default()
    };
    let track = &mut tracks[index];
    if !finish_insert(
        track,
        region_id,
        &path,
        &name,
        state,
        audio,
        new_clip_store(),
        new_note_store(),
        empty_history(),
    ) {
        return 0;
    }
    publish(track);
    region_id
}

/// Clones a region under the id the caller allocated, optionally replacing its
/// editable state first. Returns the new id, or 0 when the source is gone.
///
/// # Safety
/// `state` is null or a live region state.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_clone_region(
    key: *mut c_void,
    region_id: u32,
    state: *const HirariRegionState,
    new_region_id: u32,
) -> u32 {
    let Ok(mut tracks) = store().lock() else {
        return 0;
    };
    let Some(track_position) = track_position(&tracks, key) else {
        return 0;
    };
    let mut snapshot = match region_snapshot(&tracks[track_position], region_id) {
        Some(snapshot) => snapshot,
        None => return 0,
    };
    if !state.is_null() {
        snapshot.state = unsafe { *state };
    }
    if !add_snapshot(&mut tracks[track_position], new_region_id, &snapshot) {
        return 0;
    }
    publish(&mut tracks[track_position]);
    new_region_id
}

/// Copies a track's whole region table onto another track under the ids the
/// caller allocated, in source order. Used when a track is duplicated.
///
/// # Safety
/// `region_ids` is readable for `count` elements or null when the count is 0.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_copy_regions(
    source: *mut c_void,
    destination: *mut c_void,
    region_ids: *const u32,
    count: usize,
) -> bool {
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let (Some(source), Some(destination)) = (
        track_position(&tracks, source),
        track_position(&tracks, destination),
    ) else {
        return false;
    };
    if source == destination || tracks[source].entries.len() != count || count != 0 && region_ids.is_null()
    {
        return false;
    }
    let mut snapshots = Vec::with_capacity(count);
    for index in 0..tracks[source].entries.len() {
        let id = tracks[source].entries[index].id;
        match region_snapshot(&tracks[source], id) {
            Some(snapshot) => snapshots.push((unsafe { *region_ids.add(index) }, snapshot)),
            None => return false,
        }
    }
    for (id, snapshot) in snapshots.iter() {
        if !add_snapshot(&mut tracks[destination], *id, snapshot) {
            return false;
        }
    }
    publish(&mut tracks[destination]);
    true
}

/// Adopts an owned snapshot as a new region. The snapshot keeps ownership of
/// its buffers so the undo history can replay it.
pub(crate) fn add_snapshot(track: &mut TrackRegions, region_id: u32, snapshot: &RegionSnapshot) -> bool {
    let history = empty_history();
    for (id, operation, parameter, enabled) in snapshot.history.iter() {
        let step = HirariEventStep {
            id: *id,
            operation: operation.as_ptr(),
            operation_len: operation.len(),
            parameter: *parameter,
            enabled: *enabled,
        };
        unsafe { hirari_region_processing_history_append(history, step) };
    }
    finish_insert(
        track,
        region_id,
        &snapshot.path,
        &snapshot.name,
        snapshot.state,
        retain_audio(snapshot.audio),
        unsafe { *Box::from_raw(hirari_region_clip_edits_clone(snapshot.clip as *const _)) },
        unsafe { *Box::from_raw(hirari_region_vari_audio_store_clone(snapshot.notes as *const _)) },
        history,
    )
}

/// Fills the fields the layout serializer needs for the region at `index`.
/// # Safety
/// `out` is writable.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_layout_header(
    key: *mut c_void,
    index: usize,
    out: *mut crate::layout_gather::HirariLayoutRegionHeader,
) -> bool {
    if out.is_null() {
        return false;
    }
    let Ok(tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return false;
    };
    let Some(entry) = track.entries.get(index) else {
        return false;
    };
    let (view, frozen_notes) = match track.current.as_ref() {
        Some(generation) => (
            ptr::from_ref(&generation.clips[index].view),
            ptr::from_ref(&*generation.notes[index]),
        ),
        None => (ptr::null(), ptr::null()),
    };
    unsafe {
        ptr::write(
            out,
            crate::layout_gather::HirariLayoutRegionHeader {
                id: entry.id,
                state: entry.state,
                source_sample_rate: entry.state.source_sample_rate,
                audio_loaded: !entry.audio.is_null(),
                warp_markers: if view.is_null() { ptr::null() } else { (*view).warp_markers },
                warp_marker_count: if view.is_null() {
                    0
                } else {
                    (*view).warp_marker_count
                },
                range_edits: if view.is_null() { ptr::null() } else { (*view).range_edits },
                range_edit_count: if view.is_null() { 0 } else { (*view).range_edit_count },
                note_store: ptr::from_ref(&entry.notes),
                note_frozen: frozen_notes,
                processing_store: entry.history as *const _,
            },
        )
    };
    true
}

/// Copies the name (kind 0) or source path (kind 1) of the region at `index`.
/// # Safety
/// `out` is writable for `capacity` bytes or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_layout_string(
    key: *mut c_void,
    index: usize,
    kind: u32,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    let Some(entry) = tracks
        .iter()
        .find(|stored| stored.host == key)
        .and_then(|track| track.entries.get(index))
    else {
        return 0;
    };
    let bytes = if kind == 0 { &entry.name } else { &entry.path };
    if !out.is_null() && capacity >= bytes.len() && !bytes.is_empty() {
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

/// Reports the last timeline sample the track's regions reach, or false when a
/// region would overflow.
/// # Safety
/// `out` is writable or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_project_end(key: *mut c_void, out: *mut u64) -> bool {
    let Ok(tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return false;
    };
    let mut end = 0u64;
    for entry in track.entries.iter() {
        if entry.state.len == 0
            || entry.state.start > u64::MAX - entry.state.len
        {
            return false;
        }
        end = end.max(entry.state.start + entry.state.len);
    }
    if !out.is_null() {
        unsafe { ptr::write(out, end) };
    }
    true
}

/// Reports the region at `index`'s bounce span. Returns false past the end of
/// the table.
/// # Safety
/// Every out-parameter is writable or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_span_at(
    key: *mut c_void,
    index: usize,
    start: *mut u64,
    length: *mut u64,
    loop_count: *mut u32,
) -> bool {
    let Ok(tracks) = store().lock() else {
        return false;
    };
    let Some(entry) = tracks
        .iter()
        .find(|stored| stored.host == key)
        .and_then(|track| track.entries.get(index))
    else {
        return false;
    };
    unsafe {
        if !start.is_null() {
            ptr::write(start, entry.state.start);
        }
        if !length.is_null() {
            ptr::write(length, entry.state.len);
        }
        if !loop_count.is_null() {
            ptr::write(loop_count, entry.state.loop_count);
        }
    }
    true
}

/// Drops every region's cached peaks; used when a track leaves the project.
#[no_mangle]
pub(crate) extern "C" fn hirari_region_store_forget_waveforms(host: *mut c_void, key: *mut c_void) {
    let Ok(tracks) = store().lock() else {
        return;
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return;
    };
    for entry in track.entries.iter() {
        unsafe { hirari_region_waveform_forget(host, entry.id) };
    }
}

/// Applies one decided edit. Everything the payload can ask for is a region
/// write, so the whole switch lives here.
///
/// # Safety
/// `payload` is a live payload whose `blob` matches its op.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_apply_edit(
    host: *mut c_void,
    key: *mut c_void,
    payload: *const HirariRegionEditPayload,
) -> bool {
    let Some(payload) = (unsafe { payload.as_ref() }) else {
        return false;
    };
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let Some(track_position) = track_position(&tracks, key) else {
        return false;
    };
    let applied = apply_one(&mut tracks[track_position], payload);
    if applied && payload.op != HIRARI_REGION_EDIT_COMP_RANGES {
        // The comp-range setter republished without dropping the cached peaks;
        // keep that path unchanged.
        for entry in tracks[track_position].entries.iter() {
            if entry.id != payload.region_id {
                continue;
            }
            unsafe { hirari_region_waveform_forget(host, payload.region_id) };
        }
    }
    applied
}
