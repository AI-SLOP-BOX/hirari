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
pub use runtime_views::runtime_health_vector;
pub use waveform_cache::{analysis_waveform_peak_envelope, resample_waveform_peak_envelope};
#[path = "plugins/ara2_protocol.rs"]
pub mod ara2_protocol;
#[path = "support/bridge_error.rs"]
pub mod bridge_error;
#[path = "commands/command_api.rs"]
pub mod command_api;
#[path = "commands/command_executor.rs"]
pub mod command_executor;
#[path = "engine/generation_gate.rs"]
pub mod generation_gate;
#[path = "engine/job_system.rs"]
pub mod job_system;
#[path = "session/production_events.rs"]
pub mod production_events;
#[path = "session/production_session.rs"]
pub mod production_session;
#[path = "session/production_timeline.rs"]
pub mod production_timeline;
#[path = "regions/regions.rs"]
pub mod regions;
#[path = "regions/vari_audio.rs"]
pub mod vari_audio;

#[path = "routing/spatial_orchestrator.rs"]
pub mod spatial_orchestrator;
#[path = "ui_bridge/stable_api.rs"]
pub mod stable_api;
#[path = "ui_bridge/vfx_bindings.rs"]
pub mod vfx_bindings;
#[path = "ui_bridge/vfx_timeline_bridge.rs"]
pub mod vfx_timeline_bridge;
#[path = "analysis/vocal_quality.rs"]
pub mod vocal_quality;
use anyhow::Context;
use engine_edit_ext::TrackEditExt;
use serde::Deserialize;
use std::path::{Path, PathBuf};
include!("module_exports_tail.rs");
use crate::summing::{SummingConfig, SummingOrchestrator};
pub use engine_native::{
    get_unified_engine, new_audio_engine, new_audio_engine_offline, push_command,
};
pub use engine_native::{AudioEngine, HirariUnifiedEngine};
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

    extern "Rust" {

        type AudioEngine;

        type HirariUnifiedEngine;

        // The inner engine is a per-session object that never changes identity.
        // Rust reaches its getters directly here instead of through an
        // AudioEngine forwarder per getter.
        fn get_core(self: &AudioEngine) -> &HirariUnifiedEngine;

        // --- LIFECYCLE: No more global singletons ---
        fn new_audio_engine() -> Box<AudioEngine>;
        fn new_audio_engine_offline() -> Box<AudioEngine>;

        fn join_audio_callbacks(self: &HirariUnifiedEngine);
        // Rebuilding the graph for a new device format stays in C++; Rust's
        // config transaction calls it once its own gates pass.
        #[cxx_name = "prepareToPlay"]
        fn prepare_to_play(self: &HirariUnifiedEngine, sample_rate: f64, block_size: u32);
        fn set_tempo(self: &HirariUnifiedEngine, bpm: f32) -> bool;
        fn set_master_gain(self: &HirariUnifiedEngine, value: f32) -> bool;
        fn control_room_state(self: &AudioEngine) -> usize;
        fn tempo_state(self: &AudioEngine) -> usize;
        fn undo_state(self: &AudioEngine) -> usize;
        fn log_state(self: &AudioEngine) -> usize;
        // The MIDI snapshot store is native to Rust already; the engine keeps
        // the handle its render callback pins, so hand that address back.
        fn midi_snapshot_store(self: &HirariUnifiedEngine) -> usize;
        fn midi_learn_state(self: &HirariUnifiedEngine) -> usize;
        fn preview_synth_state(self: &HirariUnifiedEngine) -> usize;
        fn mark_project_state_changed(self: &HirariUnifiedEngine);
        fn bus_track_exists(self: &HirariUnifiedEngine, bus_track_id: u32) -> bool;
        fn get_master_gain(self: &HirariUnifiedEngine) -> f32;
        fn bounce_session_state(self: &HirariUnifiedEngine) -> usize;
        fn audio_driver_host(self: &AudioEngine) -> usize;
        fn engine_block_state(self: &AudioEngine) -> usize;
        fn is_audio_device_ready(self: &AudioEngine) -> bool;
        fn try_reconnect_audio_device(self: &AudioEngine);
        // The device-config transaction itself, including device selection, is
        // Rust-owned (`engine_config.rs`); only the session mutex it shares with
        // the destructor, shutdown and reconnect paths is handed across here.
        fn get_audio_config_generation(self: &AudioEngine) -> u64;
        fn process_audio_block(self: &AudioEngine, left: &mut [f32], right: &mut [f32]);
        fn shutdown(self: &AudioEngine);
        fn new_project(self: &HirariUnifiedEngine);

        fn add_region(self: &AudioEngine, tid: u32, path: &str, start: f64) -> bool;

        fn set_plugin_bypass(
            self: &HirariUnifiedEngine,
            tid: u32,
            plugin_index: u32,
            bypassed: bool,
        ) -> bool;
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
        fn cancel_bounce(self: &HirariUnifiedEngine) -> bool;
        fn pause_bounce(self: &HirariUnifiedEngine) -> bool;
        fn resume_bounce(self: &HirariUnifiedEngine) -> bool;
        fn midi_clock_tick(self: &HirariUnifiedEngine, timestamp: u64);
        fn midi_clock_ticks(self: &HirariUnifiedEngine) -> u64;
        fn midi_clock_last_tick(self: &HirariUnifiedEngine) -> u64;
        fn midi_clock_rate(self: &HirariUnifiedEngine) -> f64;
        fn set_midi_clock_rate(self: &HirariUnifiedEngine, bpm: f64);
        fn execute_auto_arrangement(self: &HirariUnifiedEngine) -> bool;
        fn set_project_scale(self: &HirariUnifiedEngine, root: i32, scale_type: i32) -> bool;
        fn set_articulation_map(self: &HirariUnifiedEngine, tid: u32, map_hash: u32) -> bool;
        // Rust analysis reads (engine_analysis_reads.rs) sample telemetry and
        // meter straight from these Rust-owned stores.
        fn engine_block_state(self: &HirariUnifiedEngine) -> usize;
        fn meter_state(self: &HirariUnifiedEngine) -> usize;
        fn mark_project_layout_changed(self: &HirariUnifiedEngine);

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

    /// One provisional track the loader rebuilds its plugin chain and curves
    /// on: the document id and type plus the Rust stores the engine created for
    /// it, opaque here.
    #[allow(non_snake_case)]
    #[derive(Clone, Copy, Default)]
    pub struct NativeTrackStorages {
        pub id: u32,
        pub trackType: u32,
        pub chain: usize,
        pub automation: usize,
    }

    extern "Rust" {
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
pub use project_plugin_hydration::native_project_hydrate_plugins;
pub use region_hydration::native_project_hydrate_regions;

include!("runtime/lib_runtime.rs");
