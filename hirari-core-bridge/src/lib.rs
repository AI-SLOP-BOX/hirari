// DSP kernels intentionally use index-based loops for fixed channel/lane
// processing and preallocated buffers. Keep these layouts explicit rather
// than forcing iterator rewrites that can obscure real-time data access.
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![recursion_limit = "512"]

// The native bridge accepts macOS plugin bundles; the isolated worker
// resolves the Mach-O image inside the bundle before loading it.

include!("module_exports.rs");
pub use analysis_views::{
    analysis_arrangement_advice, analysis_creative_advice, analysis_dashboard_json,
    analysis_mel_spectrogram, analysis_mixing_advice, analysis_motion_energy,
    analysis_motion_vectors, analysis_partials, analysis_phase_heatmap,
    analysis_song_structure_json, analysis_spectral_clashes, analysis_spectral_display,
    analysis_structure_sections as build_analysis_structure_sections, analysis_synesthesia_colors,
};
pub use runtime_views::{interleave_planar_audio_input, runtime_health_vector};
pub use waveform_cache::{analysis_waveform_peak_envelope, resample_waveform_peak_envelope};
pub mod ara2_protocol;
pub mod bridge_error;
pub mod command_api;
pub mod command_executor;
pub mod custom_ui;
pub mod generation_gate;
pub mod job_system;
pub mod production_events;
pub mod production_session;
pub mod production_timeline;
pub mod regions;
pub mod spatial_orchestrator;
pub mod stable_api;
pub mod vfx_bindings;
pub mod vfx_timeline_bridge;
pub mod vocal_quality;
use anyhow::Context;
use serde::Deserialize;
use std::path::{Path, PathBuf};
include!("module_exports_tail.rs");
use crate::summing::{SummingConfig, SummingOrchestrator};
use persistence::*;
use project::ProjectDocument;

#[cxx::bridge(namespace = "Hirari::Core::Bridge")]
pub mod ffi {
    #[derive(Debug, Clone, Copy)]
    pub struct BridgeEvent {
        pub timestamp: u64,
        pub event_type: u32,
        pub track_id: u32,
        pub value: f32,
        pub label: [u8; 128], // FIXED-SIZE: Zero allocation across bridge
    }

    pub struct ArrangementSectionFFI {
        pub start_sample: u64,
        pub end_sample: u64,
        pub label: String,
    }

    pub struct BridgeLoudness {
        pub integrated: f32,
        pub short_term: f32,
        pub true_peak_l: f32,
        pub true_peak_r: f32,
        pub correlation: f32,
    }

    pub struct BridgeClash {
        pub frequency: f32,
        pub severity: f32,
    }

    pub struct ArrangementSection {
        pub start_sample: u64,
        pub end_sample: u64,
        pub section_type: u8,
        pub energy_level: f32,
        pub motivic_id: u32,
        pub narrative_flow_score: f32,
    }

    #[derive(Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum HirariCommand {
        SetVolume = 0,
        SetPan = 1,
        SetMute = 2,
        SetSolo = 3,
        AddPlugin = 4,
        RemovePlugin = 5,
        GenerateDrumFill = 6,
        ExecuteAutoMixing = 7,
        ExecuteAutoArrangement = 8,
        SetSpatialMode = 9,
        SetSpatialPosition = 10,
        SetAscendedMode = 11,
        SetAutomationRecordMode = 12,
    }

    unsafe extern "C++" {
        include!("core/audio_engine.hpp");
        include!("core/analysis_hub.hpp");
        include!("core/hirari_unified_engine.hpp");
        include!("core/bridge_shims.hpp");
        include!("core/bridge_types.hpp");

        #[namespace = "Hirari::Core::BridgeFFI"]
        type AudioEngine;
        #[namespace = "Hirari::Core::BridgeFFI"]
        type AnalysisHub;

        #[namespace = "Hirari::Core::Bridge"]
        type StructureNode;
        #[namespace = "Hirari::Core::Engine"]
        type HirariUnifiedEngine;

        // --- LIFECYCLE: No more global singletons ---
        fn new_audio_engine() -> UniquePtr<AudioEngine>;
        fn new_audio_engine_offline() -> UniquePtr<AudioEngine>;
        fn start_audio_device(self: &AudioEngine) -> bool;
        fn new_analysis_hub(engine: &AudioEngine) -> UniquePtr<AnalysisHub>;

        fn set_playing(self: &AudioEngine, p: bool);
        fn try_set_playing(self: &AudioEngine, p: bool) -> bool;
        fn set_loop(self: &AudioEngine, enabled: bool);
        fn set_cycle_range(
            self: &AudioEngine,
            start_sample: u64,
            end_sample: u64,
            enabled: bool,
        ) -> bool;
        fn is_loop_enabled(self: &AudioEngine) -> bool;
        fn cycle_start(self: &AudioEngine) -> u64;
        fn cycle_end(self: &AudioEngine) -> u64;
        fn set_metronome_enabled(self: &AudioEngine, enabled: bool);
        fn is_metronome_enabled(self: &AudioEngine) -> bool;
        fn is_playing(self: &AudioEngine) -> bool;
        fn get_playhead(self: &AudioEngine) -> u64;
        fn samples_to_beats(self: &AudioEngine, samples: u64) -> f64;
        fn beats_to_samples(self: &AudioEngine, beats: f64) -> u64;
        fn get_tempo_events(self: &AudioEngine) -> Vec<f64>;
        fn get_time_signature_events(self: &AudioEngine) -> Vec<f64>;
        fn set_time_signature_event(
            self: &AudioEngine,
            beat: f64,
            numerator: u8,
            denominator: u8,
        ) -> bool;
        fn remove_time_signature_event(self: &AudioEngine, beat: f64) -> bool;
        fn clear_time_signature_events(self: &AudioEngine);
        fn clear_tempo_events(self: &AudioEngine, initial_bpm: f64);
        fn set_tempo_event(self: &AudioEngine, beat: f64, bpm: f64, ramp: bool) -> bool;
        fn set_tempo_transition_event(self: &AudioEngine, beat: f64, bpm: f64, ramp: bool) -> bool;
        fn remove_tempo_event(self: &AudioEngine, beat: f64) -> bool;
        fn move_tempo_event(self: &AudioEngine, from_beat: f64, to_beat: f64) -> bool;
        fn get_tempo(self: &AudioEngine) -> f32;
        fn set_tempo(self: &AudioEngine, bpm: f32) -> bool;
        fn set_master_gain(self: &AudioEngine, value: f32) -> bool;
        fn add_control_room_speaker(self: &AudioEngine, name: &str, gain: f32) -> bool;
        fn reset_control_room(self: &AudioEngine);
        fn select_control_room_speaker(self: &AudioEngine, index: u32) -> bool;
        fn remove_control_room_speaker(self: &AudioEngine, index: u32) -> bool;
        fn set_control_room_speaker_gain(self: &AudioEngine, index: u32, gain: f32) -> bool;
        fn set_control_room_speaker_enabled(self: &AudioEngine, index: u32, enabled: bool) -> bool;
        fn upsert_control_room_cue(self: &AudioEngine, id: u32, gain: f32, enabled: bool) -> bool;
        fn remove_control_room_cue(self: &AudioEngine, id: u32) -> bool;
        fn set_control_room_cue_enabled(self: &AudioEngine, id: u32, enabled: bool) -> bool;
        fn set_control_room_cue_bus_track(
            self: &AudioEngine,
            cue_id: u32,
            bus_track_id: u32,
        ) -> bool;
        fn set_control_room_cue_output_channel(
            self: &AudioEngine,
            cue_id: u32,
            output_channel: u32,
        ) -> bool;
        fn set_control_room_cue_click_enabled(
            self: &AudioEngine,
            cue_id: u32,
            enabled: bool,
        ) -> bool;
        fn control_room_cue_gain(self: &AudioEngine, id: u32) -> f32;
        fn set_control_room_active_cue(self: &AudioEngine, id: u32) -> bool;
        fn control_room_active_cue(self: &AudioEngine) -> u32;
        fn control_room_validate(self: &AudioEngine) -> bool;
        fn set_control_room_dim(self: &AudioEngine, enabled: bool);
        fn set_control_room_dim_reduction_db(self: &AudioEngine, db: f32) -> bool;
        fn control_room_dim_reduction_db(self: &AudioEngine) -> f32;
        fn set_control_room_talkback(self: &AudioEngine, enabled: bool, gain: f32);
        fn set_control_room_talkback_input_channel(self: &AudioEngine, channel: u32) -> bool;
        fn control_room_talkback_input_channel(self: &AudioEngine) -> u32;
        fn control_room_dimmed(self: &AudioEngine) -> bool;
        fn control_room_talkback_enabled(self: &AudioEngine) -> bool;
        fn control_room_monitor_gain(self: &AudioEngine) -> f32;
        fn get_master_gain(self: &AudioEngine) -> f32;
        fn set_track_volume(self: &AudioEngine, tid: u32, value: f32) -> bool;
        fn get_track_volume(self: &AudioEngine, tid: u32) -> f32;
        fn set_track_delay_samples(self: &AudioEngine, tid: u32, samples: u32) -> bool;
        fn get_track_delay_samples(self: &AudioEngine, tid: u32) -> u32;
        fn set_track_pan(self: &AudioEngine, tid: u32, value: f32) -> bool;
        fn set_track_mute(self: &AudioEngine, tid: u32, muted: bool) -> bool;
        fn set_track_solo(self: &AudioEngine, tid: u32, solo: bool) -> bool;
        fn set_track_solo_for_offline_render(self: &AudioEngine, tid: u32, solo: bool) -> bool;
        fn set_offline_render_target(self: &AudioEngine, tid: u32) -> bool;
        fn clear_offline_render_target(self: &AudioEngine);
        fn set_offline_render_tail_seconds(self: &AudioEngine, seconds: f32);
        fn set_offline_render_options(self: &AudioEngine, pre_fader: bool, include_inserts: bool);
        fn set_phase_invert(self: &AudioEngine, tid: u32, inverted: bool) -> bool;
        fn is_audio_device_ready(self: &AudioEngine) -> bool;
        fn is_silent_fallback(self: &AudioEngine) -> bool;
        fn audio_driver_status(self: &AudioEngine) -> String;
        fn audio_driver_error_code(self: &AudioEngine) -> i32;
        fn get_audio_output_peak(self: &AudioEngine) -> f32;
        fn get_audio_callback_count(self: &AudioEngine) -> u64;
        fn get_dropped_input_blocks(self: &AudioEngine) -> u64;
        fn discard_pending_audio_input(self: &AudioEngine);
        fn poll_audio_input(
            self: &AudioEngine,
            channel_count: &mut u32,
            dropped_input_blocks: &mut u64,
        ) -> Vec<f32>;
        fn get_audio_input_channel_count(self: &AudioEngine) -> u32;
        fn get_audio_output_channel_count(self: &AudioEngine) -> u32;
        fn try_reconnect_audio_device(self: &AudioEngine);
        fn get_sample_rate(self: &AudioEngine) -> f64;
        fn list_audio_devices_json(self: &AudioEngine) -> String;
        fn audio_input_device_uid(self: &AudioEngine) -> String;
        fn list_midi_devices_json(self: &AudioEngine) -> String;
        fn send_midi_message(self: &AudioEngine, unique_id: u32, data: &[u8]) -> bool;
        fn start_midi_input(self: &AudioEngine) -> bool;
        fn stop_midi_input(self: &AudioEngine);
        fn poll_midi_input_json(self: &AudioEngine) -> String;
        fn midi_host_time_now(self: &AudioEngine) -> u64;
        fn midi_dropped_input_events(self: &AudioEngine) -> u64;
        fn midi_host_time_delta_samples(
            self: &AudioEngine,
            start: u64,
            event: u64,
            sample_rate: f64,
        ) -> u64;
        fn select_audio_device(
            self: &AudioEngine,
            device_id: u32,
            sample_rate: f64,
            buffer_size: u32,
        ) -> bool;
        fn get_audio_config_generation(self: &AudioEngine) -> u64;
        fn apply_config(self: &AudioEngine, tempo: f32, sr: u32, bs: u32);
        fn try_apply_config(self: &AudioEngine, tempo: f32, sr: u32, bs: u32) -> bool;
        fn set_playhead(self: &AudioEngine, pos: u64);
        fn set_test_tone(self: &AudioEngine, enabled: bool);
        fn bind_midi_cc_to_macro(
            self: &AudioEngine,
            channel: u8,
            cc: u8,
            macro_index: u32,
            minimum: f32,
            maximum: f32,
            curve: f32,
            pickup: bool,
        ) -> bool;
        fn bind_midi_cc14_to_macro(
            self: &AudioEngine,
            channel: u8,
            controller: u16,
            macro_index: u32,
            minimum: f32,
            maximum: f32,
            curve: f32,
            pickup: bool,
        ) -> bool;
        fn handle_midi_cc(self: &AudioEngine, channel: u8, cc: u8, value: u8);
        fn handle_midi_cc14(self: &AudioEngine, channel: u8, controller: u16, value: u16);
        fn set_preview_sample(self: &AudioEngine, samples: &[f32], source_rate: f64);
        fn process_audio_block(self: &AudioEngine, left: &mut [f32], right: &mut [f32]);
        fn trigger_preview_sample(self: &AudioEngine);
        fn clear_preview_sample(self: &AudioEngine);
        fn shutdown(self: &AudioEngine);
        fn add_track(self: &AudioEngine, t_type: u32) -> u32;
        fn add_vca_group(self: &AudioEngine, group_id: u32, gain: f32) -> bool;
        fn assign_track_to_vca(self: &AudioEngine, track_id: u32, group_id: u32) -> bool;
        fn set_vca_group_gain(self: &AudioEngine, group_id: u32, gain: f32) -> bool;
        fn get_vca_track_gain(self: &AudioEngine, track_id: u32) -> f32;
        fn get_vca_snapshot_json(self: &AudioEngine) -> String;
        fn clear_vca_groups(self: &AudioEngine);
        fn remove_track(self: &AudioEngine, id: u32) -> bool;
        fn set_track_name(self: &AudioEngine, id: u32, name: &str) -> bool;
        fn new_project(self: &AudioEngine);
        fn duplicate_track(self: &AudioEngine, id: u32) -> u32;
        fn freeze_track(self: &AudioEngine, tid: u32, total_samples: u64, sample_rate: u32)
            -> bool;
        fn freeze_track_to_project_end(self: &AudioEngine, tid: u32, sample_rate: u32) -> bool;
        fn freeze_track_to_file(
            self: &AudioEngine,
            tid: u32,
            path: &str,
            total_samples: u64,
            sample_rate: u32,
        ) -> bool;
        fn set_track_freeze_cache_path(self: &AudioEngine, tid: u32, path: &str) -> bool;
        fn restore_track_freeze_from_file(
            self: &AudioEngine,
            tid: u32,
            path: &str,
            total_samples: u64,
            sample_rate: u32,
        ) -> bool;
        fn unfreeze_track(self: &AudioEngine, tid: u32) -> bool;
        fn is_track_frozen(self: &AudioEngine, tid: u32) -> bool;
        fn add_plugin(self: &AudioEngine, tid: u32, plugin_type: u32) -> bool;
        fn get_plugin_state(self: &AudioEngine, tid: u32, plugin_index: u32) -> Vec<u8>;
        fn set_plugin_state(self: &AudioEngine, tid: u32, plugin_index: u32, state: &[u8]) -> bool;
        fn remove_plugin(self: &AudioEngine, tid: u32, plugin_index: u32) -> bool;
        fn move_plugin(self: &AudioEngine, tid: u32, from_index: u32, to_index: u32) -> bool;
        fn add_sandboxed_plugin(self: &AudioEngine, tid: u32, path: &str) -> bool;
        fn process_sandboxed_plugin_block(
            self: &AudioEngine,
            tid: u32,
            sandbox_index: u32,
            left: &mut [f32],
            right: &mut [f32],
        ) -> bool;
        fn process_sandboxed_plugin_midi_block(
            self: &AudioEngine,
            tid: u32,
            sandbox_index: u32,
            frames: u32,
            midi_data: &[u8],
        ) -> Vec<u8>;
        fn maintain_sandboxed_plugins(self: &AudioEngine, auto_restart: bool) -> u32;
        fn take_watchdog_trips(self: &AudioEngine) -> u32;
        fn non_finite_plugin_samples(self: &AudioEngine) -> u64;
        fn retry_sandboxed_plugin(self: &AudioEngine, track_id: u32, sandbox_index: u32) -> bool;
        fn restart_sandboxed_plugin(self: &AudioEngine, track_id: u32, sandbox_index: u32) -> bool;
        fn get_sandbox_statuses(self: &AudioEngine) -> Vec<u32>;
        fn get_last_sandbox_failure(self: &AudioEngine, track_id: u32) -> u32;
        fn get_last_sandbox_failure_text(self: &AudioEngine, track_id: u32) -> String;
        fn get_sandbox_plugin_paths(self: &AudioEngine) -> Vec<String>;
        fn get_sandbox_plugin_state(
            self: &AudioEngine,
            track_id: u32,
            sandbox_index: u32,
        ) -> Vec<u8>;
        fn set_sandbox_plugin_state(
            self: &AudioEngine,
            track_id: u32,
            sandbox_index: u32,
            state: &[u8],
        ) -> bool;
        fn get_sandbox_plugin_state_error(
            self: &AudioEngine,
            track_id: u32,
            sandbox_index: u32,
        ) -> u8;
        fn get_sandbox_plugin_state_error_text(
            self: &AudioEngine,
            track_id: u32,
            sandbox_index: u32,
        ) -> String;

        fn add_region(self: &AudioEngine, tid: u32, path: &str, start: f64) -> bool;
        fn queue_region_import(self: &AudioEngine, tid: u32, path: &str, start_sample: u64) -> u64;
        fn complete_region_import(self: &AudioEngine, ticket: u64) -> u8;
        fn replace_region_audio(self: &AudioEngine, tid: u32, rid: u32, path: &str) -> bool;
        fn move_region(self: &AudioEngine, tid: u32, rid: u32, start: f64) -> bool;
        fn move_region_sync_group(self: &AudioEngine, tid: u32, rid: u32, start: f64) -> bool;
        fn split_region(self: &AudioEngine, tid: u32, rid: u32, split: f64) -> bool;
        fn split_region_with_right_id(self: &AudioEngine, tid: u32, rid: u32, split: f64) -> u32;
        fn remove_region(self: &AudioEngine, tid: u32, rid: u32) -> bool;
        fn duplicate_region(self: &AudioEngine, tid: u32, rid: u32, start_sample: u64) -> u32;
        fn set_region_gain(self: &AudioEngine, tid: u32, rid: u32, gain: f32) -> bool;
        fn set_region_range_edit(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            start: u64,
            end: u64,
            gain: f32,
            fade_in: u64,
            fade_out: u64,
        ) -> bool;
        fn clear_region_range_edits(self: &AudioEngine, tid: u32, rid: u32) -> bool;
        fn set_region_muted(self: &AudioEngine, tid: u32, rid: u32, muted: bool) -> bool;
        fn set_region_comp_ranges(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            packed_ranges: &[u64],
            managed: bool,
        ) -> bool;
        fn set_region_fades(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            fade_in: f32,
            fade_out: f32,
        ) -> bool;
        fn set_region_reverse(self: &AudioEngine, tid: u32, rid: u32, reverse: bool) -> bool;
        fn set_region_sync_group(self: &AudioEngine, tid: u32, rid: u32, group: u32) -> bool;
        fn set_region_warp_markers(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            source_samples: &[u64],
            timeline_samples: &[u64],
            transient_flags: &[u8],
        ) -> bool;
        fn align_regions_to_reference(
            self: &AudioEngine,
            reference_tid: u32,
            reference_rid: u32,
            target_tids: &[u32],
            target_rids: &[u32],
        ) -> bool;
        fn region_alignment_status(
            self: &AudioEngine,
            reference_tid: u32,
            reference_rid: u32,
        ) -> u8;
        fn finalize_region_alignment(
            self: &AudioEngine,
            reference_tid: u32,
            reference_rid: u32,
        ) -> bool;
        fn quantize_region_sync_group(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            strength: f32,
            grid_beats: f64,
            swing: f32,
        ) -> bool;
        fn region_quantize_status(self: &AudioEngine, tid: u32, rid: u32) -> u8;
        fn finalize_region_quantize(self: &AudioEngine, tid: u32, rid: u32) -> bool;
        fn set_region_warp_ratio(self: &AudioEngine, tid: u32, rid: u32, ratio: f64) -> bool;
        fn set_region_pitch_preserve_warp(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            enabled: bool,
        ) -> bool;
        fn set_region_pitch_semitones(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            semitones: f32,
        ) -> bool;
        fn set_region_audio_note_segment(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            start_seconds: f64,
            end_seconds: f64,
            pitch_offset_cents: f64,
            formant_offset_cents: f64,
        ) -> bool;
        fn set_region_audio_note_anchor(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start_seconds: f64,
            position_seconds: f64,
            pitch_cents: f64,
            formant_cents: f64,
        ) -> bool;
        fn set_region_audio_note_pitch_anchor(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start_seconds: f64,
            position_seconds: f64,
            pitch_cents: f64,
        ) -> bool;
        fn set_region_audio_note_formant_anchor(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start_seconds: f64,
            position_seconds: f64,
            formant_cents: f64,
        ) -> bool;
        fn move_region_audio_note_anchor(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start_seconds: f64,
            old_position_seconds: f64,
            new_position_seconds: f64,
            value_cents: f64,
            edit_formant: bool,
        ) -> bool;
        fn clear_region_audio_note_segments(self: &AudioEngine, tid: u32, rid: u32) -> bool;
        fn warp_region_audio_note_segment(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start: f64,
            new_start: f64,
            new_end: f64,
        ) -> bool;
        fn remove_region_audio_note_segment(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            segment_start: f64,
        ) -> bool;
        fn analyze_region_audio_note_segments(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            sample_rate: f64,
        ) -> bool;
        fn start_region_audio_note_analysis(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            sample_rate: f64,
        ) -> bool;
        fn region_audio_note_analysis_status(self: &AudioEngine, tid: u32, rid: u32) -> u8;
        fn finalize_region_audio_note_analysis(self: &AudioEngine, tid: u32, rid: u32) -> bool;
        fn set_region_loop_count(self: &AudioEngine, tid: u32, rid: u32, count: u32) -> bool;
        fn set_region_trim(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            start_norm: f32,
            end_norm: f32,
        ) -> bool;
        fn clear_midi_notes(self: &AudioEngine);
        fn replace_midi_notes(self: &AudioEngine, packed: Vec<u64>, record_undo: bool) -> bool;
        fn midi_notes_snapshot(self: &AudioEngine) -> Vec<u64>;
        fn remove_midi_notes_range(
            self: &AudioEngine,
            track_id: u32,
            start_sample: u64,
            end_sample: u64,
        ) -> bool;
        fn transpose_midi_notes_range(
            self: &AudioEngine,
            track_id: u32,
            start_sample: u64,
            end_sample: u64,
            semitones: i32,
        ) -> bool;
        fn move_midi_notes_range(
            self: &AudioEngine,
            track_id: u32,
            start_sample: u64,
            end_sample: u64,
            delta_samples: i64,
        ) -> bool;
        fn set_midi_note(
            self: &AudioEngine,
            track_id: u32,
            pitch: u8,
            velocity: u8,
            start_sample: u64,
            length_samples: u64,
        );
        fn set_spatial_position(self: &AudioEngine, tid: u32, x: f32, y: f32, z: f32) -> bool;
        fn set_hrtf_kernel(self: &AudioEngine, tid: u32, left: Vec<f32>, right: Vec<f32>) -> bool;
        fn clear_hrtf_kernel(self: &AudioEngine, tid: u32) -> bool;
        fn set_track_armed(self: &AudioEngine, tid: u32, armed: bool) -> bool;
        fn set_track_input_monitor(self: &AudioEngine, tid: u32, enabled: bool) -> bool;
        fn set_track_input_monitor_channels(
            self: &AudioEngine,
            tid: u32,
            enabled: bool,
            left: u32,
            right: u32,
        ) -> bool;
        fn set_automation_data(
            self: &AudioEngine,
            tid: u32,
            param_id: u32,
            packed_points: Vec<f64>,
        ) -> bool;
        fn set_automation_record_mode(self: &AudioEngine, mode: u32) -> bool;
        fn plugin_compatibility_snapshot_json(self: &AudioEngine) -> String;
        fn set_track_delay_automation(
            self: &AudioEngine,
            tid: u32,
            packed_points: Vec<f64>,
        ) -> bool;
        fn set_plugin_parameter(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
            value: f32,
        ) -> bool;
        fn set_plugin_automation(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
            packed_points: Vec<f64>,
        ) -> bool;
        fn set_plugin_parameter_without_undo(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
            value: f32,
        ) -> bool;
        fn get_plugin_parameter(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
        ) -> f32;
        fn get_plugin_parameter_count(self: &AudioEngine, tid: u32, plugin_index: u32) -> u32;
        fn get_plugin_parameter_name(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
        ) -> String;
        fn save_plugin_preset(self: &AudioEngine, tid: u32, plugin_index: u32, path: &str) -> bool;
        fn load_plugin_preset(self: &AudioEngine, tid: u32, plugin_index: u32, path: &str) -> bool;
        fn set_plugin_bypass(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            bypassed: bool,
        ) -> bool;
        fn get_plugin_bypass(self: &AudioEngine, tid: u32, plugin_index: u32) -> bool;
        fn set_macro_value(self: &AudioEngine, macro_index: u32, value: f32);
        fn set_preview_synth_engine(self: &AudioEngine, engine: u32);
        fn set_route(self: &AudioEngine, source_id: u32, dest_id: u32, enabled: bool) -> bool;
        fn set_route_gain(
            self: &AudioEngine,
            source_id: u32,
            dest_id: u32,
            gain: f32,
            enabled: bool,
        ) -> bool;
        fn set_send_route(
            self: &AudioEngine,
            source_id: u32,
            dest_id: u32,
            gain: f32,
            pre_fader: bool,
            enabled: bool,
        ) -> bool;
        fn set_feedback_route(
            self: &AudioEngine,
            source_id: u32,
            dest_id: u32,
            gain: f32,
            enabled: bool,
        ) -> bool;
        fn set_sidechain_link(
            self: &AudioEngine,
            source_id: u32,
            dest_id: u32,
            plugin_index: u32,
            tap_point: u32,
            enabled: bool,
        ) -> bool;
        fn has_sidechain_link(
            self: &AudioEngine,
            source_id: u32,
            dest_id: u32,
            plugin_index: u32,
        ) -> bool;
        fn save_project(self: &AudioEngine, path: &str) -> bool;
        fn load_project(self: &AudioEngine, path: &str) -> bool;
        fn bounce_project(self: &AudioEngine, path: &str, format: u32) -> bool;
        fn bounce_project_diagnostic_json(self: &AudioEngine, path: &str, format: u32) -> String;
        fn read_wav_diagnostic_json(self: &AudioEngine, path: &str, format: u32) -> String;
        fn bounce_project_async(self: &AudioEngine, path: &str, format: u32) -> bool;
        fn get_bounce_progress(self: &AudioEngine) -> f32;
        fn get_bounce_state(self: &AudioEngine) -> u32;
        fn cancel_bounce(self: &AudioEngine) -> bool;
        fn pause_bounce(self: &AudioEngine) -> bool;
        fn resume_bounce(self: &AudioEngine) -> bool;
        fn has_plugin_native_editor(self: &AudioEngine, track_id: u32, plugin_index: u32) -> bool;
        fn plugin_native_editor_embedded(
            self: &AudioEngine,
            track_id: u32,
            plugin_index: u32,
        ) -> bool;
        fn open_plugin_native_editor(
            self: &AudioEngine,
            track_id: u32,
            plugin_index: u32,
            parent: u64,
        ) -> u64;
        fn close_plugin_native_editor(self: &AudioEngine, track_id: u32, plugin_index: u32)
            -> bool;
        fn midi_clock_tick(self: &AudioEngine, timestamp: u64);
        fn midi_clock_ticks(self: &AudioEngine) -> u64;
        fn midi_clock_last_tick(self: &AudioEngine) -> u64;
        fn midi_clock_rate(self: &AudioEngine) -> f64;
        fn set_midi_clock_rate(self: &AudioEngine, bpm: f64);
        fn execute_auto_mixing(self: &AudioEngine) -> bool;
        fn execute_auto_arrangement(self: &AudioEngine) -> bool;
        fn set_project_scale(self: &AudioEngine, root: i32, scale_type: i32) -> bool;
        fn undo(self: &AudioEngine);
        fn redo(self: &AudioEngine);
        fn is_undo_transaction_active(self: &AudioEngine) -> bool;
        fn begin_undo_transaction(self: &AudioEngine, name: &str);
        fn end_undo_transaction(self: &AudioEngine) -> bool;
        fn abort_undo_transaction(self: &AudioEngine) -> bool;
        fn clear_undo_history(self: &AudioEngine);
        fn get_undo_count(self: &AudioEngine) -> u32;
        fn get_redo_count(self: &AudioEngine) -> u32;
        fn execute_vocal_remover(self: &AudioEngine, tid: u32) -> bool;
        fn set_articulation_map(self: &AudioEngine, tid: u32, map_hash: u32) -> bool;
        fn set_expression_map(
            self: &AudioEngine,
            tid: u32,
            packed: Vec<u32>,
            json: String,
            articulation_id: u32,
        ) -> bool;
        fn set_expression_map_pro(self: &AudioEngine, tid: u32, json: String) -> bool;
        fn set_track_eq(
            self: &AudioEngine,
            tid: u32,
            low_boost_db: f32,
            low_cut_db: f32,
            high_boost_db: f32,
            high_cut_db: f32,
        ) -> bool;
        fn get_track_correlation(self: &AudioEngine, tid: u32) -> f32;
        fn get_track_count(self: &AudioEngine) -> u32;
        fn get_project_layout_json(self: &AudioEngine) -> String;
        fn request_project_layout_snapshot(self: &AudioEngine) -> u64;
        fn poll_project_layout_snapshot(self: &AudioEngine, request_id: u64) -> String;
        fn cancel_project_layout_snapshot(self: &AudioEngine, request_id: u64) -> bool;
        fn get_routing_snapshot_json(self: &AudioEngine) -> String;
        fn get_project_generation(self: &AudioEngine) -> u64;
        fn get_project_layout_revision(self: &AudioEngine) -> u64;
        fn get_project_state_revision(self: &AudioEngine) -> u64;
        fn mark_project_layout_changed(self: &AudioEngine);
        fn get_last_command_error(self: &AudioEngine) -> u8;
        fn get_cpu_total_v(self: &AudioEngine) -> f32;
        fn get_runtime_health_v(self: &AudioEngine) -> Vec<f32>;
        fn audio_range_overflowed(self: &AudioEngine) -> bool;
        fn get_block_size(self: &AudioEngine) -> u32;
        fn get_latency_ms(self: &AudioEngine) -> f32;
        fn get_track_latency_ms(self: &AudioEngine, tid: u32) -> f32;
        fn get_track_pdc_compensation_ms(self: &AudioEngine, tid: u32) -> f32;
        fn set_low_latency_mode(self: &AudioEngine, active: bool) -> bool;
        fn low_latency_mode(self: &AudioEngine) -> bool;
        fn set_tonal_scale(self: &AudioEngine, root: i32, scale_type: u32) -> bool;
        fn is_note_in_tonal_scale(self: &AudioEngine, midi_note: i32) -> bool;
        fn tonal_root(self: &AudioEngine) -> i32;
        fn tonal_scale_type(self: &AudioEngine) -> u32;
        fn get_video_frame(self: &AudioEngine) -> Vec<u8>;
        fn get_video_frame_revision(self: &AudioEngine) -> u64;
        fn request_video_frame(self: &AudioEngine, seconds: f64) -> bool;
        fn load_video(self: &AudioEngine, path: &str) -> bool;

        fn push_command(
            engine: &AudioEngine,
            cmd: HirariCommand,
            tid: u32,
            val: f32,
            ts: u64,
            expected_project_generation: u64,
            expected_audio_generation: u64,
        ) -> bool;

        fn get_region_waveform(self: &AudioEngine, tid: u32, rid: u32) -> Vec<f32>;
        fn queue_region_waveform(self: &AudioEngine, tid: u32, rid: u32) -> u64;
        fn poll_region_waveform(self: &AudioEngine, request: u64) -> Vec<f32>;
        fn region_waveform_pending(self: &AudioEngine, request: u64) -> bool;
        fn get_region_audio_interleaved(self: &AudioEngine, tid: u32, rid: u32) -> Vec<f32>;
        fn get_region_audio_interleaved_range(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            start_frame: u64,
            frames: u32,
            channels: u32,
        ) -> Vec<f32>;
        fn get_region_source_frame_count(self: &AudioEngine, tid: u32, rid: u32) -> u64;
        fn get_region_sample_rate(self: &AudioEngine, tid: u32, rid: u32) -> f64;
        fn get_region_channel_count(self: &AudioEngine, tid: u32, rid: u32) -> u32;
        fn get_mixer_levels_v(self: &AnalysisHub) -> Vec<f32>;
        fn get_spectral_data_v(self: &AnalysisHub) -> Vec<f32>;
        fn get_spectral_partials_v(self: &AnalysisHub) -> Vec<f32>;
        fn get_motion_energy(self: &AnalysisHub) -> f32;
        fn get_synesthesia_colors_v(self: &AnalysisHub) -> Vec<f32>;
        fn get_master_loudness_v_ffi(hub: &AnalysisHub) -> BridgeLoudness;
        fn get_intelligence_dashboard_json(self: &AnalysisHub) -> String;
        fn get_spectral_clash_v_ffi(hub: &AnalysisHub) -> Vec<BridgeClash>;
        fn get_creative_advice(self: &AnalysisHub) -> String;
        fn get_arrangement_advice(self: &AnalysisHub, tid: u32) -> String;
        fn get_song_structure_json_ffi(hub: &AnalysisHub) -> String;

        // --- OWNERSHIP: Tied lifetime to the bridge host ---
        fn get_unified_engine(bridge: &AudioEngine) -> &HirariUnifiedEngine;
        fn get_track_peaks_l(engine: &HirariUnifiedEngine) -> &[f32];
        fn get_track_peaks_r(engine: &HirariUnifiedEngine) -> &[f32];
        fn get_track_peaks_l_owned(engine: &HirariUnifiedEngine) -> Vec<f32>;
        fn get_track_peaks_r_owned(engine: &HirariUnifiedEngine) -> Vec<f32>;
        fn pop_event(engine: &HirariUnifiedEngine, event: &mut BridgeEvent) -> bool;

        // --- GPU: Explicit deterministic initialization ---
        fn initialize_gpu();
        fn initialize_gpu_with_status() -> bool;
    }

    extern "Rust" {
        fn report_hirari_log(level: u32, msg: &str);

        fn runtime_health_vector(
            peaks_l: &[f32],
            peaks_r: &[f32],
            sample_count: u32,
            dsp_load: f32,
            correlation: f32,
            active_voices: u32,
            telemetry_version: u64,
            device_running: bool,
            playing: bool,
            playhead: f32,
        ) -> Vec<f32>;
        fn interleave_planar_audio_input(
            planar: &[f32],
            channels: u32,
            frames: u32,
            channel_stride: u32,
        ) -> Vec<f32>;

        fn analysis_spectral_display(spectrum: &[f32], sample_rate: f64) -> Vec<f32>;
        fn analysis_phase_heatmap(spectrum: &[f32]) -> Vec<f32>;
        fn analysis_partials(spectrum: &[f32]) -> Vec<f32>;
        fn analysis_mel_spectrogram(spectrum: &[f32]) -> Vec<f32>;
        fn analysis_motion_vectors(spectrum: &[f32]) -> Vec<f32>;
        fn analysis_motion_energy(spectrum: &[f32]) -> f32;
        fn analysis_synesthesia_colors(spectrum: &[f32]) -> Vec<f32>;
        fn analysis_arrangement_advice(track_id: u32) -> String;
        fn analysis_mixing_advice(true_peak_l: f32, true_peak_r: f32, correlation: f32)
            -> Vec<f32>;
        fn analysis_spectral_clashes(
            true_peak_l: f32,
            true_peak_r: f32,
            correlation: f32,
        ) -> Vec<f32>;
        fn analysis_song_structure_json(
            kinds: &[u8],
            starts: &[u64],
            ends: &[u64],
            energies: &[f32],
            flows: &[f32],
        ) -> String;
        fn analysis_structure_sections(
            region_starts: &[u64],
            total_length: u64,
            bpm: f64,
            sample_rate: f64,
            tension: f32,
            valence: f32,
        ) -> Vec<ArrangementSection>;
        fn analysis_waveform_peak_envelope(
            left: &[f32],
            right: &[f32],
            peak_count: u32,
        ) -> Vec<f32>;
        fn resample_waveform_peak_envelope(peaks: &[f32], peak_count: u32) -> Vec<f32>;
        fn analysis_creative_advice(true_peak_l: f32, true_peak_r: f32, correlation: f32)
            -> String;
        fn analysis_dashboard_json(
            lufs_short: f32,
            lufs_integrated: f32,
            true_peak_l: f32,
            true_peak_r: f32,
            correlation: f32,
        ) -> String;

        type SummingOrchestrator;
        type SummingConfig;
        fn mix_buses_boutique(
            self: &SummingOrchestrator,
            target_l: &mut [f32],
            target_r: &mut [f32],
        );
        fn process_limiter(
            self: &mut SummingOrchestrator,
            buffer: &mut [f32],
            config: &SummingConfig,
        ) -> f32;
    }
}

pub fn analysis_structure_sections(
    region_starts: &[u64],
    total_length: u64,
    bpm: f64,
    sample_rate: f64,
    tension: f32,
    valence: f32,
) -> Vec<ffi::ArrangementSection> {
    build_analysis_structure_sections(
        region_starts,
        total_length,
        bpm,
        sample_rate,
        tension,
        valence,
    )
    .into_iter()
    .map(|section| ffi::ArrangementSection {
        start_sample: section.start_sample,
        end_sample: section.end_sample,
        section_type: section.section_type,
        energy_level: section.energy_level,
        motivic_id: section.motivic_id,
        narrative_flow_score: section.narrative_flow_score,
    })
    .collect()
}
include!("lib_runtime.rs");
