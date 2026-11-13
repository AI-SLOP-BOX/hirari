//! Layout revision machinery, split out of engine_track.rs.
use super::*;

/// Shared invalidation for cheap project-layout polling. The realtime thread
/// only stores a dirty bit when it queues write-automation points; the
/// control-side reader folds that bit into the monotonic revision.
pub struct LayoutRevision {
    pub(crate) value: AtomicU64,
    pub(crate) realtime_automation_dirty: AtomicBool,
}

impl LayoutRevision {
    pub(crate) fn mark(&self) {
        self.value.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn current(&self) -> u64 {
        if self.realtime_automation_dirty.swap(false, Ordering::AcqRel) {
            self.mark();
        }
        self.value.load(Ordering::Acquire)
    }
}

/// # Safety
/// The engine holds this handle for the whole process lifetime it prepares
/// tracks with, and every track clones the arc out of it.
#[no_mangle]
pub extern "C" fn hirari_layout_revision_create() -> *mut c_void {
    let revision = Arc::new(LayoutRevision {
        value: AtomicU64::new(1),
        realtime_automation_dirty: AtomicBool::new(false),
    });
    Box::into_raw(Box::new(revision)).cast()
}

/// # Safety
/// `handle` is null or a handle from [`hirari_layout_revision_create`] that no
/// other caller has released.
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_revision_release(handle: *mut c_void) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle.cast::<Arc<LayoutRevision>>()) });
    }
}

/// # Safety
/// `handle` is null or a live handle from [`hirari_layout_revision_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_revision_mark(handle: *mut c_void) {
    if let Some(revision) = revision_of(handle) {
        revision.mark();
    }
}

/// Folds the realtime automation dirty bit in and reports the revision.
///
/// # Safety
/// `handle` is null or a live handle from [`hirari_layout_revision_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_layout_revision_current(handle: *mut c_void) -> u64 {
    revision_of(handle).map_or(0, |revision| revision.current())
}

pub(crate) fn revision_of<'a>(handle: *mut c_void) -> Option<&'a Arc<LayoutRevision>> {
    unsafe { (handle as *mut Arc<LayoutRevision>).as_ref() }
}

/// The region store's host callbacks. They used to be the C++ `Track`'s static
/// member table, handed to the store once during engine startup; every one of
/// them are an entry point of this module now, so the native side supplies no
/// table at all. The handle each receives is the track object pointer, which is
/// also the key the store's tables use.
static REGION_HOST_OPS_INSTALL: std::sync::Once = std::sync::Once::new();

const REAL_REGION_HOST_OPS: HirariRegionStorageHostOps = HirariRegionStorageHostOps {
    audio_view: Some(hirari_storage_handle_view),
    retain_handle: Some(hirari_storage_handle_retain),
    release_handle: Some(hirari_storage_handle_release),
    install_regions: Some(install_regions),
    invalidate_frozen: Some(hirari_track_begin_edit),
    mark_layout: Some(hirari_track_mark_layout),
    pitch_running: Some(hirari_track_pitch_running),
};

/// Installs the track's own region storage host. The engine calls this while it
/// installs its host tables, before the region edit host is installed: the edit
/// host only routes its region-data callbacks through the store once the storage
/// host answers, so the order decides whether an audio region can be inserted at
/// all. Binding a track installs it too, for a test binary with no engine.
#[no_mangle]
pub extern "C" fn hirari_track_region_host_install() {
    REGION_HOST_OPS_INSTALL
        .call_once(|| unsafe { hirari_region_storage_host_install(&REAL_REGION_HOST_OPS) });
}
