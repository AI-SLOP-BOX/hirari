//! Inherent entry points for the engine operations that used to be relayed
//! through two C++ layers: the `AudioEngine` wrapper forwarder and a
//! `HirariUnifiedEngine` method that only called one `hirari_*` function.
//! Both layers are gone; the calls land in the same Rust policy directly.

use std::ffi::c_void;

use crate::{
    control_edit_host, ffi, layout_gather, plugin_edit_host, plugin_sandbox_maintenance,
    region_edit_host, region_jobs,
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
use crate::undo_edit_host::hirari_engine_sync_structural_changes;

// Scalar field ids shared with the C++ mirror in rust_ffi.hpp.
const SCALAR_CLIP_GAIN: i32 = 1;
const SCALAR_MUTED: i32 = 2;
const SCALAR_REVERSE: i32 = 3;
const SCALAR_PITCH_SEMITONES: i32 = 4;
const SCALAR_LOOP_COUNT: i32 = 5;
const SCALAR_SYNC_GROUP: i32 = 6;
const SCALAR_FADES: i32 = 7;
const SCALAR_PITCH_PRESERVE_WARP: i32 = 9;

fn core_mut(engine: &ffi::AudioEngine) -> *mut ffi::HirariUnifiedEngine {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    core as *mut ffi::HirariUnifiedEngine
}

fn pinned(engine: &ffi::AudioEngine) -> core::pin::Pin<&mut ffi::HirariUnifiedEngine> {
    unsafe { core::pin::Pin::new_unchecked(&mut *core_mut(engine)) }
}

pub(crate) fn host(engine: &ffi::AudioEngine) -> *mut c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    core as *mut c_void
}

/// Address of the Rust driver host the engine constructed, for the reads in
/// `audio_driver_view`. Zero means no host -- off macOS there is never one, and
/// on macOS it is the absent host the deleted C++ null guards named.
pub(crate) fn driver_host(engine: &ffi::AudioEngine) -> usize {
    engine.audio_driver_host()
}

fn learn_state(engine: &ffi::AudioEngine) -> *const c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).midi_learn_state() as *const c_void }
}

fn preview_state(engine: &ffi::AudioEngine) -> *mut c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).preview_synth_state() as *mut c_void }
}

fn pdc_state(engine: &ffi::AudioEngine) -> *const c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).pdc_state() as *const c_void }
}

fn preview_sampler(engine: &ffi::AudioEngine) -> *const c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).preview_sampler() as *const c_void }
}

fn metronome(engine: &ffi::AudioEngine) -> *mut c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).metronome_state() as *mut c_void }
}

fn routing_engine(engine: &ffi::AudioEngine) -> *const c_void {
    let core: *const ffi::HirariUnifiedEngine = ffi::get_unified_engine(engine);
    unsafe { (*core).routing_engine_state() as *const c_void }
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

    /// Background layout snapshots. The session, its cancellation flag and the
    /// revisions around the gather all live in `layout_gather`; C++ contributes
    /// only the engine host and the ops table it reads through.
    pub fn request_project_layout_snapshot(&self) -> u64 {
        layout_gather::request_layout_snapshot(core_mut(self))
    }

    pub fn poll_project_layout_snapshot(&self, request_id: u64) -> String {
        layout_gather::poll_layout_snapshot(core_mut(self) as *const _, request_id)
    }

    pub fn cancel_project_layout_snapshot(&self, request_id: u64) -> bool {
        layout_gather::cancel_layout_snapshot(core_mut(self) as *const _, request_id)
    }

    /// The live project layout as JSON, gathered without a C++ wrapper.
    pub fn project_layout_json(&self) -> String {
        layout_gather::project_layout_json(core_mut(self) as *const _)
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

    pub fn set_track_input_monitor(&self, tid: u32, enabled: bool) -> bool {
        self.set_track_input_monitor_channels(tid, enabled, 0, 1)
    }

    pub fn move_plugin(&self, tid: u32, from_index: u32, to_index: u32) -> bool {
        if !unsafe {
            plugin_edit_host::hirari_plugin_edit_move(host(self), tid, from_index, to_index)
        } {
            return false;
        }
        unsafe { hirari_engine_sync_structural_changes(host(self)) };
        true
    }

    pub fn handle_midi_cc(&self, channel: u8, cc: u8, value: u8) {
        unsafe {
            crate::midi_learn::hirari_midi_learn_handle_cc(
                learn_state(self),
                channel,
                cc,
                value,
            )
        };
    }

    pub fn handle_midi_cc14(&self, channel: u8, controller: u16, value: u16) {
        unsafe {
            crate::midi_learn::hirari_midi_learn_handle_cc14(
                learn_state(self),
                channel,
                controller,
                value,
            )
        };
    }

    pub fn set_preview_synth_engine(&self, engine: u32) {
        unsafe { crate::preview_synth::hirari_preview_synth_set_engine(preview_state(self), engine) };
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

// Simple getters read the inner engine through `get_core`; the AudioEngine
// forwarders for them are gone. Transport reads/writes go to
// `engine_transport`, which drives the Rust-owned engine-block store directly.
impl ffi::AudioEngine {
    pub fn is_loop_enabled(&self) -> bool {
        crate::engine_transport::loop_enabled(self.get_core())
    }

    pub fn get_master_gain(&self) -> f32 {
        self.get_core().get_master_gain()
    }

    pub fn get_track_volume(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_volume(self.get_core(), tid)
    }
}

// Read-only engine getters. The AudioEngine forwarders for these are gone:
// Rust reaches the same inner engine through `get_core`.
impl ffi::AudioEngine {
    pub fn cycle_start(&self) -> u64 {
        crate::engine_transport::cycle_start(self.get_core())
    }
    pub fn cycle_end(&self) -> u64 {
        crate::engine_transport::cycle_end(self.get_core())
    }
    pub fn is_metronome_enabled(&self) -> bool {
        unsafe { crate::metronome_renderer::hirari_metronome_is_enabled(metronome(self)) }
    }
    pub fn set_metronome_enabled(&self, enabled: bool) {
        unsafe { crate::metronome_renderer::hirari_metronome_set_enabled(metronome(self), enabled) };
        self.mark_project_state_changed();
    }
    pub fn get_playhead(&self) -> u64 {
        crate::engine_transport::playhead(self.get_core())
    }
    pub fn samples_to_beats(&self, samples: u64) -> f64 {
        unsafe {
            crate::tempo::hirari_tempo_map_samples_to_beats(
                self.tempo_state() as *const c_void,
                samples,
                self.get_core().get_sample_rate(),
            )
        }
    }
    pub fn beats_to_samples(&self, beats: f64) -> u64 {
        unsafe {
            crate::tempo::hirari_tempo_map_beats_to_samples(
                self.tempo_state() as *const c_void,
                beats,
                self.get_core().get_sample_rate(),
            )
        }
    }
    pub fn get_routing_snapshot_json(&self) -> String {
        unsafe {
            let json = crate::routing_engine_core::hirari_routing_engine_connections_json(
                routing_engine(self),
            );
            if json.is_null() {
                return "[]".to_owned();
            }
            let text = std::ffi::CStr::from_ptr(json).to_string_lossy().into_owned();
            crate::routing_engine_core::hirari_routing_engine_connections_json_free(json);
            text
        }
    }
    pub fn midi_clock_ticks(&self) -> u64 {
        self.get_core().midi_clock_ticks()
    }
    pub fn midi_clock_last_tick(&self) -> u64 {
        self.get_core().midi_clock_last_tick()
    }
    pub fn midi_clock_rate(&self) -> f64 {
        self.get_core().midi_clock_rate()
    }
    pub fn bus_track_exists(&self, bus_track_id: u32) -> bool {
        self.get_core().bus_track_exists(bus_track_id)
    }
    pub fn get_track_delay_samples(&self, tid: u32) -> u32 {
        crate::engine_analysis_reads::track_delay_samples(self.get_core(), tid)
    }
    pub fn get_latency_ms(&self) -> f32 {
        crate::engine_analysis_reads::latency_ms(self.get_core())
    }
    pub fn low_latency_mode(&self) -> bool {
        unsafe { crate::native_pdc::hirari_native_pdc_low_latency_mode(pdc_state(self)) }
    }
    pub fn set_low_latency_mode(&self, active: bool) -> bool {
        unsafe { crate::native_pdc::hirari_native_pdc_set_low_latency_mode(pdc_state(self), active) }
    }
    pub fn get_tempo(&self) -> f32 {
        unsafe { crate::tempo::hirari_tempo_map_get_current_bpm(self.tempo_state() as *const c_void) }
    }
    pub fn trigger_preview_sample(&self) {
        unsafe { crate::preview_sampler::hirari_preview_sampler_trigger(preview_sampler(self)) }
    }
    pub fn set_preview_sample(&self, samples: &[f32], source_rate: f64) {
        crate::engine_transport::set_preview_sample(self.get_core(), samples, source_rate);
    }
    pub fn non_finite_plugin_samples(&self) -> u64 {
        crate::engine_analysis_reads::non_finite_plugin_samples(self.get_core())
    }
    pub fn get_sandbox_plugin_state_error(&self, track_id: u32, sandbox_index: u32) -> u8 {
        crate::plugin_sandbox_maintenance::sandbox_state_error(
            host(self),
            track_id,
            sandbox_index,
        )
        .0
    }
    pub fn get_plugin_parameter(&self, tid: u32, plugin_index: u32, parameter_id: u32) -> f32 {
        crate::engine_analysis_reads::plugin_parameter(self.get_core(), tid, plugin_index, parameter_id)
    }
    pub fn get_plugin_parameter_count(&self, tid: u32, plugin_index: u32) -> u32 {
        crate::engine_analysis_reads::plugin_parameter_count(self.get_core(), tid, plugin_index)
    }
    pub fn get_plugin_bypass(&self, tid: u32, plugin_index: u32) -> bool {
        crate::engine_analysis_reads::plugin_bypass(self.get_core(), tid, plugin_index)
    }
    pub fn get_track_latency_ms(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_latency_ms(self.get_core(), tid)
    }
    pub fn get_track_pdc_compensation_ms(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_pdc_compensation_ms(self.get_core(), tid)
    }
    pub fn has_sidechain_link(&self, source_id: u32, dest_id: u32, plugin_index: u32) -> bool {
        self.get_core()
            .has_sidechain_link(source_id, dest_id, plugin_index)
    }
    pub fn get_bounce_progress(&self) -> f32 {
        self.get_core().get_bounce_progress()
    }
    pub fn get_bounce_state(&self) -> u32 {
        self.get_core().get_bounce_state()
    }
    pub fn audio_range_overflowed(&self) -> bool {
        crate::engine_transport::range_overflowed(self.get_core())
    }
    pub fn get_video_frame_revision(&self) -> u64 {
        crate::video_system::video_service().frame_revision()
    }
    pub fn get_track_correlation(&self, tid: u32) -> f32 {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::get_track_correlation(host, tid)
    }
    pub fn assign_track_to_vca(&self, track_id: u32, group_id: u32) -> bool {
        crate::engine_transport::assign_track_to_vca(self.get_core(), track_id, group_id)
    }
    pub fn new_project(&self) {
        pinned(self).new_project()
    }
    pub fn take_watchdog_trips(&self) -> u32 {
        crate::engine_analysis_reads::watchdog_trips(self.get_core())
    }
    pub fn retry_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        crate::engine_analysis_reads::retry_sandboxed_plugin(self.get_core(), track_id, sandbox_index)
    }
    pub fn restart_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        crate::engine_analysis_reads::restart_sandboxed_plugin(
            self.get_core(),
            track_id,
            sandbox_index,
        )
    }
    pub fn set_plugin_bypass(&self, tid: u32, plugin_index: u32, bypassed: bool) -> bool {
        pinned(self).set_plugin_bypass(tid, plugin_index, bypassed)
    }
    pub fn get_plugin_parameter_name(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
    ) -> String {
        crate::engine_analysis_reads::plugin_parameter_name(
            self.get_core(),
            track_id,
            plugin_index,
            parameter_id,
        )
    }
    pub fn cancel_bounce(&self) -> bool {
        pinned(self).cancel_bounce()
    }
    pub fn pause_bounce(&self) -> bool {
        pinned(self).pause_bounce()
    }
    pub fn resume_bounce(&self) -> bool {
        pinned(self).resume_bounce()
    }
    pub fn execute_vocal_remover(&self, tid: u32) -> bool {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::execute_vocal_remover(host, tid)
    }
    pub fn set_articulation_map(&self, tid: u32, map_hash: u32) -> bool {
        pinned(self).set_articulation_map(tid, map_hash)
    }
    pub fn set_track_eq(
        &self,
        tid: u32,
        low_boost_db: f32,
        low_cut_db: f32,
        high_boost_db: f32,
        high_cut_db: f32,
    ) -> bool {
        crate::engine_analysis_reads::set_track_eq(
            self.get_core(),
            tid,
            low_boost_db,
            low_cut_db,
            high_boost_db,
            high_cut_db,
        )
    }
    pub fn execute_auto_arrangement(&self) -> bool {
        pinned(self).execute_auto_arrangement()
    }
    pub fn set_project_scale(&self, root: i32, scale_type: i32) -> bool {
        pinned(self).set_project_scale(root, scale_type)
    }
    pub fn clear_region_processing(&self, tid: u32, rid: u32) -> bool {
        pinned(self).clear_region_processing(tid, rid)
    }
    pub fn set_automation_record_mode(&self, mode: u32) -> bool {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::set_automation_record_mode(host, mode)
    }
    pub fn request_video_frame(&self, seconds: f64) -> bool {
        crate::video_system::video_service().request_frame(seconds)
    }
    pub fn mark_project_layout_changed(&self) {
        self.get_core().mark_project_layout_changed()
    }
    pub fn mark_project_state_changed(&self) {
        pinned(self).mark_project_state_changed()
    }
    pub fn set_loop(&self, enabled: bool) {
        crate::engine_transport::set_loop(self.get_core(), enabled);
        self.mark_project_state_changed();
    }
    pub fn set_cycle_range(&self, start_sample: u64, end_sample: u64, enabled: bool) -> bool {
        if !crate::engine_transport::set_cycle_range(self.get_core(), start_sample, end_sample, enabled) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn set_playhead(&self, pos: u64) {
        crate::engine_transport::set_playhead(self.get_core(), pos)
    }
    pub fn set_test_tone(&self, enabled: bool) {
        crate::engine_transport::set_test_tone(self.get_core(), enabled)
    }
    pub fn set_master_gain(&self, value: f32) -> bool {
        if !pinned(self).set_master_gain(value) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn set_offline_render_target(&self, tid: u32) -> bool {
        crate::engine_analysis_reads::set_offline_target(self.get_core(), tid)
    }
    pub fn clear_offline_render_target(&self) {
        crate::engine_analysis_reads::clear_offline_target(self.get_core());
    }
    pub fn set_offline_render_tail_seconds(&self, seconds: f32) {
        crate::engine_analysis_reads::set_offline_tail_seconds(self.get_core(), seconds);
    }
    pub fn set_offline_render_options(&self, pre_fader: bool, include_inserts: bool) {
        crate::engine_analysis_reads::set_offline_options(
            self.get_core(),
            pre_fader,
            include_inserts,
        );
    }
    pub fn get_project_layout_revision(&self) -> u64 {
        layout_gather::project_layout_revision(core_mut(self) as *const _)
    }

    pub fn get_project_state_revision(&self) -> u64 {
        layout_gather::project_state_revision(core_mut(self) as *const _)
    }
    pub fn is_playing(&self) -> bool {
        crate::engine_transport::is_playing(self.get_core())
    }
    pub fn set_tempo(&self, bpm: f32) -> bool {
        let core = self.get_core();
        if !core.set_tempo(bpm) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn midi_clock_tick(&self, timestamp: u64) {
        self.get_core().midi_clock_tick(timestamp)
    }
    pub fn set_midi_clock_rate(&self, bpm: f64) {
        self.get_core().set_midi_clock_rate(bpm)
    }
    pub fn get_sample_rate(&self) -> f64 {
        self.get_core().get_sample_rate()
    }
    pub fn set_region_warp_markers(
        &self,
        tid: u32,
        rid: u32,
        source_samples: &[u64],
        timeline_samples: &[u64],
        transient_flags: &[u8],
    ) -> bool {
        // The slice agreement checks, marker cap and marker layout live in
        // region_edit_host; this only hands it the engine host handle.
        unsafe {
            crate::region_edit_host::hirari_region_edit_set_warp_markers(
                host(self),
                tid,
                rid,
                source_samples.as_ptr(),
                source_samples.len(),
                timeline_samples.as_ptr(),
                timeline_samples.len(),
                transient_flags.as_ptr(),
                transient_flags.len(),
            )
        }
    }
    pub fn get_sandbox_statuses(&self) -> Vec<u32> {
        plugin_sandbox_maintenance::sandbox_statuses_packed(host(self) as usize)
    }
    pub fn read_wav_diagnostic_json(&self, path: &str, format: u32) -> String {
        unsafe {
            let raw = crate::wav_diagnostic::hirari_wav_diagnostic_json_for_path(
                path.as_ptr(),
                path.len(),
                format == 1,
            );
            if raw.is_null() {
                return String::new();
            }
            let data = crate::wav_diagnostic::hirari_wav_diagnostic_json_data(raw);
            let size = crate::wav_diagnostic::hirari_wav_diagnostic_json_size(raw);
            let text = if data.is_null() {
                String::new()
            } else {
                String::from_utf8_lossy(std::slice::from_raw_parts(data, size)).into_owned()
            };
            crate::wav_diagnostic::hirari_wav_diagnostic_json_free(raw);
            text
        }
    }
    pub fn poll_region_waveform(&self, request: u64) -> Vec<f32> {
        unsafe extern "C" fn collect(context: *mut c_void, samples: *const f32, count: usize) {
            if samples.is_null() || count == 0 {
                return;
            }
            // SAFETY: the poll visitor only runs for the duration of the call
            // and the context is the local vector below.
            let output = unsafe { &mut *(context as *mut Vec<f32>) };
            output.extend_from_slice(unsafe { std::slice::from_raw_parts(samples, count) });
        }
        let mut result = Vec::new();
        if request == 0 {
            return result;
        }
        unsafe {
            crate::async_snapshots::hirari_waveform_snapshot_poll(
                request,
                &mut result as *mut Vec<f32> as *mut c_void,
                Some(collect),
            );
        }
        result
    }
    pub fn region_waveform_pending(&self, request: u64) -> bool {
        request != 0 && unsafe { crate::async_snapshots::hirari_waveform_snapshot_pending(request) }
    }
}
