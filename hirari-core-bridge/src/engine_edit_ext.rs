//! Rust-side entry points for the track-edit calls the C++ `AudioEngine`
//! wrapper used to relay. The wrapper's engine pointer is always valid (it is
//! constructed with the wrapper and never reset), so these helpers resolve it
//! directly and hand it to the same Rust edit policy the wrapper called.

use std::ffi::c_void;

use crate::{ffi, plugin_edit_host, routing_edit_host, track_freeze_host, undo_edit_host};

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
    fn freeze_track(&self, tid: u32, total_samples: u64, sample_rate: u32) -> bool;
    fn freeze_track_to_file(
        &self,
        tid: u32,
        path: &str,
        total_samples: u64,
        sample_rate: u32,
    ) -> bool;
    fn set_track_freeze_cache_path(&self, tid: u32, path: &str) -> bool;
    fn restore_track_freeze_from_file(
        &self,
        tid: u32,
        path: &str,
        total_samples: u64,
        sample_rate: u32,
    ) -> bool;
    fn unfreeze_track(&self, tid: u32) -> bool;
    fn is_track_frozen(&self, tid: u32) -> bool;
    fn freeze_track_to_project_end(&self, tid: u32, sample_rate: u32) -> bool;
    fn get_plugin_state(&self, tid: u32, plugin_index: u32) -> Vec<u8>;
    fn set_plugin_state(&self, tid: u32, plugin_index: u32, state: &[u8]) -> bool;
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

    fn freeze_track(&self, tid: u32, total_samples: u64, sample_rate: u32) -> bool {
        unsafe { track_freeze_host::hirari_track_freeze(host(self), tid, total_samples, sample_rate) }
    }

    fn freeze_track_to_file(
        &self,
        tid: u32,
        path: &str,
        total_samples: u64,
        sample_rate: u32,
    ) -> bool {
        unsafe {
            track_freeze_host::hirari_track_freeze_to_file(
                host(self),
                tid,
                path.as_ptr(),
                path.len(),
                total_samples,
                sample_rate,
            )
        }
    }

    fn set_track_freeze_cache_path(&self, tid: u32, path: &str) -> bool {
        unsafe {
            track_freeze_host::hirari_track_set_freeze_cache_path(
                host(self),
                tid,
                path.as_ptr(),
                path.len(),
            )
        }
    }

    fn restore_track_freeze_from_file(
        &self,
        tid: u32,
        path: &str,
        total_samples: u64,
        sample_rate: u32,
    ) -> bool {
        unsafe {
            track_freeze_host::hirari_track_restore_freeze(
                host(self),
                tid,
                path.as_ptr(),
                path.len(),
                total_samples,
                sample_rate,
            )
        }
    }

    fn unfreeze_track(&self, tid: u32) -> bool {
        unsafe { track_freeze_host::hirari_track_unfreeze(host(self), tid) }
    }

    fn is_track_frozen(&self, tid: u32) -> bool {
        unsafe { track_freeze_host::hirari_track_is_frozen(host(self), tid) }
    }

    fn freeze_track_to_project_end(&self, tid: u32, sample_rate: u32) -> bool {
        unsafe { track_freeze_host::hirari_track_freeze_to_project_end(host(self), tid, sample_rate) }
    }

    fn get_plugin_state(&self, tid: u32, plugin_index: u32) -> Vec<u8> {
        let h = host(self);
        let required = unsafe {
            plugin_edit_host::hirari_plugin_edit_get_state(h, tid, plugin_index, std::ptr::null_mut(), 0)
        };
        if required == 0 {
            return Vec::new();
        }
        let mut state = vec![0u8; required];
        let written = unsafe {
            plugin_edit_host::hirari_plugin_edit_get_state(
                h,
                tid,
                plugin_index,
                state.as_mut_ptr(),
                required,
            )
        };
        if written != required {
            return Vec::new();
        }
        state
    }

    fn set_plugin_state(&self, tid: u32, plugin_index: u32, state: &[u8]) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_set_state(
                host(self),
                tid,
                plugin_index,
                state.as_ptr(),
                state.len(),
            )
        }
    }
}
