//! The engine's region table, owned by Rust.
//!
//! A track's regions — editable state, source path and name, the decoded-audio
//! handle, the clip-edit/note/processing tables, the spectral stretcher and the
//! immutable projection the audio callback reads — live here. The C++ `Track`
//! keeps only the published render-input array, its length and a published
//! flag, so the realtime callback never touches a region object.
//!
//! C++ supplies the host table below: it answers for the decoded buffer it
//! owns, installs a published array into the track's atomics, invalidates the
//! rendered freeze and sets the layout dirty bit. Region normalization on
//! insert, edit storage, snapshot publication and retirement are decided here.
//!
//! Locking: the store lock is the innermost lock. A host callback may run while
//! it is held, so the host side must not take the project topology lock in the
//! callbacks below.

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio_pitch_analysis::{
    hirari_audio_pitch_commit_region_result, hirari_audio_pitch_result_destroy,
};
use crate::region_audio::hirari_region_waveform_forget;
use crate::region_clip_edits::{
    hirari_region_clip_edits_clone, hirari_region_clip_edits_create,
    hirari_region_clip_edits_destroy, hirari_region_clip_edits_freeze,
    hirari_region_clip_edits_normalize_range_edits,
    hirari_region_clip_edits_normalize_warp_markers, hirari_region_clip_edits_replace_comp_ranges,
    hirari_region_clip_edits_replace_range_edits, hirari_region_clip_edits_set_warp_markers,
    RegionClipEdits, RegionClipEditsFrozen, RegionClipEditsView,
};
use crate::region_comp_ranges::NativeCompRange;
use crate::region_edit_host::{
    hirari_region_note_store_restore, HirariRegionEditPayload, HirariRegionGroupMember,
    HirariRegionNoteEditFn, HirariRegionState, HIRARI_REGION_EDIT_COMP_RANGES,
    HIRARI_REGION_EDIT_GEOMETRY, HIRARI_REGION_EDIT_MARKERS, HIRARI_REGION_EDIT_NOTE_STORE,
    HIRARI_REGION_EDIT_PITCH_COMMIT, HIRARI_REGION_EDIT_RANGE_EDITS, HIRARI_REGION_EDIT_REMOVE,
    HIRARI_REGION_EDIT_REPLACE_AUDIO, HIRARI_REGION_EDIT_RESTORE, HIRARI_REGION_EDIT_STATE,
};
use crate::region_processing::{hirari_region_normalize_add_metadata, NativeRangeEdit};
use crate::region_processing_history::{
    hirari_region_processing_history_append, hirari_region_processing_history_clear,
    hirari_region_processing_history_count, hirari_region_processing_history_materialize,
    hirari_region_processing_history_new, hirari_region_processing_history_reset,
    hirari_region_processing_history_sizes, HirariEventStep, HirariEventStepView,
    HirariRegionProcessingHistoryStore,
};
use crate::region_resampler::hirari_region_resampler_prepare;
use crate::region_time_stretch::{
    hirari_region_stretch_create, hirari_region_stretch_destroy, hirari_region_stretch_prepare,
};
use crate::region_warp::HirariWarpMarker;
use crate::track_region_render::HirariTrackRegionRenderInput;
use crate::track_region_snapshot::{
    hirari_track_build_region_render_inputs, HirariRegionRenderSource,
};
use crate::vari_audio::{
    hirari_region_vari_audio_freeze, hirari_region_vari_audio_frozen_curves,
    hirari_region_vari_audio_frozen_ranges, hirari_region_vari_audio_store_clear,
    hirari_region_vari_audio_store_clone, hirari_region_vari_audio_store_new,
    hirari_region_vari_audio_store_normalize, HirariRegionVariAudioStore, HirariVariAudioFrozen,
};

/// Matches the C++ WSOLA window the renderer was built with: 1024 raised-cosine
/// taps, constructed once before any callback can run.
const WSOLA_WINDOW_TAPS: usize = 1024;

fn wsola_window() -> &'static [f32; WSOLA_WINDOW_TAPS] {
    static WINDOW: OnceLock<[f32; WSOLA_WINDOW_TAPS]> = OnceLock::new();
    WINDOW.get_or_init(|| {
        let mut window = [0.0f32; WSOLA_WINDOW_TAPS];
        let last = (WSOLA_WINDOW_TAPS - 1) as f32;
        for (index, tap) in window.iter_mut().enumerate() {
            *tap = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * index as f32 / last).cos());
        }
        window
    })
}

/// The byte size of the owned audio handle that leads a replace-audio blob.
const HANDLE_BYTES: usize = std::mem::size_of::<usize>();

struct StretchHandle(*mut c_void);

impl Drop for StretchHandle {
    fn drop(&mut self) {
        unsafe { hirari_region_stretch_destroy(self.0) };
    }
}

/// An owned region copy handed to the undo history. Audio is retained rather
/// than shared, so the copy stays valid after the live region moves on.
struct RegionSnapshot {
    path: Vec<u8>,
    name: Vec<u8>,
    state: HirariRegionState,
    audio: *mut c_void,
    clip: *mut RegionClipEdits,
    notes: *mut HirariRegionVariAudioStore,
    history: Vec<(u32, Vec<u8>, f32, bool)>,
}

impl Drop for RegionSnapshot {
    fn drop(&mut self) {
        unsafe {
            hirari_region_clip_edits_destroy(self.clip);
            hirari_region_vari_audio_store_clear(self.notes);
        }
        release_audio(self.audio);
    }
}

/// One region as the store holds it. `audio` is the C++ handle for the decoded
/// buffer: the store releases it through the host table and never reads it.
/// The tables are mutated only while the store lock is held.
struct RegionEntry {
    id: u32,
    path: Vec<u8>,
    name: Vec<u8>,
    state: HirariRegionState,
    audio: *mut c_void,
    clip: RegionClipEdits,
    notes: HirariRegionVariAudioStore,
    history: *mut HirariRegionProcessingHistoryStore,
    stretch: Option<StretchHandle>,
}

impl Drop for RegionEntry {
    fn drop(&mut self) {
        unsafe { hirari_region_processing_history_clear(self.history) };
        release_audio(self.audio);
    }
}

/// An immutable projection the audio callback reads: the render inputs plus the
/// frozen tables whose buffers they address. Nothing is mutated after
/// installation, so a callback can keep reading a retired generation safely.
struct Generation {
    inputs: *mut HirariTrackRegionRenderInput,
    length: usize,
    clips: Vec<Box<RegionClipEditsFrozen>>,
    notes: Vec<Box<HirariVariAudioFrozen>>,
}

// Rust owns the buffers and only the control thread installs or retires one.
unsafe impl Send for Generation {}

impl Drop for Generation {
    fn drop(&mut self) {
        if self.inputs.is_null() || self.length == 0 {
            return;
        }
        if let Ok(layout) = Layout::array::<HirariTrackRegionRenderInput>(self.length) {
            unsafe { dealloc(self.inputs.cast(), layout) };
        }
        self.inputs = ptr::null_mut();
    }
}

struct TrackRegions {
    track_id: u32,
    host: *mut c_void,
    entries: Vec<RegionEntry>,
    sample_rate: f64,
    block_size: u32,
    publications: u64,
    current: Option<Arc<Generation>>,
    retired: Vec<Arc<Generation>>,
}

// The store lock serialises every mutation, and the raw pointers it holds are
// handles owned by the C++ engine or buffers Rust itself allocated.
unsafe impl Send for TrackRegions {}
unsafe impl Sync for TrackRegions {}
unsafe impl Sync for Generation {}

/// Host callbacks into the native engine. Region data is Rust-owned; the host
/// answers for the decoded buffer it owns and for the track's published
/// pointer, freeze invalidation and layout dirty bit.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariRegionStorageHostOps {
    /// Reports a decoded-audio handle's channel count; with room in `out`, also
    /// writes the channel pointers and the frame count.
    pub audio_view:
        Option<unsafe extern "C" fn(*mut c_void, *mut *const f32, usize, *mut u64) -> u32>,
    /// Copies an owned handle so a second owner can outlive the region.
    pub retain_handle: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    /// Releases a handle the store or the host produced.
    pub release_handle: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Installs a published render-input array into the track's atomics.
    pub install_regions: Option<
        unsafe extern "C" fn(*mut c_void, *const HirariTrackRegionRenderInput, usize, u8),
    >,
    /// Drops the rendered freeze: a region edit makes it stale.
    pub invalidate_frozen: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Flags the project layout dirty after a region write.
    pub mark_layout: Option<unsafe extern "C" fn(*mut c_void)>,
    /// True while an F0 analysis job owns the track's note table.
    pub pitch_running: Option<unsafe extern "C" fn(*mut c_void) -> bool>,
}

unsafe impl Send for HirariRegionStorageHostOps {}
unsafe impl Sync for HirariRegionStorageHostOps {}

static STORE: OnceLock<Mutex<Vec<TrackRegions>>> = OnceLock::new();
static HOST_OPS: OnceLock<Mutex<Option<HirariRegionStorageHostOps>>> = OnceLock::new();
static DATA_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn store() -> &'static Mutex<Vec<TrackRegions>> {
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

fn host_ops() -> Option<HirariRegionStorageHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|ops| *ops)
}

fn release_audio(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let Some(release) = host_ops().and_then(|ops| ops.release_handle) else {
        return;
    };
    unsafe { release(handle) };
}

fn retain_audio(handle: *mut c_void) -> *mut c_void {
    if handle.is_null() {
        return ptr::null_mut();
    }
    let Some(retain) = host_ops().and_then(|ops| ops.retain_handle) else {
        return ptr::null_mut();
    };
    unsafe { retain(handle) }
}

fn track_call(host: *mut c_void, pick: impl Fn(HirariRegionStorageHostOps) -> Option<unsafe extern "C" fn(*mut c_void)>) {
    let Some(ops) = host_ops() else { return };
    if let Some(call) = pick(ops) {
        unsafe { call(host) };
    }
}

fn install_regions(
    host: *mut c_void,
    inputs: *const HirariTrackRegionRenderInput,
    length: usize,
    published: u8,
) {
    let Some(install) = host_ops().and_then(|ops| ops.install_regions) else {
        return;
    };
    unsafe { install(host, inputs, length, published) };
}

fn mark_layout(host: *mut c_void) {
    track_call(host, |ops| ops.mark_layout);
}

fn invalidate_frozen(host: *mut c_void) {
    track_call(host, |ops| ops.invalidate_frozen);
}

fn pitch_running(host: *mut c_void) -> bool {
    host_ops()
        .and_then(|ops| ops.pitch_running)
        .is_some_and(|running| unsafe { running(host) })
}

/// Reads a decoded-audio handle into the two channel pointers the renderer
/// needs plus its frame count. A single-channel handle reports the same pointer
/// for both channels, matching the C++ handle's behaviour.
fn audio_view(
    handle: *mut c_void,
    left: &mut *const f32,
    right: &mut *const f32,
    samples: &mut u64,
) {
    *left = ptr::null();
    *right = ptr::null();
    *samples = 0;
    let Some(view) = host_ops().and_then(|ops| ops.audio_view) else {
        return;
    };
    if handle.is_null() {
        return;
    }
    let mut pointers: [*const f32; 2] = [ptr::null(); 2];
    let mut frames = 0u64;
    let channels = unsafe { view(handle, pointers.as_mut_ptr(), 2, &mut frames) };
    if channels == 0 {
        return;
    }
    *left = pointers[0];
    *right = if channels > 1 { pointers[1] } else { pointers[0] };
    *samples = frames;
}

fn audio_dimensions(handle: *mut c_void) -> (u64, u32) {
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

fn new_note_store() -> HirariRegionVariAudioStore {
    unsafe { *Box::from_raw(hirari_region_vari_audio_store_new()) }
}

fn new_clip_store() -> RegionClipEdits {
    unsafe { *Box::from_raw(hirari_region_clip_edits_create()) }
}

fn empty_history() -> *mut HirariRegionProcessingHistoryStore {
    unsafe { hirari_region_processing_history_new() }
}


/// Materializes a history into `(id, operation, parameter, enabled)` tuples for
/// an undo snapshot.
fn history_steps(store: *const HirariRegionProcessingHistoryStore) -> Vec<(u32, Vec<u8>, f32, bool)> {
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

fn bytes_of<'a>(data: *const u8, len: usize) -> &'a [u8] {
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

fn track_position(tracks: &[TrackRegions], key: *mut c_void) -> Option<usize> {
    tracks.iter().position(|track| track.host == key)
}

fn entry_index(track: &TrackRegions, region_id: u32) -> Option<usize> {
    track.entries.iter().position(|entry| entry.id == region_id)
}

/// Runs the insert normalization for one region: metadata from the decoded
/// buffer, then the capped edit tables, then a sorted insert by timeline start.
/// Publication is the caller's, so a batch of adds can publish once.
#[allow(clippy::too_many_arguments)]
fn finish_insert(
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
fn rebuild_stretchers(track: &mut TrackRegions) {
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
fn publish(track: &mut TrackRegions) {
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

/// Copies a region's published clip-edit table into `out`, reporting the total
/// count either way. Reads the projection the callback sees, so a reader can
/// never observe a later edit.
fn read_projection<T: Copy>(
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
pub unsafe extern "C" fn hirari_region_store_find(
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
pub unsafe extern "C" fn hirari_region_store_warp_markers(
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
pub unsafe extern "C" fn hirari_region_store_range_edits(
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
pub unsafe extern "C" fn hirari_region_store_comp_ranges(
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
pub unsafe extern "C" fn hirari_region_store_region_path(
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
pub unsafe extern "C" fn hirari_region_store_list_track_regions(
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
pub extern "C" fn hirari_region_store_count(key: *mut c_void) -> usize {
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
pub unsafe extern "C" fn hirari_region_store_track_token(
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
pub unsafe extern "C" fn hirari_region_store_list_sync_group(
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
pub extern "C" fn hirari_region_store_retain_audio(
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
pub extern "C" fn hirari_region_store_note_snapshot(
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
pub unsafe extern "C" fn hirari_region_store_note_edit(
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
pub extern "C" fn hirari_region_store_snapshot_region(
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

fn region_snapshot(track: &TrackRegions, region_id: u32) -> Option<RegionSnapshot> {
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
pub unsafe extern "C" fn hirari_region_store_release_snapshot(
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
pub unsafe extern "C" fn hirari_region_store_insert_region(
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
pub unsafe extern "C" fn hirari_region_store_clone_region(
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
pub unsafe extern "C" fn hirari_region_store_copy_regions(
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
fn add_snapshot(track: &mut TrackRegions, region_id: u32, snapshot: &RegionSnapshot) -> bool {
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
pub unsafe extern "C" fn hirari_region_store_layout_header(
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
pub unsafe extern "C" fn hirari_region_store_layout_string(
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
pub unsafe extern "C" fn hirari_region_store_project_end(key: *mut c_void, out: *mut u64) -> bool {
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
pub unsafe extern "C" fn hirari_region_store_span_at(
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
pub extern "C" fn hirari_region_store_forget_waveforms(host: *mut c_void, key: *mut c_void) {
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
pub unsafe extern "C" fn hirari_region_store_apply_edit(
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

fn apply_one(track: &mut TrackRegions, payload: &HirariRegionEditPayload) -> bool {
    invalidate_frozen(track.host);
    match payload.op {
        HIRARI_REGION_EDIT_STATE => {
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let mut snapshot = match region_snapshot(track, payload.region_id) {
                Some(snapshot) => snapshot,
                None => return false,
            };
            snapshot.state = payload.state;
            // The id is stable, so the replacement takes the removed region's
            // place after the same sorted-insert normalization `addRegion` ran.
            track.entries.remove(index);
            if !add_snapshot(track, payload.region_id, &snapshot) {
                return false;
            }
            publish(track);
            true
        }
        HIRARI_REGION_EDIT_REMOVE => {
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            track.entries.remove(index);
            publish(track);
            true
        }
        HIRARI_REGION_EDIT_RESTORE => {
            let Some(snapshot) = (unsafe { (payload.blob as *const RegionSnapshot).as_ref() })
            else {
                return false;
            };
            // Undo replays a whole-region snapshot over whatever the region has
            // become, so a live entry is dropped first; its retained audio
            // handle is released by that drop and the snapshot keeps its own.
            if let Some(index) = entry_index(track, payload.region_id) {
                track.entries.remove(index);
            }
            if !add_snapshot(track, payload.region_id, snapshot) {
                return false;
            }
            publish(track);
            true
        }
        HIRARI_REGION_EDIT_NOTE_STORE => {
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let store = ptr::from_mut(&mut track.entries[index].notes);
            if !unsafe { hirari_region_note_store_restore(payload.blob as *mut c_void, store) } {
                return false;
            }
            publish(track);
            true
        }
        HIRARI_REGION_EDIT_GEOMETRY => {
            let Some(markers) = (unsafe { blob_span::<HirariWarpMarker>(payload) }) else {
                return false;
            };
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let locked = track.entries[index].state.locked;
            if locked {
                return false;
            }
            let clip = ptr::from_mut(&mut track.entries[index].clip);
            if unsafe {
                hirari_region_clip_edits_set_warp_markers(clip, markers.as_ptr(), markers.len(), 0, 0, false)
            } < 0
            {
                return false;
            }
            let state = payload.state;
            let entry = &mut track.entries[index];
            entry.state.start = state.start;
            entry.state.len = state.len;
            entry.state.source_offset = state.source_offset;
            entry.state.source_length = state.source_length;
            entry.state.fade_in_samples = state.fade_in_samples;
            entry.state.fade_out_samples = state.fade_out_samples;
            entry.state.warp_ratio = state.warp_ratio;
            publish(track);
            true
        }
        HIRARI_REGION_EDIT_MARKERS => {
            let Some(markers) = (unsafe { blob_span::<HirariWarpMarker>(payload) }) else {
                return false;
            };
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let entry = &mut track.entries[index];
            if entry.state.locked {
                return false;
            }
            let source_span = if entry.state.source_length > 0 {
                entry.state.source_length
            } else {
                entry.state.len
            };
            let timeline = entry.state.len;
            let clip = ptr::from_mut(&mut entry.clip);
            let applied = unsafe {
                hirari_region_clip_edits_set_warp_markers(
                    clip,
                    markers.as_ptr(),
                    markers.len(),
                    source_span,
                    timeline,
                    true,
                )
            };
            if applied < 0 {
                return false;
            }
            if applied > 0 {
                publish(track);
            }
            true
        }
        HIRARI_REGION_EDIT_RANGE_EDITS => {
            let Some(edits) = (unsafe { blob_span::<NativeRangeEdit>(payload) }) else {
                return false;
            };
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            if track.entries[index].state.locked {
                return false;
            }
            let length = track.entries[index].state.len;
            let clip = ptr::from_mut(&mut track.entries[index].clip);
            if unsafe {
                hirari_region_clip_edits_replace_range_edits(clip, edits.as_ptr(), edits.len(), length, false)
            } {
                publish(track);
                return true;
            }
            false
        }
        HIRARI_REGION_EDIT_COMP_RANGES => {
            let Some(ranges) = (unsafe { blob_span::<NativeCompRange>(payload) }) else {
                return false;
            };
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let managed = payload.managed;
            let clip = ptr::from_mut(&mut track.entries[index].clip);
            if unsafe {
                hirari_region_clip_edits_replace_comp_ranges(clip, ranges.as_ptr(), ranges.len(), managed)
            } {
                publish(track);
                return true;
            }
            false
        }
        HIRARI_REGION_EDIT_PITCH_COMMIT => {
            let analysis = payload.blob as *mut c_void;
            if analysis.is_null() {
                return false;
            }
            let current_generation = track.publications;
            let index = entry_index(track, payload.region_id);
            let accepted = index.is_some()
                && (current_generation == payload.generation || payload.generation == 0);
            let mut applied = false;
            if accepted {
                let store = ptr::from_mut(&mut track.entries[index.unwrap()].notes).cast();
                applied = unsafe { hirari_audio_pitch_commit_region_result(analysis, store) };
                if applied {
                    publish(track);
                }
            }
            unsafe { hirari_audio_pitch_result_destroy(analysis) };
            applied
        }
        HIRARI_REGION_EDIT_REPLACE_AUDIO => {
            if payload.blob.is_null() || payload.blob_len < HANDLE_BYTES {
                return false;
            }
            let mut raw = [0u8; HANDLE_BYTES];
            raw.copy_from_slice(unsafe { slice::from_raw_parts(payload.blob, HANDLE_BYTES) });
            let handle = usize::from_ne_bytes(raw) as *mut c_void;
            let path = bytes_of(unsafe { payload.blob.add(HANDLE_BYTES) }, payload.blob_len - HANDLE_BYTES).to_vec();
            let Some(index) = entry_index(track, payload.region_id) else {
                release_audio(handle);
                return false;
            };
            let entry = &mut track.entries[index];
            let previous = unsafe { ptr::replace(&mut entry.audio, handle) };
            release_audio(previous);
            entry.state = payload.state;
            entry.path = path.clone();
            entry.name = path;
            // The previous source's warp markers described a different file.
            let clip = ptr::from_mut(&mut entry.clip);
            unsafe {
                hirari_region_clip_edits_set_warp_markers(clip, ptr::null(), 0, 0, 0, false);
            }
            publish(track);
            true
        }
        _ => {
            let Some(index) = entry_index(track, payload.region_id) else {
                return false;
            };
            let entry = &mut track.entries[index];
            let state = payload.state;
            if state.pitch_preserve_warp != entry.state.pitch_preserve_warp
                && entry.state.source_length == 0
            {
                // The renderer needs the untrimmed length to build the warp map.
                entry.state.source_length = entry.state.len;
            }
            entry.state.clip_gain = state.clip_gain;
            entry.state.muted = state.muted;
            entry.state.reverse = state.reverse;
            entry.state.pitch_semitones = state.pitch_semitones;
            entry.state.loop_count = state.loop_count;
            entry.state.sync_group = state.sync_group;
            entry.state.fade_in_samples = state.fade_in_samples;
            entry.state.fade_out_samples = state.fade_out_samples;
            entry.state.locked = state.locked;
            entry.state.pitch_preserve_warp = state.pitch_preserve_warp;
            publish(track);
            true
        }
    }
}

/// Borrows a payload blob as a slice of `T`, rejecting a ragged length or a
/// missing buffer for a non-zero count.
///
/// # Safety
/// `payload.blob` is readable for `payload.blob_len` bytes.
unsafe fn blob_span<T>(payload: &HirariRegionEditPayload) -> Option<&[T]> {
    let size = std::mem::size_of::<T>();
    if size == 0 || payload.blob_len % size != 0 {
        return None;
    }
    let count = payload.blob_len / size;
    if count == 0 {
        return Some(&[]);
    }
    if payload.blob.is_null() {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(payload.blob.cast::<T>(), count) })
}

/// Clears a region's processing history. Returns false when the region is gone.
#[no_mangle]
pub extern "C" fn hirari_region_store_clear_processing(key: *mut c_void, region_id: u32) -> bool {
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        return false;
    };
    let Some(index) = entry_index(track, region_id) else {
        return false;
    };
    if unsafe { hirari_region_processing_history_count(track.entries[index].history) } == 0 {
        return false;
    }
    unsafe { hirari_region_processing_history_reset(track.entries[index].history) };
    invalidate_frozen(track.host);
    publish(track);
    true
}

/// Appends one processing step. The store owns the admissibility policy.
///
/// # Safety
/// `step.operation` is readable for its length.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_append_processing(
    key: *mut c_void,
    region_id: u32,
    step: HirariEventStep,
) -> bool {
    let Ok(mut tracks) = store().lock() else {
        return false;
    };
    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {
        return false;
    };
    let Some(index) = entry_index(track, region_id) else {
        return false;
    };
    if unsafe { !hirari_region_processing_history_append(track.entries[index].history, step) } {
        return false;
    }
    invalidate_frozen(track.host);
    publish(track);
    true
}

#[cfg(test)]
#[path = "region_store_tests.rs"]
mod region_store_tests;

