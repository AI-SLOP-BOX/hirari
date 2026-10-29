//! Rust-side entry points for the track-edit calls the C++ `AudioEngine`
//! wrapper used to relay. The wrapper's engine pointer is always valid (it is
//! constructed with the wrapper and never reset), so these helpers resolve it
//! directly and hand it to the same Rust edit policy the wrapper called.

use std::ffi::c_void;

use crate::{ffi, routing_edit_host, undo_edit_host};

fn host(engine: &ffi::AudioEngine) -> *mut c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    core as *mut c_void
}

pub trait TrackEditExt {
    fn set_track_volume(&self, tid: u32, value: f32) -> bool;
    fn set_track_pan(&self, tid: u32, value: f32) -> bool;
    fn set_track_mute(&self, tid: u32, muted: bool) -> bool;
    fn set_track_solo(&self, tid: u32, solo: bool) -> bool;
    fn clear_undo_history(&self);
}

impl TrackEditExt for ffi::AudioEngine {
    fn set_track_volume(&self, tid: u32, value: f32) -> bool {
        unsafe { routing_edit_host::hirari_routing_edit_set_track_volume(host(self), tid, value) }
    }

    fn set_track_pan(&self, tid: u32, value: f32) -> bool {
        unsafe { routing_edit_host::hirari_routing_edit_set_track_pan(host(self), tid, value) }
    }

    fn set_track_mute(&self, tid: u32, muted: bool) -> bool {
        unsafe { routing_edit_host::hirari_routing_edit_set_track_mute(host(self), tid, muted) }
    }

    fn set_track_solo(&self, tid: u32, solo: bool) -> bool {
        unsafe { routing_edit_host::hirari_routing_edit_set_track_solo(host(self), tid, solo) }
    }

    fn clear_undo_history(&self) {
        unsafe { undo_edit_host::hirari_undo_edit_clear(self.undo_state() as *mut c_void) };
    }
}
