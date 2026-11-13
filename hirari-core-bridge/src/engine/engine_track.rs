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

#[path = "engine_layout_revision.rs"]
mod layout_revision;
pub use layout_revision::*;
#[path = "engine_track_ffi.rs"]
mod track_ffi;
pub use track_ffi::*;

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
            strip: { hirari_channel_strip_create() },
            chain: { hirari_effect_chain_create() },
            plugin_midi: { hirari_midi_buffer_create() },
            articulation: { hirari_track_midi_articulation_create() },
            built_in_midi: if kind == TRACK_TYPE_MIDI || kind == TRACK_TYPE_INSTRUMENT {
                hirari_builtin_midi_instrument_create()
            } else {
                std::ptr::null_mut()
            },
            pdc_delay: { hirari_track_pdc_delay_create() },
            runtime: std::ptr::null_mut(),
            frozen_store: { hirari_track_frozen_store_create() },
            automation_store: { hirari_track_automation_store_create() },
            name_store: unsafe { hirari_track_name_store_create() },
            panner: { hirari_track_holographic_panner_create() },
            work: { hirari_audio_buffer_storage_create() },
            pre_fader: { hirari_audio_buffer_storage_create() },
            send_pre: { hirari_audio_buffer_storage_create() },
            send_post: { hirari_audio_buffer_storage_create() },
            pre_insert: { hirari_audio_buffer_storage_create() },
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


