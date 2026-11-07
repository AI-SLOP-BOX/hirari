//! Rust-side entry points for the track-edit calls the C++ `AudioEngine`
//! wrapper used to relay. The wrapper's engine pointer is always valid (it is
//! constructed with the wrapper and never reset), so these helpers resolve it
//! directly and hand it to the same Rust edit policy the wrapper called.

use std::ffi::c_void;

use crate::automation_edit_host::{
    HIRARI_AUTOMATION_PAN, HIRARI_AUTOMATION_TRACK_DELAY, HIRARI_AUTOMATION_VOLUME,
};
use crate::{
    automation_edit_host, ffi, plugin_edit_host, region_edit_host, routing_edit_host,
    track_control_host, track_freeze_host, undo_edit_host,
};

fn host(engine: &crate::AudioEngine) -> *mut c_void {
    let core: *const crate::HirariUnifiedEngine = crate::get_unified_engine(engine);
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
    fn set_plugin_automation(
        &self,
        tid: u32,
        plugin_index: u32,
        parameter_id: u32,
        packed_points: Vec<f64>,
    ) -> bool;
    fn save_plugin_preset(&self, tid: u32, plugin_index: u32, path: &str) -> bool;
    fn load_plugin_preset(&self, tid: u32, plugin_index: u32, path: &str) -> bool;
    fn set_region_comp_ranges(
        &self,
        tid: u32,
        rid: u32,
        packed_ranges: &[u64],
        managed: bool,
    ) -> bool;
    fn set_region_range_edit(
        &self,
        tid: u32,
        rid: u32,
        start: u64,
        end: u64,
        gain: f32,
        fade_in: u64,
        fade_out: u64,
    ) -> bool;
    fn clear_region_range_edits(&self, tid: u32, rid: u32) -> bool;
    fn set_spatial_position(&self, tid: u32, x: f32, y: f32, z: f32) -> bool;
    fn set_hrtf_kernel(&self, tid: u32, left: Vec<f32>, right: Vec<f32>) -> bool;
    fn clear_hrtf_kernel(&self, tid: u32) -> bool;
    fn set_automation_data(&self, tid: u32, param_id: u32, packed_points: Vec<f64>) -> bool;
    fn set_track_delay_automation(&self, tid: u32, packed_points: Vec<f64>) -> bool;
    fn undo(&self);
    fn redo(&self);
    fn is_undo_transaction_active(&self) -> bool;
    fn begin_undo_transaction(&self, name: &str);
    fn end_undo_transaction(&self) -> bool;
    fn abort_undo_transaction(&self) -> bool;
    fn get_undo_count(&self) -> u32;
    fn get_redo_count(&self) -> u32;
}

impl TrackEditExt for crate::AudioEngine {
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

    fn set_plugin_automation(
        &self,
        tid: u32,
        plugin_index: u32,
        parameter_id: u32,
        packed_points: Vec<f64>,
    ) -> bool {
        unsafe {
            automation_edit_host::hirari_automation_edit_plugin(
                host(self),
                tid,
                plugin_index,
                parameter_id,
                packed_points.as_ptr(),
                packed_points.len(),
            )
        }
    }

    fn save_plugin_preset(&self, tid: u32, plugin_index: u32, path: &str) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_save_preset(
                host(self),
                tid,
                plugin_index,
                path.as_ptr(),
                path.len(),
            )
        }
    }

    fn load_plugin_preset(&self, tid: u32, plugin_index: u32, path: &str) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_load_preset(
                host(self),
                tid,
                plugin_index,
                path.as_ptr(),
                path.len(),
            )
        }
    }

    fn set_region_comp_ranges(
        &self,
        tid: u32,
        rid: u32,
        packed_ranges: &[u64],
        managed: bool,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_comp_ranges_set(
                host(self),
                tid,
                rid,
                packed_ranges.as_ptr(),
                packed_ranges.len(),
                managed,
            )
        }
    }

    fn set_region_range_edit(
        &self,
        tid: u32,
        rid: u32,
        start: u64,
        end: u64,
        gain: f32,
        fade_in: u64,
        fade_out: u64,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_range_edit_set(
                host(self),
                tid,
                rid,
                start,
                end,
                gain,
                fade_in,
                fade_out,
            )
        }
    }

    fn clear_region_range_edits(&self, tid: u32, rid: u32) -> bool {
        unsafe { region_edit_host::hirari_region_edit_range_edits_clear(host(self), tid, rid) }
    }

    fn set_spatial_position(&self, tid: u32, x: f32, y: f32, z: f32) -> bool {
        unsafe { track_control_host::hirari_track_set_spatial_position(host(self), tid, x, y, z) }
    }

    fn set_hrtf_kernel(&self, tid: u32, left: Vec<f32>, right: Vec<f32>) -> bool {
        unsafe {
            track_control_host::hirari_track_set_hrtf_kernel(
                host(self),
                tid,
                left.as_ptr(),
                left.len(),
                right.as_ptr(),
                right.len(),
            )
        }
    }

    fn clear_hrtf_kernel(&self, tid: u32) -> bool {
        unsafe { track_control_host::hirari_track_clear_hrtf_kernel(host(self), tid) }
    }

    fn set_automation_data(&self, tid: u32, param_id: u32, packed_points: Vec<f64>) -> bool {
        let kind = if param_id == 0 {
            HIRARI_AUTOMATION_VOLUME
        } else {
            HIRARI_AUTOMATION_PAN
        };
        unsafe {
            automation_edit_host::hirari_automation_edit_track(
                host(self),
                tid,
                kind,
                param_id,
                packed_points.as_ptr(),
                packed_points.len(),
            )
        }
    }

    fn set_track_delay_automation(&self, tid: u32, packed_points: Vec<f64>) -> bool {
        unsafe {
            automation_edit_host::hirari_automation_edit_track(
                host(self),
                tid,
                HIRARI_AUTOMATION_TRACK_DELAY,
                0,
                packed_points.as_ptr(),
                packed_points.len(),
            )
        }
    }

    fn undo(&self) {
        unsafe {
            undo_edit_host::hirari_undo_edit_undo(
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
                host(self),
            )
        };
    }

    fn redo(&self) {
        unsafe {
            undo_edit_host::hirari_undo_edit_redo(
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
                host(self),
            )
        };
    }

    fn is_undo_transaction_active(&self) -> bool {
        unsafe { undo_edit_host::hirari_undo_edit_transaction_active(self.undo_state() as *const c_void) }
    }

    fn begin_undo_transaction(&self, name: &str) {
        unsafe {
            undo_edit_host::hirari_undo_edit_begin_transaction(
                self.undo_state() as *mut c_void,
                name.as_ptr(),
                name.len(),
            )
        };
    }

    fn end_undo_transaction(&self) -> bool {
        unsafe { undo_edit_host::hirari_undo_edit_end_transaction(self.undo_state() as *mut c_void) }
    }

    fn abort_undo_transaction(&self) -> bool {
        unsafe {
            undo_edit_host::hirari_undo_edit_abort_transaction(
                self.undo_state() as *mut c_void,
                host(self),
            )
        }
    }

    fn get_undo_count(&self) -> u32 {
        unsafe { undo_edit_host::hirari_undo_edit_undo_count(self.undo_state() as *const c_void) }
    }

    fn get_redo_count(&self) -> u32 {
        unsafe { undo_edit_host::hirari_undo_edit_redo_count(self.undo_state() as *const c_void) }
    }
}
