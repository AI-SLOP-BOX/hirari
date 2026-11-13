//! Region storage FFI utility layer: registry holding FFI glue needed by
//! the native C++ region store.
use super::*;

pub(crate) fn audio_dimensions(handle: *mut c_void) -> (u64, u32) {
    let Some(view) = host_ops().and_then(|ops| ops.audio_view) else {
        return (0, 0);
    };
    if handle.is_null() {
        return (0, 0);
    }
    let mut frames = 0u64;
    let channels = unsafe { view(handle, ptr::null_mut(), 0, &mut frames) };
    (frames, channels)
}

/// # Safety
/// `ops` is null or a live table of host callbacks.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_storage_host_install(
    ops: *const HirariRegionStorageHostOps,
) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { ptr::read(ops) })
    };
    DATA_INSTALLED.store(value.is_some(), std::sync::atomic::Ordering::Release);
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

/// True once the native engine has supplied the storage host table, so the
/// region edit host may route its region-data callbacks through this store.
pub fn region_data_installed() -> bool {
    DATA_INSTALLED.load(std::sync::atomic::Ordering::Acquire)
}

pub(crate) fn new_note_store() -> HirariRegionVariAudioStore {
    unsafe { *Box::from_raw(hirari_region_vari_audio_store_new()) }
}

pub(crate) fn new_clip_store() -> RegionClipEdits {
    unsafe { *Box::from_raw(hirari_region_clip_edits_create()) }
}

pub(crate) fn empty_history() -> *mut HirariRegionProcessingHistoryStore {
    unsafe { hirari_region_processing_history_new() }
}


/// Materializes a history into `(id, operation, parameter, enabled)` tuples for
/// an undo snapshot.
pub(crate) fn history_steps(store: *const HirariRegionProcessingHistoryStore) -> Vec<(u32, Vec<u8>, f32, bool)> {
    let mut steps = 0usize;
    let mut bytes = 0usize;
    unsafe { hirari_region_processing_history_sizes(store, &mut steps, &mut bytes) };
    if steps == 0 {
        return Vec::new();
    }
    let mut views = vec![HirariEventStepView::default(); steps];
    let mut operations = vec![0u8; bytes.max(1)];
    let mut written = 0usize;
    let mut used = 0usize;
    let ok = unsafe {
        hirari_region_processing_history_materialize(
            store,
            views.as_mut_ptr(),
            steps,
            operations.as_mut_ptr(),
            operations.len(),
            &mut written,
            &mut used,
        )
    };
    if !ok {
        return Vec::new();
    }
    views
        .iter()
        .take(written)
        .map(|view| {
            let start = view.operation_offset.min(operations.len());
            let end = (start + view.operation_len).min(operations.len());
            (view.id, operations[start..end].to_vec(), view.parameter, view.enabled)
        })
        .collect()
}

/// A region under construction, for callers that fill the edit tables through
/// the same Rust stores the finished region will own.
pub struct HirariRegionDraft {
    pub id: u32,
    pub path: Vec<u8>,
    pub name: Vec<u8>,
    pub state: HirariRegionState,
    pub clip: RegionClipEdits,
    pub notes: HirariRegionVariAudioStore,
    pub history: *mut HirariRegionProcessingHistoryStore,
}

/// # Safety
/// `path`/`name` are readable for their lengths, or null when the length is 0.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_draft_new(
    id: u32,
    path: *const u8,
    path_len: usize,
    name: *const u8,
    name_len: usize,
    state: *const HirariRegionState,
) -> *mut HirariRegionDraft {
    let draft = Box::new(HirariRegionDraft {
        id,
        path: bytes_of(path, path_len).to_vec(),
        name: bytes_of(name, name_len).to_vec(),
        state: if state.is_null() {
            HirariRegionState::default()
        } else {
            *state
        },
        clip: new_clip_store(),
        notes: new_note_store(),
        history: empty_history(),
    });
    Box::into_raw(draft)
}

pub(crate) fn bytes_of<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        return &[];
    }
    unsafe { slice::from_raw_parts(data, len) }
}

#[no_mangle]
pub extern "C" fn hirari_region_store_draft_clip(draft: *mut HirariRegionDraft) -> *mut RegionClipEdits {
    unsafe { draft.as_mut().map_or(ptr::null_mut(), |draft| &mut draft.clip) }
}

#[no_mangle]
pub extern "C" fn hirari_region_store_draft_notes(
    draft: *mut HirariRegionDraft,
) -> *mut HirariRegionVariAudioStore {
    unsafe { draft.as_mut().map_or(ptr::null_mut(), |draft| &mut draft.notes) }
}

#[no_mangle]
pub extern "C" fn hirari_region_store_draft_history(
    draft: *mut HirariRegionDraft,
) -> *mut HirariRegionProcessingHistoryStore {
    unsafe { draft.as_mut().map_or(ptr::null_mut(), |draft| draft.history) }
}

/// Adds a drafted region to a track. The draft and the audio handle are consumed
/// on every path. Returns false when the region is rejected.
///
/// # Safety
/// `draft` comes from `hirari_region_store_draft_new` and `audio` is an owned
/// handle from the host table, or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_draft_add(
    draft: *mut HirariRegionDraft,
    key: *mut c_void,
    audio: *mut c_void,
) -> bool {
    if draft.is_null() {
        return false;
    }
    let draft = unsafe { *Box::from_raw(draft) };
    let Ok(mut tracks) = store().lock() else {
        unsafe { hirari_region_processing_history_clear(draft.history) };
        release_audio(audio);
        return false;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        unsafe { hirari_region_processing_history_clear(draft.history) };
        release_audio(audio);
        return false;
    };
    let added = finish_insert(
        track,
        draft.id,
        &draft.path,
        &draft.name,
        draft.state,
        audio,
        draft.clip,
        draft.notes,
        draft.history,
    );
    if added {
        publish(track);
    }
    added
}

/// Drops a draft the caller never finished, releasing its edit tables.
///
/// # Safety
/// `draft` comes from `hirari_region_store_draft_new` and is not used again.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_draft_discard(draft: *mut HirariRegionDraft) {
    if draft.is_null() {
        return;
    }
    let draft = unsafe { *Box::from_raw(draft) };
    unsafe { hirari_region_processing_history_clear(draft.history) };
}

/// # Safety
/// `host` is the live `Track` the host callbacks receive.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_install(host: *mut c_void, track_id: u32) {
    let Ok(mut tracks) = store().lock() else {
        return;
    };
    if let Some(index) = track_position(&tracks, host) {
        let previous = tracks.remove(index);
        install_regions(previous.host, ptr::null(), 0, 0);
    }
    tracks.push(TrackRegions {
        track_id,
        host,
        entries: Vec::new(),
        sample_rate: 44_100.0,
        block_size: 0,
        publications: 0,
        current: None,
        retired: Vec::new(),
    });
    let track = match tracks.last_mut() {
        Some(track) => track,
        None => return,
    };
    publish(track);
}

/// Drops a track's table. The caller has quiesced the audio callback.
#[no_mangle]
pub extern "C" fn hirari_region_store_forget(key: *mut c_void) {
    let Ok(mut tracks) = store().lock() else {
        return;
    };
    let Some(index) = track_position(&tracks, key) else {
        return;
    };
    let track = tracks.remove(index);
    install_regions(track.host, ptr::null(), 0, 0);
}

/// Re-prepares the stretchers for a new rate and block size and republishes, so
/// a snapshot baked at the previous rate cannot keep rendering at it.
#[no_mangle]
pub extern "C" fn hirari_region_store_prepare(key: *mut c_void, sample_rate: f64, block_size: u32) -> bool {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || block_size == 0 {
        return false;
    }
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        return false;
    };
    track.sample_rate = sample_rate;
    track.block_size = block_size;
    publish(track);
    true
}

/// Frees every generation except the published one. Only safe once the audio
/// callback has been quiesced.
#[no_mangle]
pub extern "C" fn hirari_region_store_trim_retired(key: *mut c_void) {
    let Ok(mut tracks) = store().lock() else {
        return;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        return;
    };
    let keep = match track.current.clone() {
        Some(current) => current,
        None => {
            track.retired.clear();
            return;
        }
    };
    track.retired.retain(|generation| Arc::ptr_eq(generation, &keep));
}
