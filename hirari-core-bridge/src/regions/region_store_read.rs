//! Region store read-side FFI: find, markers, edits, paths, listings and
//! note edits, split out of region_store.rs.
use super::*;

/// Copies a region's published clip-edit table into `out`, reporting the total
/// count either way. Reads the projection the callback sees, so a reader can
/// never observe a later edit.
pub(crate) fn read_projection<T: Copy>(
    tracks: &[TrackRegions],
    key: *mut c_void,
    region_id: u32,
    out: *mut T,
    capacity: usize,
    project: impl Fn(&RegionClipEditsView) -> (*const T, usize),
) -> usize {
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return 0;
    };
    let Some(region) = entry_index(track, region_id) else {
        return 0;
    };
    let view = match &track.current {
        Some(generation) => &generation.clips[region].view,
        None => return 0,
    };
    let (items, count) = project(view);
    if !out.is_null() && capacity >= count && count != 0 {
        unsafe { ptr::copy_nonoverlapping(items, out, count) };
    }
    count
}

/// Reads a region's editable state.
///
/// # Safety
/// `host` is ignored; the store is keyed by track id. `out` is writable.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_find(
    key: *mut c_void,
    region_id: u32,
    out: *mut HirariRegionState,
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
    match entry_index(track, region_id).and_then(|index| track.entries.get(index)) {
        Some(entry) => {
            unsafe { ptr::write(out, entry.state) };
            true
        }
        None => false,
    }
}

/// Copies a region's warp markers into `out`, reporting the total count.
///
/// # Safety
/// `out` is writable for `capacity` elements or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_warp_markers(
    key: *mut c_void,
    region_id: u32,
    out: *mut HirariWarpMarker,
    capacity: usize,
) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    read_projection(&tracks, key, region_id, out, capacity, |view| {
        (view.warp_markers, view.warp_marker_count)
    })
}

/// Copies a region's range edits into `out`, reporting the total count.
///
/// # Safety
/// `out` is writable for `capacity` elements or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_range_edits(
    key: *mut c_void,
    region_id: u32,
    out: *mut NativeRangeEdit,
    capacity: usize,
) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    read_projection(&tracks, key, region_id, out, capacity, |view| {
        (view.range_edits, view.range_edit_count)
    })
}

/// Copies a region's comp ranges into `out` and reports the managed flag.
///
/// # Safety
/// `out` is writable for `capacity` elements or null; `managed` is writable or
/// null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_comp_ranges(
    key: *mut c_void,
    region_id: u32,
    out: *mut NativeCompRange,
    capacity: usize,
    managed: *mut bool,
) -> usize {
    if !managed.is_null() {
        unsafe { ptr::write(managed, false) };
    }
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    if !managed.is_null() {
        if let Some(view) = tracks
            .iter()
            .find(|stored| stored.host == key)
            .and_then(|track| entry_index(track, region_id))
            .and_then(|region| {
                tracks
                    .iter()
                    .find(|stored| stored.host == key)?
                    .current
                    .as_ref()
                    .map(|generation| &generation.clips[region].view)
            })
        {
            unsafe { ptr::write(managed, view.comp_managed != 0) };
        }
    }
    read_projection(&tracks, key, region_id, out, capacity, |view| {
        (view.comp_ranges, view.comp_range_count)
    })
}

/// Copies a region's source path, reporting its byte length either way.
///
/// # Safety
/// `out` is writable for `capacity` bytes or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_region_path(
    key: *mut c_void,
    region_id: u32,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    let Some(entry) = tracks
        .iter()
        .find(|stored| stored.host == key)
        .and_then(|track| entry_index(track, region_id))
        .and_then(|index| {
            tracks
                .iter()
                .find(|stored| stored.host == key)?
                .entries
                .get(index)
        })
    else {
        return 0;
    };
    let bytes = &entry.path;
    if !out.is_null() && capacity >= bytes.len() && !bytes.is_empty() {
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

/// Lists a track's region ids. Called with the store key, so the id-addressed
/// entry point lives in the edit host.
///
/// # Safety
/// `out` is writable for `capacity` elements or null.
pub(crate) unsafe extern "C" fn hirari_region_store_list_track_regions(
    key: *mut c_void,
    out: *mut u32,
    capacity: usize,
) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return 0;
    };
    if !out.is_null() && capacity >= track.entries.len() {
        for (index, entry) in track.entries.iter().enumerate() {
            unsafe { ptr::write(out.add(index), entry.id) };
        }
    }
    track.entries.len()
}

/// Reports a track's region count.
#[no_mangle]
pub(crate) extern "C" fn hirari_region_store_count(key: *mut c_void) -> usize {
    let Ok(tracks) = store().lock() else {
        return 0;
    };
    tracks
        .iter()
        .find(|stored| stored.host == key)
        .map_or(0, |track| track.entries.len())
}

/// Reports a track's region generation and published snapshot identity.
///
/// # Safety
/// Both out-parameters are writable or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_track_token(
    key: *mut c_void,
    generation: *mut u64,
    snapshot: *mut usize,
) -> bool {
    let Ok(tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return false;
    };
    if !generation.is_null() {
        unsafe { ptr::write(generation, track.publications) };
    }
    if !snapshot.is_null() {
        unsafe {
            ptr::write(
                snapshot,
                track.current.as_ref().map_or(0, |generation| Arc::as_ptr(generation) as usize),
            )
        };
    }
    true
}

/// Lists the members of a sync group inside one engine. Track ids only name a
/// track within the engine that issued the query, so the candidates gathered
/// under the store lock are re-resolved against it before being reported.
///
/// # Safety
/// `out` is writable for `capacity` elements or null.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_list_sync_group(
    host: *mut c_void,
    group: u32,
    out: *mut HirariRegionGroupMember,
    capacity: usize,
) -> usize {
    let candidates = {
        let Ok(tracks) = store().lock() else {
            return 0;
        };
        tracks
            .iter()
            .flat_map(|track| {
                track.entries.iter().filter(move |entry| {
                    entry.state.sync_group == group && entry.state.sync_group != 0
                })
                .map(move |entry| (track.host, track.track_id, entry.id, entry.state))
            })
            .collect::<Vec<_>>()
    };
    let mut count = 0usize;
    for (key, track_id, region_id, state) in candidates {
        if crate::region_edit_host::store_key(host, track_id) != key {
            continue;
        }
        if !out.is_null() && count < capacity {
            unsafe {
                ptr::write(
                    out.add(count),
                    HirariRegionGroupMember {
                        track_id,
                        region_id,
                        start: state.start,
                        base_start: state.base_start,
                        locked: state.locked,
                    },
                )
            };
        }
        count += 1;
    }
    count
}

/// Retains a region's audio handle so a worker can read it after the region
/// moves on.
#[no_mangle]
pub(crate) extern "C" fn hirari_region_store_retain_audio(
    key: *mut c_void,
    region_id: u32,
) -> *mut c_void {
    let Ok(tracks) = store().lock() else {
        return ptr::null_mut();
    };
    let audio = match tracks.iter().find(|stored| stored.host == key) {
        Some(track) => match entry_index(track, region_id) {
            Some(index) => track.entries[index].audio,
            None => return ptr::null_mut(),
        },
        None => return ptr::null_mut(),
    };
    retain_audio(audio)
}

/// Copies a region's note table into an owned store for an analysis worker.
/// `track` is the raw `Track` pointer a retained handle refers to.
#[no_mangle]
pub(crate) extern "C" fn hirari_region_store_note_snapshot(
    key: *mut c_void,
    region_id: u32,
) -> *mut c_void {
    let Ok(tracks) = store().lock() else {
        return ptr::null_mut();
    };
    let track = match tracks.iter().find(|stored| stored.host == key) {
        Some(track) => track,
        None => return ptr::null_mut(),
    };
    let index = match entry_index(track, region_id) {
        Some(index) => index,
        None => return ptr::null_mut(),
    };
    unsafe { hirari_region_vari_audio_store_clone(&track.entries[index].notes) }.cast()
}

/// Runs a note-table mutation and republishes. Rejected while an F0 analysis
/// job owns the table, exactly as the previous host did.
///
/// # Safety
/// `mutate` is a live host callback and `context` stays valid for the call.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_region_store_note_edit(
    key: *mut c_void,
    region_id: u32,
    context: *mut c_void,
    mutate: HirariRegionNoteEditFn,
) -> bool {
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        return false;
    };
    if pitch_running(track.host) {
        return false;
    }
    let Some(index) = entry_index(track, region_id) else {
        return false;
    };
    invalidate_frozen(track.host);
    let store = ptr::from_mut(&mut track.entries[index].notes);
    if !mutate(context, store) {
        return false;
    }
    publish(track);
    true
}

/// Copies the whole region into an owned handle the undo history keeps.
#[no_mangle]
pub(crate) extern "C" fn hirari_region_store_snapshot_region(
    key: *mut c_void,
    region_id: u32,
) -> *mut c_void {
    let Ok(tracks) = store().lock() else {
        return ptr::null_mut();
    };
    let Some(track) = tracks.iter().find(|stored| stored.host == key) else {
        return ptr::null_mut();
    };
    match region_snapshot(track, region_id) {
        Some(snapshot) => Box::into_raw(Box::new(snapshot)).cast(),
        None => ptr::null_mut(),
    }
}
