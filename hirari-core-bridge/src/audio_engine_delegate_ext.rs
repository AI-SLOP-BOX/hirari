//! Inherent entry points for the engine operations that used to be relayed
//! through two C++ layers: the `AudioEngine` wrapper forwarder and a
//! `HirariUnifiedEngine` method that only called one `hirari_*` function.
//! Both layers are gone; the calls land in the same Rust policy directly.

use std::ffi::c_void;

use crate::{
    control_edit_host, ffi, plugin_edit_host, plugin_sandbox_maintenance, region_edit_host,
    region_jobs,
};

use crate::realtime_vca_manager::{
    hirari_vca_add_group, hirari_vca_clear, hirari_vca_get_cumulative_gain,
    hirari_vca_resolve_hierarchy, hirari_vca_snapshot_json,
};
use crate::region_alignment::HirariRegionAlignmentTargetId;
use crate::region_audio::{
    hirari_float_buffer_data, hirari_float_buffer_destroy, hirari_float_buffer_len,
    hirari_region_audio_channel_count, hirari_region_audio_interleaved_range,
    hirari_region_source_frame_count, hirari_region_source_sample_rate, hirari_region_waveform,
};
use crate::region_time_jobs::{hirari_region_finalize_quantize, hirari_region_quantize_status};
use crate::routing_edit_host::{
    hirari_routing_edit_set_feedback_route, hirari_routing_edit_set_phase_invert,
    hirari_routing_edit_set_route, hirari_routing_edit_set_route_gain,
    hirari_routing_edit_set_send_route, hirari_routing_edit_set_sidechain_link,
    hirari_routing_edit_set_track_armed, hirari_routing_edit_set_track_delay_samples,
};
use crate::track_edit_host::{hirari_track_add, hirari_track_duplicate, hirari_track_remove};

// Scalar field ids shared with the C++ mirror in rust_ffi.hpp.
const SCALAR_CLIP_GAIN: i32 = 1;
const SCALAR_MUTED: i32 = 2;
const SCALAR_REVERSE: i32 = 3;
const SCALAR_PITCH_SEMITONES: i32 = 4;
const SCALAR_LOOP_COUNT: i32 = 5;
const SCALAR_SYNC_GROUP: i32 = 6;
const SCALAR_FADES: i32 = 7;
const SCALAR_PITCH_PRESERVE_WARP: i32 = 9;

fn host(engine: &ffi::AudioEngine) -> *mut c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    core as *mut c_void
}

fn bytes(text: &str) -> *const u8 {
    text.as_ptr()
}

impl ffi::AudioEngine {
    pub fn add_track(&self, t_type: u32) -> u32 {
        unsafe { hirari_track_add(host(self), t_type) }
    }

    pub fn remove_track(&self, id: u32) -> bool {
        unsafe { hirari_track_remove(host(self), id) }
    }

    pub fn duplicate_track(&self, id: u32) -> u32 {
        unsafe { hirari_track_duplicate(host(self), id) }
    }

    pub fn set_track_name(&self, id: u32, name: &str) -> bool {
        unsafe { control_edit_host::hirari_control_edit_rename_track(host(self), id, bytes(name), name.len()) }
    }

    pub fn add_plugin(&self, tid: u32, plugin_type: u32) -> bool {
        unsafe { plugin_edit_host::hirari_plugin_edit_add(host(self), tid, plugin_type) }
    }

    pub fn remove_plugin(&self, tid: u32, plugin_index: u32) -> bool {
        unsafe { plugin_edit_host::hirari_plugin_edit_remove(host(self), tid, plugin_index) }
    }

    pub fn add_sandboxed_plugin(&self, tid: u32, path: &str) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_add_sandboxed(host(self), tid, bytes(path), path.len())
        }
    }

    pub fn set_phase_invert(&self, tid: u32, inverted: bool) -> bool {
        unsafe { hirari_routing_edit_set_phase_invert(host(self), tid, inverted) }
    }

    pub fn set_track_armed(&self, tid: u32, armed: bool) -> bool {
        unsafe { hirari_routing_edit_set_track_armed(host(self), tid, armed) }
    }

    pub fn set_track_delay_samples(&self, tid: u32, samples: u32) -> bool {
        unsafe { hirari_routing_edit_set_track_delay_samples(host(self), tid, samples) }
    }

    pub fn set_route(&self, source_id: u32, dest_id: u32, enabled: bool) -> bool {
        unsafe { hirari_routing_edit_set_route(host(self), source_id, dest_id, enabled) }
    }

    pub fn set_macro_value(&self, macro_index: u32, value: f32) {
        unsafe { control_edit_host::hirari_control_edit_set_macro_value(host(self), macro_index, value) }
    }

    pub fn get_vca_track_gain(&self, track_id: u32) -> f32 {
        hirari_vca_get_cumulative_gain(track_id)
    }

    pub fn clear_vca_groups(&self) {
        hirari_vca_clear()
    }

    pub fn set_expression_map_pro(&self, tid: u32, json: String) -> bool {
        unsafe {
            crate::expression_map_edit_host::hirari_expression_map_edit_set_pro(
                host(self),
                tid,
                bytes(&json),
                json.len(),
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
            )
        }
    }

    pub fn replace_region_audio(&self, tid: u32, rid: u32, path: &str) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_replace_audio(host(self), tid, rid, bytes(path), path.len())
        }
    }

    pub fn complete_region_import(&self, ticket: u64) -> u8 {
        unsafe { region_jobs::hirari_region_import_complete(host(self), ticket) }
    }

    pub fn move_region(&self, tid: u32, rid: u32, start: f64) -> bool {
        unsafe { region_edit_host::hirari_region_edit_move(host(self), tid, rid, start) }
    }

    pub fn move_region_sync_group(&self, tid: u32, rid: u32, start: f64) -> bool {
        unsafe { region_edit_host::hirari_region_edit_move_sync_group(host(self), tid, rid, start) }
    }

    pub fn remove_region(&self, tid: u32, rid: u32) -> bool {
        unsafe { region_edit_host::hirari_region_edit_remove(host(self), tid, rid) }
    }

    pub fn duplicate_region(&self, tid: u32, rid: u32, start_sample: u64) -> u32 {
        unsafe { region_edit_host::hirari_region_duplicate(host(self), tid, rid, start_sample) }
    }

    pub fn set_region_gain(&self, tid: u32, rid: u32, gain: f32) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_CLIP_GAIN, gain, 0.0)
    }

    pub fn set_region_muted(&self, tid: u32, rid: u32, muted: bool) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_MUTED, if muted { 1.0 } else { 0.0 }, 0.0)
    }

    pub fn set_region_fades(&self, tid: u32, rid: u32, fade_in: f32, fade_out: f32) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_FADES, fade_in, fade_out)
    }

    pub fn set_region_reverse(&self, tid: u32, rid: u32, reverse: bool) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_REVERSE, if reverse { 1.0 } else { 0.0 }, 0.0)
    }

    pub fn set_region_trim(&self, tid: u32, rid: u32, start_norm: f32, end_norm: f32) -> bool {
        unsafe { region_edit_host::hirari_region_edit_trim(host(self), tid, rid, start_norm, end_norm) }
    }

    pub fn set_region_warp_ratio(&self, tid: u32, rid: u32, ratio: f64) -> bool {
        unsafe { region_edit_host::hirari_region_edit_warp_ratio(host(self), tid, rid, ratio) }
    }

    pub fn set_region_pitch_semitones(&self, tid: u32, rid: u32, semitones: f32) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_PITCH_SEMITONES, semitones, 0.0)
    }

    pub fn set_region_loop_count(&self, tid: u32, rid: u32, count: u32) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_LOOP_COUNT, f32::from_bits(count), 0.0)
    }

    pub fn set_region_sync_group(&self, tid: u32, rid: u32, group: u32) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_SYNC_GROUP, f32::from_bits(group), 0.0)
    }

    pub fn clear_region_audio_note_segments(&self, tid: u32, rid: u32) -> bool {
        unsafe { region_edit_host::hirari_region_edit_note_clear(host(self), tid, rid) }
    }

    pub fn finalize_region_audio_note_analysis(&self, tid: u32, rid: u32) -> bool {
        unsafe { region_jobs::hirari_region_pitch_analysis_finalize(host(self), tid, rid) }
    }

    pub fn region_quantize_status(&self, tid: u32, rid: u32) -> u8 {
        unsafe { hirari_region_quantize_status(host(self), tid, rid) }
    }

    pub fn finalize_region_quantize(&self, tid: u32, rid: u32) -> bool {
        unsafe { hirari_region_finalize_quantize(host(self), tid, rid) }
    }

    pub fn get_region_source_frame_count(&self, tid: u32, rid: u32) -> u64 {
        unsafe { hirari_region_source_frame_count(host(self), tid, rid) }
    }

    pub fn get_region_sample_rate(&self, tid: u32, rid: u32) -> f64 {
        unsafe { hirari_region_source_sample_rate(host(self), tid, rid) }
    }

    pub fn get_region_channel_count(&self, tid: u32, rid: u32) -> u32 {
        unsafe { hirari_region_audio_channel_count(host(self), tid, rid) }
    }

    pub fn queue_region_import(&self, tid: u32, path: &str, start_sample: u64) -> u64 {
        unsafe { region_jobs::hirari_region_import_queue(host(self), tid, bytes(path), path.len(), start_sample) }
    }

    pub fn set_plugin_parameter(&self, tid: u32, plugin_index: u32, parameter_id: u32, value: f32) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_set_parameter(host(self), tid, plugin_index, parameter_id, value)
        }
    }

    pub fn set_plugin_parameter_without_undo(
        &self,
        tid: u32,
        plugin_index: u32,
        parameter_id: u32,
        value: f32,
    ) -> bool {
        unsafe {
            plugin_edit_host::hirari_plugin_edit_set_parameter_without_undo(
                host(self),
                tid,
                plugin_index,
                parameter_id,
                value,
            )
        }
    }

    pub fn bind_midi_cc_to_macro(
        &self,
        channel: u8,
        cc: u8,
        macro_index: u32,
        minimum: f32,
        maximum: f32,
        curve: f32,
        pickup: bool,
    ) -> bool {
        unsafe {
            control_edit_host::hirari_control_edit_bind_midi_cc(
                host(self),
                channel,
                cc as u16,
                macro_index,
                minimum,
                maximum,
                curve,
                pickup,
            )
        }
    }

    pub fn bind_midi_cc14_to_macro(
        &self,
        channel: u8,
        controller: u16,
        macro_index: u32,
        minimum: f32,
        maximum: f32,
        curve: f32,
        pickup: bool,
    ) -> bool {
        unsafe {
            control_edit_host::hirari_control_edit_bind_midi_cc14(
                host(self),
                channel,
                controller,
                macro_index,
                minimum,
                maximum,
                curve,
                pickup,
            )
        }
    }

    pub fn set_route_gain(&self, source_id: u32, dest_id: u32, gain: f32, enabled: bool) -> bool {
        unsafe { hirari_routing_edit_set_route_gain(host(self), source_id, dest_id, gain, enabled) }
    }

    pub fn set_send_route(&self, source_id: u32, dest_id: u32, gain: f32, pre_fader: bool, enabled: bool) -> bool {
        unsafe { hirari_routing_edit_set_send_route(host(self), source_id, dest_id, gain, pre_fader, enabled) }
    }

    pub fn set_feedback_route(&self, source_id: u32, dest_id: u32, gain: f32, enabled: bool) -> bool {
        unsafe { hirari_routing_edit_set_feedback_route(host(self), source_id, dest_id, gain, enabled) }
    }

    pub fn set_region_pitch_preserve_warp(&self, tid: u32, rid: u32, enabled: bool) -> bool {
        self.set_region_scalar(tid, rid, SCALAR_PITCH_PRESERVE_WARP, if enabled { 1.0 } else { 0.0 }, 0.0)
    }

    pub fn set_region_audio_note_segment(
        &self,
        tid: u32,
        rid: u32,
        start: f64,
        end: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_segment(host(self), tid, rid, start, end, pitch_cents, formant_cents)
        }
    }

    pub fn set_region_audio_note_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        position: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_anchor(
                host(self),
                tid,
                rid,
                segment_start,
                position,
                pitch_cents,
                formant_cents,
            )
        }
    }

    pub fn set_region_audio_note_pitch_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        position: f64,
        pitch_cents: f64,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_pitch_anchor(
                host(self),
                tid,
                rid,
                segment_start,
                position,
                pitch_cents,
            )
        }
    }

    pub fn set_region_audio_note_formant_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        position: f64,
        formant_cents: f64,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_formant_anchor(
                host(self),
                tid,
                rid,
                segment_start,
                position,
                formant_cents,
            )
        }
    }

    pub fn move_region_audio_note_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        old_position: f64,
        new_position: f64,
        value_cents: f64,
        edit_formant: bool,
    ) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_move_anchor(
                host(self),
                tid,
                rid,
                segment_start,
                old_position,
                new_position,
                value_cents,
                edit_formant,
            )
        }
    }

    pub fn warp_region_audio_note_segment(&self, tid: u32, rid: u32, segment_start: f64, new_start: f64, new_end: f64) -> bool {
        unsafe {
            region_edit_host::hirari_region_edit_note_warp(host(self), tid, rid, segment_start, new_start, new_end)
        }
    }

    pub fn remove_region_audio_note_segment(&self, tid: u32, rid: u32, segment_start: f64) -> bool {
        unsafe { region_edit_host::hirari_region_edit_note_remove(host(self), tid, rid, segment_start) }
    }

    pub fn start_region_audio_note_analysis(&self, tid: u32, rid: u32, sample_rate: f64) -> bool {
        unsafe { region_jobs::hirari_region_pitch_analysis_start(host(self), tid, rid, sample_rate) }
    }

    pub fn region_audio_note_analysis_status(&self, tid: u32, rid: u32) -> u8 {
        unsafe { region_jobs::hirari_region_pitch_analysis_status(host(self), tid, rid) }
    }

    pub fn align_regions_to_reference(
        &self,
        reference_tid: u32,
        reference_rid: u32,
        target_tids: &[u32],
        target_rids: &[u32],
    ) -> bool {
        if target_tids.is_empty() || target_tids.len() != target_rids.len() || target_tids.len() > 32 {
            return false;
        }
        let targets: Vec<HirariRegionAlignmentTargetId> = target_tids
            .iter()
            .zip(target_rids)
            .map(|(&track_id, &region_id)| HirariRegionAlignmentTargetId { track_id, region_id })
            .collect();
        unsafe {
            crate::region_time_jobs::hirari_region_align_to_reference(
                host(self),
                reference_tid,
                reference_rid,
                targets.as_ptr(),
                targets.len(),
            )
        }
    }

    pub fn region_alignment_status(&self, reference_tid: u32, reference_rid: u32) -> u8 {
        unsafe { crate::region_time_jobs::hirari_region_alignment_status(host(self), reference_tid, reference_rid) }
    }

    pub fn finalize_region_alignment(&self, reference_tid: u32, reference_rid: u32) -> bool {
        unsafe { crate::region_time_jobs::hirari_region_finalize_alignment(host(self), reference_tid, reference_rid) }
    }

    pub fn quantize_region_sync_group(&self, tid: u32, rid: u32, strength: f32, grid_beats: f64, swing: f32) -> bool {
        unsafe {
            crate::region_time_jobs::hirari_region_quantize_sync_group(
                host(self),
                tid,
                rid,
                strength,
                grid_beats,
                swing,
            )
        }
    }

    pub fn set_expression_map(&self, tid: u32, packed: Vec<u32>, json: String, articulation_id: u32) -> bool {
        unsafe {
            crate::expression_map_edit_host::hirari_expression_map_edit_set(
                host(self),
                tid,
                packed.as_ptr(),
                packed.len(),
                bytes(&json),
                json.len(),
                articulation_id,
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
            )
        }
    }

    fn set_region_scalar(&self, tid: u32, rid: u32, field: i32, a: f32, b: f32) -> bool {
        unsafe { region_edit_host::hirari_region_edit_scalar(host(self), tid, rid, field, a, b) }
    }
}

/// Rust hands out owned float buffers; copying and releasing one here keeps the
/// single decoder path in Rust instead of keeping a C++ mirror of it.
fn take_float_buffer(buffer: *mut c_void) -> Vec<f32> {
    if buffer.is_null() {
        return Vec::new();
    }
    let (data, count) = unsafe {
        (
            hirari_float_buffer_data(buffer),
            hirari_float_buffer_len(buffer),
        )
    };
    let mut samples = Vec::new();
    if !data.is_null() && count != 0 {
        samples.extend_from_slice(unsafe { std::slice::from_raw_parts(data, count) });
    }
    unsafe { hirari_float_buffer_destroy(buffer) };
    samples
}

impl ffi::AudioEngine {
    pub fn add_vca_group(&self, group_id: u32, gain: f32) -> bool {
        if !hirari_vca_add_group(group_id, gain) {
            return false;
        }
        hirari_vca_resolve_hierarchy();
        true
    }

    pub fn set_vca_group_gain(&self, group_id: u32, gain: f32) -> bool {
        if !hirari_vca_add_group(group_id, gain) {
            return false;
        }
        hirari_vca_resolve_hierarchy();
        true
    }

    pub fn get_vca_snapshot_json(&self) -> String {
        let required = unsafe { hirari_vca_snapshot_json(std::ptr::null_mut(), 0) };
        if required == 0 {
            return "[]".to_string();
        }
        let mut buffer = vec![0u8; required];
        unsafe { hirari_vca_snapshot_json(buffer.as_mut_ptr(), required) };
        let end = buffer.iter().position(|&b| b == 0).unwrap_or(required);
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    }

    pub fn split_region(&self, tid: u32, rid: u32, split: f64) -> bool {
        self.split_region_with_right_id(tid, rid, split) != 0
    }

    pub fn split_region_with_right_id(&self, tid: u32, rid: u32, split: f64) -> u32 {
        unsafe { region_edit_host::hirari_region_split(host(self), tid, rid, split) }
    }

    pub fn get_region_waveform(&self, tid: u32, rid: u32) -> Vec<f32> {
        take_float_buffer(unsafe { hirari_region_waveform(host(self), tid, rid, 128) })
    }

    pub fn get_region_audio_interleaved_range(
        &self,
        tid: u32,
        rid: u32,
        start_frame: u64,
        frames: u32,
        channels: u32,
    ) -> Vec<f32> {
        take_float_buffer(unsafe {
            hirari_region_audio_interleaved_range(
                host(self),
                tid,
                rid,
                start_frame,
                frames,
                channels,
            )
        })
    }

    pub fn maintain_sandboxed_plugins(&self, auto_restart: bool) -> u32 {
        unsafe { plugin_sandbox_maintenance::hirari_plugin_sandbox_maintain(host(self), auto_restart) }
    }

    pub fn set_track_input_monitor_channels(
        &self,
        tid: u32,
        enabled: bool,
        left: u32,
        right: u32,
    ) -> bool {
        unsafe {
            control_edit_host::hirari_control_edit_set_input_monitor(
                host(self),
                tid,
                enabled,
                left,
                right,
            )
        }
    }

    pub fn set_sidechain_link(
        &self,
        source_id: u32,
        dest_id: u32,
        plugin_index: u32,
        tap_point: u32,
        enabled: bool,
    ) -> bool {
        unsafe {
            hirari_routing_edit_set_sidechain_link(
                host(self),
                source_id,
                dest_id,
                plugin_index,
                tap_point,
                enabled,
            )
        }
    }
}
