//! Region store setup: window tables, the track store types and the host
//! ops bus, split out of region_store.rs. The types are re-exported
//! through the parent.
use super::*;

pub(crate) fn wsola_window() -> &'static [f32; WSOLA_WINDOW_TAPS] {
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
pub(crate) const HANDLE_BYTES: usize = std::mem::size_of::<usize>();

pub(crate) struct StretchHandle(pub(crate) *mut c_void);

impl Drop for StretchHandle {
    fn drop(&mut self) {
        unsafe { hirari_region_stretch_destroy(self.0) };
    }
}

/// An owned region copy handed to the undo history. Audio is retained rather
/// than shared, so the copy stays valid after the live region moves on.
pub(crate) struct RegionSnapshot {
    pub(crate) path: Vec<u8>,
    pub(crate) name: Vec<u8>,
    pub(crate) state: HirariRegionState,
    pub(crate) audio: *mut c_void,
    pub(crate) clip: *mut RegionClipEdits,
    pub(crate) notes: *mut HirariRegionVariAudioStore,
    pub(crate) history: Vec<(u32, Vec<u8>, f32, bool)>,
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
pub(crate) struct RegionEntry {
    pub(crate) id: u32,
    pub(crate) path: Vec<u8>,
    pub(crate) name: Vec<u8>,
    pub(crate) state: HirariRegionState,
    pub(crate) audio: *mut c_void,
    pub(crate) clip: RegionClipEdits,
    pub(crate) notes: HirariRegionVariAudioStore,
    pub(crate) history: *mut HirariRegionProcessingHistoryStore,
    pub(crate) stretch: Option<StretchHandle>,
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
pub(crate) struct Generation {
    pub(crate) inputs: *mut HirariTrackRegionRenderInput,
    pub(crate) length: usize,
    pub(crate) clips: Vec<Box<RegionClipEditsFrozen>>,
    pub(crate) notes: Vec<Box<HirariVariAudioFrozen>>,
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

pub(crate) struct TrackRegions {
    pub(crate) track_id: u32,
    pub(crate) host: *mut c_void,
    pub(crate) entries: Vec<RegionEntry>,
    pub(crate) sample_rate: f64,
    pub(crate) block_size: u32,
    pub(crate) publications: u64,
    pub(crate) current: Option<Arc<Generation>>,
    pub(crate) retired: Vec<Arc<Generation>>,
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

pub(crate) static STORE: OnceLock<Mutex<Vec<TrackRegions>>> = OnceLock::new();
pub(crate) static HOST_OPS: OnceLock<Mutex<Option<HirariRegionStorageHostOps>>> = OnceLock::new();
pub(crate) static DATA_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn store() -> &'static Mutex<Vec<TrackRegions>> {
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn host_ops() -> Option<HirariRegionStorageHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|ops| *ops)
}

pub(crate) fn release_audio(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let Some(release) = host_ops().and_then(|ops| ops.release_handle) else {
        return;
    };
    unsafe { release(handle) };
}

pub(crate) fn retain_audio(handle: *mut c_void) -> *mut c_void {
    if handle.is_null() {
        return ptr::null_mut();
    }
    let Some(retain) = host_ops().and_then(|ops| ops.retain_handle) else {
        return ptr::null_mut();
    };
    unsafe { retain(handle) }
}

pub(crate) fn track_call(host: *mut c_void, pick: impl Fn(HirariRegionStorageHostOps) -> Option<unsafe extern "C" fn(*mut c_void)>) {
    let Some(ops) = host_ops() else { return };
    if let Some(call) = pick(ops) {
        unsafe { call(host) };
    }
}

pub(crate) fn install_regions(
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

pub(crate) fn mark_layout(host: *mut c_void) {
    track_call(host, |ops| ops.mark_layout);
}

pub(crate) fn invalidate_frozen(host: *mut c_void) {
    track_call(host, |ops| ops.invalidate_frozen);
}

pub(crate) fn pitch_running(host: *mut c_void) -> bool {
    host_ops()
        .and_then(|ops| ops.pitch_running)
        .is_some_and(|running| unsafe { running(host) })
}

/// Reads a decoded-audio handle into the two channel pointers the renderer
/// needs plus its frame count. A single-channel handle reports the same pointer
/// for both channels, matching the C++ handle's behaviour.
pub(crate) fn audio_view(
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
