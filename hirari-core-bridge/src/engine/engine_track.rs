//! The track object itself.
//!
//! A track never did anything but own Rust storages and order the edits made
//! through them: every fader, plug-in chain, automation curve, freeze render,
//! region table, name, panner, delay line and block buffer it holds lives in
//! this crate already. What stayed in C++ was the ownership of those handles,
//! the constructor and destructor ordering, and the three-step control edit
//! protocol (drop the frozen render, mutate, bump the layout revision). This
//! module owns all of it, so the native side keeps no track type at all — only
//! the opaque pointers the storages are addressed through.
//!
//! A track is reference-counted here. The engine's track table holds the arcs,
//! an edit host or a region job holds a clone through [`hirari_track_owner_retain`]
//! and releases it with [`hirari_track_owner_release`], and the object pointer a
//! caller passes around is the stable address inside the allocation. That is the
//! same address the region store keys its table by, so a retained identity stays
//! comparable for as long as the track lives.
//!
//! Lock discipline is unchanged: the engine's tracks lock serialises every
//! control walk, and the realtime callback only ever reads the atomics the
//! storages publish.

use crate::audio_buffer_ops::{
    hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
    hirari_audio_buffer_storage_samples,
};
use crate::automation_capture_runtime::hirari_automation_recorder_instance;
use crate::built_in_midi_instrument::{
    hirari_builtin_midi_instrument_create, hirari_builtin_midi_instrument_destroy,
};
use crate::channel_strip::{
    hirari_channel_strip_begin_pitch_analysis, hirari_channel_strip_create,
    hirari_channel_strip_destroy, hirari_channel_strip_finish_pitch_analysis,
    hirari_channel_strip_gain, hirari_channel_strip_is_pitch_analysis_running,
    hirari_channel_strip_is_record_armed, hirari_channel_strip_pan,
    hirari_channel_strip_set_gain, hirari_channel_strip_set_pan,
    hirari_channel_strip_set_record_armed, hirari_channel_strip_set_solo,
};
use crate::effect_chain_host::{
    hirari_effect_chain_create, hirari_effect_chain_destroy, hirari_effect_chain_reclaim, hirari_effect_chain_sandbox_maintain,
    hirari_effect_chain_sandbox_slot_count, hirari_effect_chain_sandbox_status,
    hirari_effect_chain_total_latency_samples, SandboxStatusOut,
};
use crate::midi_buffer_ops::{hirari_midi_buffer_create, hirari_midi_buffer_destroy};
use crate::plugin_sandbox_maintenance::{HirariPluginSandboxStatus, StatusVisitorFn};
use crate::region_store::{
    hirari_region_storage_host_install, hirari_region_store_forget, hirari_region_store_install,
    HirariRegionStorageHostOps,
};
use crate::storage_handle::{
    hirari_storage_handle_release, hirari_storage_handle_retain, hirari_storage_handle_view,
};
use crate::track_automation_store::{
    hirari_track_automation_store_create, hirari_track_automation_store_destroy,
    hirari_track_automation_store_flush_recorded, hirari_track_automation_store_record,
};
use crate::track_frozen_store::{
    hirari_track_frozen_store_clear, hirari_track_frozen_store_create,
    hirari_track_frozen_store_destroy,
};
use crate::track_holographic_panner::{
    hirari_track_holographic_panner_clear_kernel, hirari_track_holographic_panner_create,
    hirari_track_holographic_panner_destroy, hirari_track_holographic_panner_set_kernel,
};
use crate::track_midi_articulation::{
    hirari_track_midi_articulation_create, hirari_track_midi_articulation_destroy,
    hirari_track_midi_expression_map_set,
};
use crate::track_name_store::{
    hirari_track_name_store_create, hirari_track_name_store_destroy, hirari_track_name_store_set,
};
use crate::track_pdc_delay::{hirari_track_pdc_delay_create, hirari_track_pdc_delay_destroy};
use crate::track_region_render::HirariTrackRegionRenderInput;
use crate::track_runtime::{
    hirari_track_runtime_create, hirari_track_runtime_destroy, hirari_track_runtime_is_muted, hirari_track_runtime_pdc_requested,
    hirari_track_runtime_prepare, hirari_track_runtime_publish_regions,
    hirari_track_runtime_reset_for_offline_render, hirari_track_runtime_set_delays,
    hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
    hirari_track_runtime_set_spatial, hirari_track_runtime_track_delay_samples,
};
use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// `Track::Type`: the role the graph and the DSP variants select.
pub const TRACK_TYPE_AUDIO: u32 = 0;
pub const TRACK_TYPE_MIDI: u32 = 1;
pub const TRACK_TYPE_INSTRUMENT: u32 = 2;
pub const TRACK_TYPE_BUS: u32 = 3;
pub const TRACK_TYPE_VOCAL: u32 = 4;

/// One of a track's own storages, selected by the number the layout gather, the
/// track-edit session and the engine block all address it with. A control walk
/// that needs a track's state asks for the storage instead of a per-field relay.
pub const STORAGE_STRIP: u32 = 0;
pub const STORAGE_CHAIN: u32 = 1;
pub const STORAGE_AUTOMATION: u32 = 2;
pub const STORAGE_FROZEN: u32 = 3;
pub const STORAGE_RUNTIME: u32 = 4;
pub const STORAGE_ARTICULATION: u32 = 5;
pub const STORAGE_NAME: u32 = 6;
pub const STORAGE_WORK_BUFFER: u32 = 7;
pub const STORAGE_PLUGIN_MIDI: u32 = 8;
pub const STORAGE_BUS_STATE: u32 = 9;
pub const STORAGE_SEND_PRE: u32 = 10;
pub const STORAGE_SEND_POST: u32 = 11;

/// A track that is not a bus occupies no bus slot.
pub const NO_BUS_ID: u32 = u32::MAX;

/// The longest HRTF kernel a panner accepts, so a malformed kernel is rejected
/// before it can reach an edit.
const MAX_HRTF_TAPS: u32 = 128;

/// Shared invalidation for cheap project-layout polling. The realtime thread
/// only stores a dirty bit when it queues write-automation points; the
/// control-side reader folds that bit into the monotonic revision.
pub struct LayoutRevision {
    value: AtomicU64,
    realtime_automation_dirty: AtomicBool,
}

impl LayoutRevision {
    fn mark(&self) {
        self.value.fetch_add(1, Ordering::Release);
    }

    fn current(&self) -> u64 {
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

fn revision_of<'a>(handle: *mut c_void) -> Option<&'a Arc<LayoutRevision>> {
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

/// One track: the handles of the Rust storages it owns, its role in the graph
/// and the layout revision it marks when it changes.
pub struct Track {
    revision: Option<Arc<LayoutRevision>>,
    id: u32,
    kind: u32,
    /// The bus slot a bus track occupies; [`NO_BUS_ID`] for every other kind.
    bus_id: u32,
    bus_state: *mut c_void,
    strip: *mut c_void,
    chain: *mut c_void,
    plugin_midi: *mut c_void,
    articulation: *mut c_void,
    built_in_midi: *mut c_void,
    pdc_delay: *mut c_void,
    runtime: *mut c_void,
    frozen_store: *mut c_void,
    automation_store: *mut c_void,
    name_store: *mut c_void,
    panner: *mut c_void,
    work: *mut c_void,
    pre_fader: *mut c_void,
    send_pre: *mut c_void,
    send_post: *mut c_void,
    pre_insert: *mut c_void,
}

// Every field is a handle into a Rust storage with its own synchronisation; the
// track itself is only moved between threads under the engine's tracks lock.
unsafe impl Send for Track {}
unsafe impl Sync for Track {}

impl Drop for Track {
    fn drop(&mut self) {
        // The runtime renders whole blocks through every other handle, so it is
        // retired first and the tap buffers it wrote last.
        unsafe {
            hirari_track_runtime_destroy(self.runtime);
            hirari_midi_buffer_destroy(self.plugin_midi);
            hirari_region_store_forget(self.object());
            hirari_builtin_midi_instrument_destroy(self.built_in_midi);
            hirari_track_midi_articulation_destroy(self.articulation);
            hirari_track_pdc_delay_destroy(self.pdc_delay);
            hirari_track_automation_store_destroy(self.automation_store);
            hirari_track_name_store_destroy(self.name_store);
            hirari_effect_chain_destroy(self.chain);
            hirari_channel_strip_destroy(self.strip);
            hirari_track_holographic_panner_destroy(self.panner);
            hirari_track_frozen_store_destroy(self.frozen_store);
            for buffer in [
                self.work,
                self.pre_fader,
                self.send_pre,
                self.send_post,
                self.pre_insert,
            ] {
                hirari_audio_buffer_storage_destroy(buffer);
            }
        }
    }
}

impl Track {
    /// Allocates every storage the track owns. The runtime is left unbound
    /// until [`Track::bind`] runs, because binding registers this address with
    /// the region store.
    fn new(id: u32, kind: u32, revision: Option<Arc<LayoutRevision>>) -> Self {
        Self {
            revision,
            id,
            kind,
            bus_id: NO_BUS_ID,
            bus_state: std::ptr::null_mut(),
            strip: unsafe { hirari_channel_strip_create() },
            chain: unsafe { hirari_effect_chain_create() },
            plugin_midi: unsafe { hirari_midi_buffer_create() },
            articulation: unsafe { hirari_track_midi_articulation_create() },
            built_in_midi: if kind == TRACK_TYPE_MIDI || kind == TRACK_TYPE_INSTRUMENT {
                unsafe { hirari_builtin_midi_instrument_create() }
            } else {
                std::ptr::null_mut()
            },
            pdc_delay: unsafe { hirari_track_pdc_delay_create() },
            runtime: std::ptr::null_mut(),
            frozen_store: unsafe { hirari_track_frozen_store_create() },
            automation_store: unsafe { hirari_track_automation_store_create() },
            name_store: unsafe { hirari_track_name_store_create() },
            panner: unsafe { hirari_track_holographic_panner_create() },
            work: unsafe { hirari_audio_buffer_storage_create() },
            pre_fader: unsafe { hirari_audio_buffer_storage_create() },
            send_pre: unsafe { hirari_audio_buffer_storage_create() },
            send_post: unsafe { hirari_audio_buffer_storage_create() },
            pre_insert: unsafe { hirari_audio_buffer_storage_create() },
        }
    }

    /// Publishes the initial name, binds the realtime runtime to everything the
    /// track owns and installs the region table this track renders from.
    fn bind(&mut self, name: &[u8]) {
        // A track existing is the engine existing, so this is where the region
        // store gets the callbacks it used to receive from native startup.
        hirari_track_region_host_install();
        // The name is a store the track owns, so binding hands over the initial
        // value rather than keeping a copy.
        unsafe { hirari_track_name_store_set(self.name_store, name.as_ptr(), name.len()) };
        // The runtime renders whole blocks, so it is bound to every DSP handle
        // and tap buffer the track owns. It is created last and destroyed first.
        self.runtime = unsafe {
            hirari_track_runtime_create(
                self.id,
                self.kind,
                self.chain,
                self.strip,
                self.automation_store,
                self.frozen_store,
                self.panner,
                self.pdc_delay,
                self.work,
                self.pre_insert,
                self.pre_fader,
                self.send_pre,
                self.send_post,
                self.plugin_midi,
                self.articulation,
                self.built_in_midi,
            )
        };
        // The region table is keyed by the object, not by the id: two engines
        // can own the same id at the same time.
        unsafe { hirari_region_store_install(self.object(), self.id) };
    }

    fn mark_layout(&self) {
        if let Some(revision) = &self.revision {
            revision.mark();
        }
    }

    /// The control protocol every edit keeps: the frozen render is dropped
    /// before the store write and the layout revision bumped after it.
    fn begin_edit(&self) {
        unsafe { hirari_track_frozen_store_clear(self.frozen_store) };
        self.mark_layout();
    }

    fn set_delays(&self, pdc_requested: u32, manual: u32) {
        unsafe { hirari_track_runtime_set_delays(self.runtime, pdc_requested, manual) };
    }

    fn pdc_requested(&self) -> u32 {
        unsafe { hirari_track_runtime_pdc_requested(self.runtime) }
    }

    fn manual_delay(&self) -> u32 {
        unsafe { hirari_track_runtime_track_delay_samples(self.runtime) }
    }

    /// The address every entry point and the region store use for this track.
    fn object(&self) -> *mut c_void {
        self as *const Track as *mut c_void
    }

    /// The graph identity the track table resolves and maps by.
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// `Track::Type` as its numeric value; [`TRACK_TYPE_BUS`] is a bus.
    pub(crate) fn kind(&self) -> u32 {
        self.kind
    }

    /// The bus slot this track occupies, or [`NO_BUS_ID`].
    pub(crate) fn bus_slot(&self) -> u32 {
        self.bus_id
    }

    /// The track's own reported latency, the input to the PDC solve.
    pub(crate) fn latency_samples(&self) -> u32 {
        unsafe { hirari_effect_chain_total_latency_samples(self.chain) }
    }

    /// Applies one solved offset, leaving the musician's manual delay in place.
    pub(crate) fn apply_pdc_offset(&self, samples: u32) {
        self.set_delays(samples, self.manual_delay());
    }
}

fn track_ref<'a>(handle: *mut c_void) -> Option<&'a Track> {
    unsafe { (handle as *const Track).as_ref() }
}

/// Builds a track and hands back its owning handle. `out_track` receives the
/// object pointer every other entry point addresses it with; it stays valid for
/// as long as one owner handle exists.
///
/// # Safety
/// `name` must be readable for `name_len` bytes, `revision` must be null or a
/// live [`hirari_layout_revision_create`] handle and `out_track` must be
/// writable or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_new(
    id: u32,
    name: *const u8,
    name_len: usize,
    kind: u32,
    revision: *mut c_void,
    out_track: *mut *mut c_void,
) -> *mut c_void {
    let bytes = if name.is_null() {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(name, name_len) }
    };
    let mut arc = Arc::new(Track::new(id, kind, revision_of(revision).cloned()));
    // Binding has to happen inside the allocation: the address it registers is
    // the one the track keeps until its last handle is released.
    if let Some(inner) = Arc::get_mut(&mut arc) {
        inner.bind(bytes);
    }
    if !out_track.is_null() {
        unsafe { *out_track = Arc::as_ptr(&arc).cast_mut().cast() };
    }
    Box::into_raw(Box::new(arc)).cast()
}

/// Clones an owning handle so a second holder can outlive the first.
///
/// # Safety
/// `owner` is null or a handle from [`hirari_track_new`] or
/// [`hirari_track_owner_retain`].
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_retain(owner: *mut c_void) -> *mut c_void {
    let Some(arc) = (unsafe { owner.cast::<Arc<Track>>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(arc.clone())).cast()
}

/// Drops one owning handle; the track's storages are destroyed with its last
/// one, in the order [`Track::drop`] keeps.
///
/// # Safety
/// `owner` is null or a live owning handle, released at most once.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_release(owner: *mut c_void) {
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner.cast::<Arc<Track>>()) });
    }
}

/// The object pointer inside an owning handle, or null.
///
/// # Safety
/// `owner` is null or a live owning handle.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_owner_object(owner: *mut c_void) -> *mut c_void {
    let Some(arc) = (unsafe { owner.cast::<Arc<Track>>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    Arc::as_ptr(arc).cast_mut().cast()
}

/// # Safety
/// `track` is null or an object pointer from [`hirari_track_new`].
#[no_mangle]
pub unsafe extern "C" fn hirari_track_id(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| track.id)
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_type(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| track.kind)
}

/// The same answer as [`hirari_track_type`] in the byte the engine block
/// stores alongside each track.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_type_byte(track: *mut c_void) -> u8 {
    unsafe { hirari_track_type(track) as u8 }
}

/// Binds a bus track to the bus state its engine's bus system hands out. The
/// bus system is only mutated on the control thread, so resolution happens
/// after the replacement graph is committed, never while it is provisional. An
/// unresolved bus loses its slot so the graph cannot route to a stale one.
///
/// # Safety
/// `track` is null or a live track object and `bus_state` is a live bus or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_resolve_bus(
    track: *mut c_void,
    bus_state: *mut c_void,
    is_bus: bool,
) {
    let Some(inner) = (unsafe { (track as *mut Track).as_mut() }) else {
        return;
    };
    inner.bus_state = bus_state;
    inner.bus_id = if is_bus { inner.id } else { NO_BUS_ID };
}

/// One of the track's own storages, or null when it has none of that kind.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_storage(track: *mut c_void, kind: u32) -> *mut c_void {
    let Some(track) = track_ref(track) else {
        return std::ptr::null_mut();
    };
    match kind {
        STORAGE_STRIP => track.strip,
        STORAGE_CHAIN => track.chain,
        STORAGE_AUTOMATION => track.automation_store,
        STORAGE_FROZEN => track.frozen_store,
        STORAGE_RUNTIME => track.runtime,
        STORAGE_ARTICULATION => track.articulation,
        STORAGE_NAME => track.name_store,
        STORAGE_WORK_BUFFER => track.work,
        STORAGE_PLUGIN_MIDI => track.plugin_midi,
        STORAGE_BUS_STATE => track.bus_state,
        STORAGE_SEND_PRE => track.send_pre,
        STORAGE_SEND_POST => track.send_post,
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_mark_layout(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        track.mark_layout();
    }
}

/// Drops the frozen render so a stale one cannot survive a region, parameter or
/// automation mutation, and marks the layout dirty. The audio thread only ever
/// observes the raw pointer the freeze host published, and the store keeps the
/// replaced render until the control side retires it after quiescing.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_begin_edit(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_end_edit(track: *mut c_void) {
    unsafe { hirari_track_mark_layout(track) };
}

/// Rebuilds a track for a new device format. The status is the runtime's own
/// allocation report: 0 ready, 1 a growth the realtime guard declined, 2 an
/// impossible size and 3 an exhausted allocation — the last two are what the
/// host has to report, so a refused growth leaves the previous size in place.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_prepare(
    track: *mut c_void,
    sample_rate: f64,
    block_frames: u32,
) -> u8 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    unsafe { hirari_track_runtime_prepare(track.runtime, track.object(), sample_rate, block_frames) }
}

/// Whether the track's work block can carry a block of this length. The graph
/// skips a track that was prepared for a smaller one instead of rendering past
/// its buffers.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_can_process(track: *mut c_void, frames: u32) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_audio_buffer_storage_samples(track.work) >= frames
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_reset_for_offline_render(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_track_runtime_reset_for_offline_render(track.runtime) };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_latency_samples(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, |track| unsafe {
        hirari_effect_chain_total_latency_samples(track.chain)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_requested(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, Track::pdc_requested)
}

/// User timing offset is intentionally separate from plugin PDC. Both values
/// share one preallocated transition line, but a graph rebuild must never erase
/// the musician's manual delay.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_delay_samples(track: *mut c_void) -> u32 {
    track_ref(track).map_or(0, Track::manual_delay)
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_delay_samples(track: *mut c_void, samples: u32) {
    let Some(track) = track_ref(track) else {
        return;
    };
    let previous = track.manual_delay();
    track.set_delays(track.pdc_requested(), samples);
    if track.manual_delay() != previous {
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_get_volume(track: *mut c_void) -> f32 {
    track_ref(track).map_or(0.0, |track| unsafe {
        hirari_channel_strip_gain(track.strip)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_volume(track: *mut c_void, value: f32) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_gain(track.strip, value) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_pan(track: *mut c_void, value: f32) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_pan(track.strip, value) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_solo(track: *mut c_void, solo: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_channel_strip_set_solo(track.strip, solo) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_muted(track: *mut c_void, muted: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_track_runtime_set_muted(track.runtime, u8::from(muted)) };
        track.mark_layout();
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_is_muted(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe { hirari_track_runtime_is_muted(track.runtime) })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_phase_inverted(track: *mut c_void, inverted: bool) {
    if let Some(track) = track_ref(track) {
        track.begin_edit();
        unsafe { hirari_track_runtime_set_phase_invert(track.runtime, u8::from(inverted)) };
        track.mark_layout();
    }
}

/// Record arm reports whether it actually changed, because the automation
/// recorder admission and the undo entry both key off the transition.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_record_armed(track: *mut c_void, armed: bool) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    if unsafe { hirari_channel_strip_set_record_armed(track.strip, armed) } {
        track.mark_layout();
        return true;
    }
    false
}

/// Spatial coordinates are control-thread input, so a non-finite one is folded
/// to the centre instead of being published to the realtime panner.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_spatial(track: *mut c_void, x: f32, y: f32, z: f32) {
    let Some(track) = track_ref(track) else {
        return;
    };
    let sanitize = |value: f32| {
        if value.is_finite() {
            value.clamp(-100.0, 100.0)
        } else {
            0.0
        }
    };
    track.begin_edit();
    unsafe { hirari_track_runtime_set_spatial(track.runtime, sanitize(x), sanitize(y), sanitize(z)) };
    track.mark_layout();
}

/// # Safety
/// `track` is null or a live track object and `left`/`right` each hold `taps`
/// samples when they are not null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_hrtf_kernel(
    track: *mut c_void,
    left: *const f32,
    right: *const f32,
    taps: u32,
) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    if left.is_null() || right.is_null() || taps == 0 || taps > MAX_HRTF_TAPS {
        return false;
    }
    track.begin_edit();
    let changed = unsafe { hirari_track_holographic_panner_set_kernel(track.panner, left, right, taps) };
    if changed {
        track.mark_layout();
    }
    changed
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_clear_hrtf_kernel(track: *mut c_void) {
    let Some(track) = track_ref(track) else {
        return;
    };
    track.begin_edit();
    unsafe { hirari_track_holographic_panner_clear_kernel(track.panner) };
    track.mark_layout();
}

/// The audio-thread producer for write automation. The queue is deliberately
/// bounded and SPSC: recording a control-surface gesture must never grow a
/// vector, take an edit lock or allocate inside the callback. Rust gates the
/// write on the recorder mode and the punch range, so the callback only
/// publishes the current fader values of an armed track.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_record_state(track: *mut c_void, playhead: u64) {
    let Some(track) = track_ref(track) else {
        return;
    };
    if !unsafe { hirari_channel_strip_is_record_armed(track.strip) } {
        return;
    }
    let recorded = unsafe {
        hirari_track_automation_store_record(
            track.automation_store,
            hirari_automation_recorder_instance(),
            track.id,
            playhead,
            hirari_channel_strip_gain(track.strip),
            hirari_channel_strip_pan(track.strip),
        )
    };
    if recorded {
        if let Some(revision) = &track.revision {
            revision.realtime_automation_dirty.store(true, Ordering::Release);
        }
    }
}

/// The control-thread consumer: Rust drains the callback queue and appends the
/// captured points to the published volume and pan lists.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_flush_recorded_automation(track: *mut c_void) -> u32 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    let drained = unsafe { hirari_track_automation_store_flush_recorded(track.automation_store) };
    if drained != 0 {
        track.mark_layout();
    }
    drained
}

/// Hands a rendered, immutable region table to the track's realtime runtime.
/// The region store retires the previous generation late, so the pointer stays
/// readable until the next publication. This is the store's own callback rather
/// than a native entry point: the handle it receives is the track object.
unsafe extern "C" fn install_regions(
    track: *mut c_void,
    inputs: *const HirariTrackRegionRenderInput,
    length: usize,
    published: u8,
) {
    if let Some(track) = track_ref(track) {
        unsafe {
            hirari_track_runtime_publish_regions(track.runtime, inputs.cast(), length, published)
        };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_begin(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_channel_strip_begin_pitch_analysis(track.strip)
    })
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_finish(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_channel_strip_finish_pitch_analysis(track.strip) };
    }
}

/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pitch_running(track: *mut c_void) -> bool {
    track_ref(track).is_some_and(|track| unsafe {
        hirari_channel_strip_is_pitch_analysis_running(track.strip)
    })
}

/// Installs one expression map, then marks the layout so the editor and the
/// playback path agree on which map is live.
///
/// # Safety
/// `track` is null or a live track object and `data` holds `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_expression_map_set(
    track: *mut c_void,
    kind: u8,
    data: *const u8,
    length: usize,
) -> bool {
    let Some(track) = track_ref(track) else {
        return false;
    };
    let set =
        unsafe { hirari_track_midi_expression_map_set(track.articulation, kind, data, length) };
    track.mark_layout();
    set
}

/// Reclaims the retired plug-in nodes a chain kept for a callback in flight.
///
/// # Safety
/// `track` is null or a live track object.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_reclaim_chain(track: *mut c_void) {
    if let Some(track) = track_ref(track) {
        unsafe { hirari_effect_chain_reclaim(track.chain) };
    }
}

/// One sandbox slot's health, in the layout the maintenance pass reports.
fn sandbox_status(track: &Track, slot: usize) -> Option<HirariPluginSandboxStatus> {
    let mut raw = SandboxStatusOut::default();
    if !unsafe { hirari_effect_chain_sandbox_status(track.chain, slot, &mut raw) } {
        return None;
    }
    Some(HirariPluginSandboxStatus {
        plugin_index: raw.plugin_index,
        alive: raw.alive != 0,
        can_retry: raw.can_retry != 0,
        failure: raw.failure,
        dropped_output_midi: raw.dropped_output_midi,
        mailbox_overruns: raw.mailbox_overruns,
        input_midi_truncations: raw.input_midi_truncations,
        recovery_mode: u32::from(raw.recovery_mode),
    })
}

fn visit_statuses(track: &Track, context: *mut c_void, visit: Option<StatusVisitorFn>) -> bool {
    let Some(visit) = visit else { return false };
    for slot in 0..unsafe { hirari_effect_chain_sandbox_slot_count(track.chain) } {
        if let Some(status) = sandbox_status(track, slot) {
            if !unsafe { visit(context, track.id, &status) } {
                return false;
            }
        }
    }
    true
}

/// Reports every sandbox slot of one track. The maintenance pass walks the
/// graph under the tracks lock and stops on the first visitor refusal.
///
/// # Safety
/// `track` is null or a live track object; `visit` receives a status that is
/// only valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_visit_sandbox_statuses(
    track: *mut c_void,
    context: *mut c_void,
    visit: Option<StatusVisitorFn>,
) -> bool {
    track_ref(track).is_some_and(|track| visit_statuses(track, context, visit))
}

/// Restarts one track's rejected sandboxes and reports the resulting statuses.
/// A restart can block on a worker process, so the caller holds the track by
/// owning handle and has already released the topology lock.
///
/// # Safety
/// `track` is null or a live track object that outlives this call.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_maintain_sandboxes(
    track: *mut c_void,
    auto_restart: bool,
    context: *mut c_void,
    visit: Option<StatusVisitorFn>,
) -> u32 {
    let Some(track) = track_ref(track) else {
        return 0;
    };
    let recovered = unsafe { hirari_effect_chain_sandbox_maintain(track.chain, auto_restart) };
    if recovered != 0 {
        track.mark_layout();
    }
    visit_statuses(track, context, visit);
    recovered
}
