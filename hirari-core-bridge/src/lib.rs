// DSP kernels intentionally use index-based loops for fixed channel/lane
// processing and preallocated buffers. Keep these layouts explicit rather
// than forcing iterator rewrites that can obscure real-time data access.
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![recursion_limit = "512"]

// The native bridge accepts macOS plugin bundles; the isolated worker
// resolves the Mach-O image inside the bundle before loading it.

include!("module_exports.rs");
use core::pin::Pin;

pub use analysis_views::{
    analysis_arrangement_advice, analysis_creative_advice, analysis_dashboard_json,
    analysis_mel_spectrogram, analysis_mixing_advice, analysis_motion_energy,
    analysis_motion_vectors, analysis_partials, analysis_phase_heatmap,
    analysis_song_structure_json, analysis_spectral_clashes, analysis_spectral_display,
    analysis_structure_sections as build_analysis_structure_sections, analysis_synesthesia_colors,
};
pub use command_host::engine_last_command_error;
pub use plugin_sandbox_maintenance::sandbox_statuses_packed;
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
pub mod vari_audio;

pub mod spatial_orchestrator;
pub mod stable_api;
pub mod vfx_bindings;
pub mod vfx_timeline_bridge;
pub mod vocal_quality;
use anyhow::Context;
use engine_edit_ext::TrackEditExt;
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
        include!("core/hirari_unified_engine.hpp");
        include!("core/bridge_shims.hpp");
        include!("core/bridge_types.hpp");

        #[namespace = "Hirari::Core::BridgeFFI"]
        type AudioEngine;

        #[namespace = "Hirari::Core::Engine"]
        type HirariUnifiedEngine;

        // The inner engine is a per-session object that never changes identity.
        // Rust reaches its getters directly here instead of through an
        // AudioEngine forwarder per getter.
        fn get_core(self: &AudioEngine) -> &HirariUnifiedEngine;

        // --- LIFECYCLE: No more global singletons ---
        fn new_audio_engine() -> UniquePtr<AudioEngine>;
        fn new_audio_engine_offline() -> UniquePtr<AudioEngine>;

        fn set_playing(self: &AudioEngine, p: bool);
        fn try_set_playing(self: &AudioEngine, p: bool) -> bool;
        fn set_loop(self: Pin<&mut HirariUnifiedEngine>, enabled: bool);
        fn set_cycle_range(
            self: Pin<&mut HirariUnifiedEngine>,
            start_sample: u64,
            end_sample: u64,
            enabled: bool,
        ) -> bool;
        fn is_loop_enabled(self: &HirariUnifiedEngine) -> bool;
        fn cycle_start(self: &HirariUnifiedEngine) -> u64;
        fn cycle_end(self: &HirariUnifiedEngine) -> u64;
        fn is_playing(self: &HirariUnifiedEngine) -> bool;
        fn get_playhead(self: &HirariUnifiedEngine) -> u64;
        fn set_tempo(self: &HirariUnifiedEngine, bpm: f32) -> bool;
        fn set_master_gain(self: Pin<&mut HirariUnifiedEngine>, value: f32) -> bool;
        fn control_room_state(self: &AudioEngine) -> usize;
        fn tempo_state(self: &AudioEngine) -> usize;
        fn undo_state(self: &AudioEngine) -> usize;
        fn log_state(self: &AudioEngine) -> usize;
        // The MIDI snapshot store is native to Rust already; the engine keeps
        // the handle its render callback pins, so hand that address back.
        fn midi_snapshot_store(self: &HirariUnifiedEngine) -> usize;
        fn midi_learn_state(self: &HirariUnifiedEngine) -> usize;
        fn preview_synth_state(self: &HirariUnifiedEngine) -> usize;
        fn mark_project_state_changed(self: Pin<&mut HirariUnifiedEngine>);
        fn bus_track_exists(self: &HirariUnifiedEngine, bus_track_id: u32) -> bool;
        fn get_master_gain(self: &HirariUnifiedEngine) -> f32;
        fn get_track_volume(self: &HirariUnifiedEngine, tid: u32) -> f32;
        fn get_track_delay_samples(self: &HirariUnifiedEngine, tid: u32) -> u32;
        fn set_offline_render_target(self: Pin<&mut HirariUnifiedEngine>, tid: u32) -> bool;
        fn clear_offline_render_target(self: Pin<&mut HirariUnifiedEngine>);
        fn set_offline_render_tail_seconds(self: Pin<&mut HirariUnifiedEngine>, seconds: f32);
        fn set_offline_render_options(
            self: Pin<&mut HirariUnifiedEngine>,
            pre_fader: bool,
            include_inserts: bool,
        );
        fn audio_driver_host(self: &AudioEngine) -> usize;
        fn is_audio_device_ready(self: &AudioEngine) -> bool;
        fn discard_pending_audio_input(self: &AudioEngine);
        fn poll_audio_input(
            self: &AudioEngine,
            channel_count: &mut u32,
            dropped_input_blocks: &mut u64,
        ) -> Vec<f32>;
        fn try_reconnect_audio_device(self: &AudioEngine);
        fn select_audio_device(
            self: &AudioEngine,
            device_id: u32,
            sample_rate: f64,
            buffer_size: u32,
        ) -> bool;
        fn get_audio_config_generation(self: &AudioEngine) -> u64;
        fn apply_config(self: &AudioEngine, tempo: f32, sr: u32, bs: u32);
        fn try_apply_config(self: &AudioEngine, tempo: f32, sr: u32, bs: u32) -> bool;
        fn set_playhead(self: Pin<&mut HirariUnifiedEngine>, pos: u64);
        fn set_test_tone(self: Pin<&mut HirariUnifiedEngine>, enabled: bool);
        fn set_preview_sample(self: &AudioEngine, samples: &[f32], source_rate: f64);
        fn process_audio_block(self: &AudioEngine, left: &mut [f32], right: &mut [f32]);
        fn shutdown(self: &AudioEngine);
        fn assign_track_to_vca(
            self: Pin<&mut HirariUnifiedEngine>,
            track_id: u32,
            group_id: u32,
        ) -> bool;
        fn new_project(self: Pin<&mut HirariUnifiedEngine>);
        fn take_watchdog_trips(self: Pin<&mut HirariUnifiedEngine>) -> u32;
        fn non_finite_plugin_samples(self: &HirariUnifiedEngine) -> u64;
        fn retry_sandboxed_plugin(
            self: Pin<&mut HirariUnifiedEngine>,
            track_id: u32,
            sandbox_index: u32,
        ) -> bool;
        fn restart_sandboxed_plugin(
            self: Pin<&mut HirariUnifiedEngine>,
            track_id: u32,
            sandbox_index: u32,
        ) -> bool;

        fn add_region(self: &AudioEngine, tid: u32, path: &str, start: f64) -> bool;
        fn append_region_processing(
            self: &AudioEngine,
            tid: u32,
            rid: u32,
            step_id: u32,
            operation: &str,
            parameter: f32,
            enabled: bool,
        ) -> bool;
        fn clear_region_processing(self: Pin<&mut HirariUnifiedEngine>, tid: u32, rid: u32)
            -> bool;

        fn set_automation_record_mode(self: Pin<&mut HirariUnifiedEngine>, mode: u32) -> bool;
        fn get_plugin_parameter(
            self: &HirariUnifiedEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
        ) -> f32;
        fn get_plugin_parameter_count(
            self: &HirariUnifiedEngine,
            tid: u32,
            plugin_index: u32,
        ) -> u32;
        fn get_plugin_parameter_name(
            self: &AudioEngine,
            tid: u32,
            plugin_index: u32,
            parameter_id: u32,
        ) -> String;
        fn set_plugin_bypass(
            self: Pin<&mut HirariUnifiedEngine>,
            tid: u32,
            plugin_index: u32,
            bypassed: bool,
        ) -> bool;
        fn get_plugin_bypass(self: &HirariUnifiedEngine, tid: u32, plugin_index: u32) -> bool;
        fn has_sidechain_link(
            self: &HirariUnifiedEngine,
            source_id: u32,
            dest_id: u32,
            plugin_index: u32,
        ) -> bool;
        fn save_project(self: &AudioEngine, path: &str) -> bool;
        fn load_project(self: &AudioEngine, path: &str) -> bool;
        fn bounce_project(self: &AudioEngine, path: &str, format: u32) -> bool;
        fn bounce_project_async(self: &AudioEngine, path: &str, format: u32) -> bool;
        fn get_bounce_progress(self: &HirariUnifiedEngine) -> f32;
        fn get_bounce_state(self: &HirariUnifiedEngine) -> u32;
        fn cancel_bounce(self: Pin<&mut HirariUnifiedEngine>) -> bool;
        fn pause_bounce(self: Pin<&mut HirariUnifiedEngine>) -> bool;
        fn resume_bounce(self: Pin<&mut HirariUnifiedEngine>) -> bool;
        fn has_plugin_native_editor(
            self: &HirariUnifiedEngine,
            track_id: u32,
            plugin_index: u32,
        ) -> bool;
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
        fn midi_clock_tick(self: &HirariUnifiedEngine, timestamp: u64);
        fn midi_clock_ticks(self: &HirariUnifiedEngine) -> u64;
        fn midi_clock_last_tick(self: &HirariUnifiedEngine) -> u64;
        fn midi_clock_rate(self: &HirariUnifiedEngine) -> f64;
        fn set_midi_clock_rate(self: &HirariUnifiedEngine, bpm: f64);
        fn execute_auto_arrangement(self: Pin<&mut HirariUnifiedEngine>) -> bool;
        fn set_project_scale(
            self: Pin<&mut HirariUnifiedEngine>,
            root: i32,
            scale_type: i32,
        ) -> bool;
        fn execute_vocal_remover(self: Pin<&mut HirariUnifiedEngine>, tid: u32) -> bool;
        fn set_articulation_map(
            self: Pin<&mut HirariUnifiedEngine>,
            tid: u32,
            map_hash: u32,
        ) -> bool;
        fn set_track_eq(
            self: Pin<&mut HirariUnifiedEngine>,
            tid: u32,
            low_boost_db: f32,
            low_cut_db: f32,
            high_boost_db: f32,
            high_cut_db: f32,
        ) -> bool;
        fn get_track_correlation(self: &HirariUnifiedEngine, tid: u32) -> f32;
        // The Rust layout gather (layout_gather.rs) walks the live tracks
        // through this table; C++ no longer runs a snapshot session.
        fn layout_ops_handle(self: &HirariUnifiedEngine) -> usize;
        fn get_project_generation(self: &HirariUnifiedEngine) -> u64;
        fn get_project_layout_revision(self: &HirariUnifiedEngine) -> u64;
        fn get_project_state_revision(self: &HirariUnifiedEngine) -> u64;
        fn mark_project_layout_changed(self: &HirariUnifiedEngine);
        fn audio_range_overflowed(self: &HirariUnifiedEngine) -> bool;

        fn get_runtime_telemetry(bridge: &AudioEngine) -> Vec<f32>;

        fn get_block_size(self: &AudioEngine) -> u32;
        fn get_latency_ms(self: &HirariUnifiedEngine) -> f32;
        fn get_track_latency_ms(self: &HirariUnifiedEngine, tid: u32) -> f32;
        fn get_track_pdc_compensation_ms(self: &HirariUnifiedEngine, tid: u32) -> f32;
        fn pdc_state(self: &HirariUnifiedEngine) -> usize;
        fn preview_sampler(self: &HirariUnifiedEngine) -> usize;
        fn metronome_state(self: &HirariUnifiedEngine) -> usize;
        fn routing_engine_state(self: &HirariUnifiedEngine) -> usize;
        fn get_sample_rate(self: &HirariUnifiedEngine) -> f64;

        fn push_command(
            engine: &AudioEngine,
            cmd: HirariCommand,
            tid: u32,
            val: f32,
            ts: u64,
            expected_project_generation: u64,
            expected_audio_generation: u64,
        ) -> bool;

        fn queue_region_waveform(self: &AudioEngine, tid: u32, rid: u32) -> u64;
        // --- OWNERSHIP: Tied lifetime to the bridge host ---
        fn get_unified_engine(bridge: &AudioEngine) -> &HirariUnifiedEngine;
        fn get_track_peaks_l_owned(engine: &HirariUnifiedEngine) -> Vec<f32>;
        fn get_track_peaks_r_owned(engine: &HirariUnifiedEngine) -> Vec<f32>;
        fn pop_event(engine: &HirariUnifiedEngine, event: &mut BridgeEvent) -> bool;
        // Engine-side analysis accessors (replace the deleted AnalysisHub shim).
        fn get_active_telemetry_idx(engine: &HirariUnifiedEngine) -> u32;
        fn get_spectrum(engine: &HirariUnifiedEngine, idx: u32) -> Vec<f32>;
        fn get_master_meter_data(engine: &HirariUnifiedEngine) -> BridgeLoudness;
        fn get_intelligence_dashboard_json(engine: &HirariUnifiedEngine) -> String;
        fn get_song_structure_json(engine: &HirariUnifiedEngine) -> String;
    }

    // ---------------------------------------------------------------------
    // Native project container (`ARUA`) codec.
    //
    // Rust owns the binary layout, version compatibility branches, checksum
    // policy and file I/O. The C++ engine only extracts its live state into
    // these structs on save, and hydrates its graph from them on load.
    // Field names intentionally mirror the former C++ `ProjectState` so the
    // engine adapter stays a direct field-by-field copy.
    // ---------------------------------------------------------------------
    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeAutomationPoint {
        pub time: f64,
        pub value: f32,
        pub curve: f32,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativePluginAutomationLane {
        pub pluginIndex: u32,
        pub parameterId: u32,
        pub points: Vec<NativeAutomationPoint>,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeBlob {
        pub bytes: Vec<u8>,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeTrackState {
        pub id: u32,
        pub trackType: u32,
        pub volume: f32,
        pub pan: f32,
        pub pan3d_x: f32,
        pub pan3d_y: f32,
        pub pan3d_z: f32,
        pub muted: bool,
        pub solo: bool,
        pub phaseInvert: bool,
        pub recordArmed: bool,
        pub trackDelaySamples: u32,
        pub pluginName: String,
        pub pluginData: Vec<u8>,
        /// Plugin type ids decoded from `pluginData` by the container codec.
        /// `u32::MAX` marks a sandboxed slot. Only populated on load; the
        /// save path writes `pluginData` and ignores this projection.
        pub pluginTypes: Vec<u32>,
        pub pluginStates: Vec<NativeBlob>,
        pub pluginGuiStates: Vec<NativeBlob>,
        pub pluginBypass: Vec<u8>,
        pub sandboxedPluginPaths: Vec<String>,
        pub sandboxedPluginStates: Vec<NativeBlob>,
        pub volumeAutomation: Vec<NativeAutomationPoint>,
        pub panAutomation: Vec<NativeAutomationPoint>,
        pub trackDelayAutomation: Vec<NativeAutomationPoint>,
        pub pluginAutomation: Vec<NativePluginAutomationLane>,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeRangeEdit {
        pub start: u64,
        pub end: u64,
        pub gain: f32,
        pub fadeIn: u64,
        pub fadeOut: u64,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeWarpMarker {
        pub sourceSample: u64,
        pub timelineSample: u64,
        pub transient: bool,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeProcessingStep {
        pub id: u32,
        pub operation: String,
        pub parameter: f32,
        pub enabled: bool,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeNoteAnchor {
        pub positionSeconds: f64,
        pub pitchCents: f64,
        pub formantCents: f64,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeNoteSegment {
        pub startSeconds: f64,
        pub endSeconds: f64,
        pub detectedPitchCents: f64,
        pub pitchOffsetCents: f64,
        pub formantOffsetCents: f64,
        pub anchors: Vec<NativeNoteAnchor>,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeRegionState {
        pub id: u32,
        pub trackId: u32,
        pub samplePosition: u64,
        pub sampleLength: u64,
        pub sourceLength: u64,
        pub sourceSampleRate: u32,
        pub sourceOffset: u64,
        pub baseStart: u64,
        pub baseSourceOffset: u64,
        pub baseLength: u64,
        pub isMuted: bool,
        pub filePath: String,
        pub name: String,
        pub clipGain: f32,
        pub fadeInSamples: u64,
        pub fadeOutSamples: u64,
        pub reverse: bool,
        pub warpRatio: f64,
        pub pitchPreserveWarp: bool,
        pub pitchSemitones: f32,
        pub loopCount: u32,
        pub locked: bool,
        pub syncGroup: u32,
        pub rangeEdits: Vec<NativeRangeEdit>,
        pub processingHistory: Vec<NativeProcessingStep>,
        pub audioNoteSegments: Vec<NativeNoteSegment>,
        pub warpMarkers: Vec<NativeWarpMarker>,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeSidechainState {
        pub sourceId: u32,
        pub destinationId: u32,
        pub pluginIndex: u32,
        pub tapPoint: u32,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeRouteState {
        pub sourceId: u32,
        pub destinationId: u32,
        pub gain: f32,
        pub send: bool,
        pub preFader: bool,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeMarkerState {
        pub sample: u64,
        pub name: String,
        pub color: u32,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeArrangerPartState {
        pub start: u64,
        pub length: u64,
        pub name: String,
        pub repeats: u32,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeTempoEventState {
        pub sample: u64,
        pub bpm: f64,
        pub ramp: u8,
    }

    #[allow(non_snake_case)]
    #[derive(Clone, Default)]
    pub struct NativeProjectState {
        pub bpm: f64,
        pub rootNote: i32,
        pub scaleType: i32,
        pub tracks: Vec<NativeTrackState>,
        pub regions: Vec<NativeRegionState>,
        pub sampleRate: u32,
        pub routes: Vec<NativeRouteState>,
        pub sidechains: Vec<NativeSidechainState>,
        pub markers: Vec<NativeMarkerState>,
        pub arrangerParts: Vec<NativeArrangerPartState>,
        pub tempoEvents: Vec<NativeTempoEventState>,
        pub version: u32,
        pub valid: bool,
    }

    /// One provisional track the loader may install regions into: the id the
    /// document knows plus the engine's own handle, opaque to Rust.
    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeTrackHandle {
        pub id: u32,
        pub track: usize,
    }

    extern "Rust" {
        /// Decodes an `ARUA` container. An unreadable or malformed file yields
        /// a default state with `valid == false`.
        fn native_project_load(path: &str) -> NativeProjectState;
        /// Encodes and atomically stores an `ARUA` container.
        fn native_project_save(path: &str, state: &NativeProjectState) -> bool;
        /// Encodes and atomically stores an `ARUA` container whose timeline
        /// markers and arranger parts come from the Rust arrangement document.
        fn native_project_save_with_arrangement(
            path: &str,
            state: &NativeProjectState,
            arrangement_document: usize,
        ) -> bool;
        /// Installs every decoded project region into the provisional tracks the
        /// engine is building. False means the hydration aborted and the caller
        /// must keep the previous document published.
        fn native_project_hydrate_regions(
            state: &mut NativeProjectState,
            tracks: &Vec<NativeTrackHandle>,
            project_sample_rate: f64,
        ) -> bool;

        fn report_hirari_log(level: u32, msg: &str);

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
        /// Packs every sandbox status of the engine at `host` into the UI wire
        /// format. The engine pointer must remain valid for the call.
        fn sandbox_statuses_packed(host: usize) -> Vec<u32>;
        /// Admission error code of the most recent command on `host`.
        fn engine_last_command_error(host: usize) -> u8;
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
pub use native_project::{
    native_project_load, native_project_save, native_project_save_with_arrangement,
};
pub use region_hydration::native_project_hydrate_regions;

include!("lib_runtime.rs");
