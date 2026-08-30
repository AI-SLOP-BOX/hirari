use crate::bridge_error::BridgeError;
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, read};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProtocolRequest {
    pub protocol: String,
    pub request_id: String,
    pub client: String,
    pub command: CommandDocument,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProtocolResponse<T: Serialize> {
    pub protocol: String,
    pub request_id: String,
    pub ok: bool,
    pub result: Option<T>,
    pub error: Option<ProtocolError>,
}

/// Protocol errors and runtime/FFI errors intentionally share one wire type.
/// This prevents code/message/retry/generation context from being lost at
/// adapter boundaries.
pub type ProtocolError = BridgeError;

pub const PROTOCOL_VERSION: &str = "aura.command.v1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompSegmentCommand {
    pub take_id: u32,
    pub start_sample: u64,
    pub length_samples: u64,
    #[serde(default)]
    pub crossfade_samples: u32,
}

/// Validate the envelope before inspecting or applying its command. Keeping
/// this at the wire boundary prevents adapters from accidentally accepting a
/// command from a different protocol revision or an untraceable client.
pub fn validate_request_envelope(request: &ProtocolRequest) -> Result<(), BridgeError> {
    if request.protocol != PROTOCOL_VERSION {
        return Err(BridgeError::new(
            "unsupported_protocol",
            format!("expected {PROTOCOL_VERSION}, got {}", request.protocol),
        ));
    }
    if !valid_token(&request.request_id, 128) {
        return Err(BridgeError::new(
            "invalid_request_id",
            "request_id must be 1..=128 characters",
        ));
    }
    if !valid_token(&request.client, 128) {
        return Err(BridgeError::new(
            "invalid_client",
            "client must be 1..=128 characters",
        ));
    }
    Ok(())
}

fn valid_token(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

/// Stable generation for the exact project snapshot exposed to UI/CLI
/// clients.  Keep this in the protocol module so every adapter uses the same
/// scope and hashing rule instead of inventing a second generation source.
pub fn snapshot_generation(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "schema_versions": [1],
        "command_versions": [1],
        "modes": ["dry_run", "apply"],
        "permissions": ["read_only", "project_write", "system_write"],
        "privileged_permissions": [{"name": "unrestricted", "requires": "explicit_cli_flag_and_trusted_client", "audit_required": true}],
        "operations": ["project.init", "project.inspect", "project.search", "control.inspect", "analyze_dynamics", "analyze_mix", "analyze_silence", "apply_dynamics_suggestion", "generate_chord", "describe_drum_lane", "inspect_chord_track", "add_chord_event", "place_generated_chord", "remove_chord_events_range", "clear_chord_track", "suggest_next_chords", "generate_arpeggio", "place_arpeggio", "preview_vocal_pitch_correction", "extension.catalog", "extension.validate", "extension.invoke", "extension.set_enabled", "project.load", "add_track", "add_aux_track", "duplicate_track", "remove_track", "add_plugin", "remove_plugin", "move_plugin", "insert_named_plugin", "insert_plugin_path", "set_plugin_parameter", "set_plugin_bypass", "set_plugin_favorite", "plugin_search", "freeze_track", "freeze_track_to_project_end", "unfreeze_track", "track_freeze_status", "set_macro_value", "add_macro_mapping", "remove_macro_mapping", "add_midi_learn_mapping", "remove_midi_learn_mapping", "humanize_midi", "apply_midi_swing", "quantize_midi", "apply_midi_logical_rule", "open_utau_import", "open_utau_notes", "open_utau_import_midi", "add_audio_region", "replace_region_audio", "set_volume", "set_master_gain", "set_track_delay", "set_track_delay_automation", "set_track_stack", "delete_track_stack", "set_track_stack_gain", "set_track_stack_collapsed", "add_vca_group", "assign_track_to_vca", "set_vca_group_gain", "upsert_marker", "delete_marker", "set_pan", "set_mute", "set_solo", "set_track_armed", "set_phase_invert", "set_automation", "set_route", "set_route_gain", "set_feedback_route", "set_sidechain_link", "set_midi_note", "clear_midi_notes", "remove_midi_notes_range", "transpose_midi_notes_range", "move_midi_notes_range", "move_region", "split_region", "duplicate_region", "remove_region", "set_region_warp", "set_region_gain", "set_region_pitch", "set_track_name", "set_time_signature", "select_recording_take", "register_comp_take", "select_comp_take", "remove_comp_take", "set_comp_segments", "split_region_with_crossfade", "transport_play", "transport_pause", "transport_stop", "set_playhead", "set_loop", "set_cycle_range", "set_metronome", "set_tempo", "record_arm", "record_start", "record_stop", "record_commit", "save_project", "bounce_project", "bounce_stems", "render_target_catalog", "undo", "redo", "project_inspect", "plugin_catalog", "history.status", "history.log", "history.diff", "history.commit", "history.branch", "history.checkout", "history.tag", "history.revert", "history.cherry_pick"],
        "mix_assistant_operations": ["analyze_dynamics", "apply_gain_staging", "analyze_mix"],
        "standard_effect_operations": ["set_eq"],
        "dynamics_effect_operations": ["add_plugin", "Aura/Limiter", "Aura/Compressor", "Aura/Gate", "Aura/Saturation", "Aura/Transient", "Aura/DeEsser", "Aura/Delay", "Aura/Reverb", "Aura/DynamicEQ", "Aura/MidSide", "Aura/Width"],
        "comping_operations": ["select_recording_take", "register_comp_take", "select_comp_take", "remove_comp_take", "set_comp_segments"],
        "plugin_operations": ["plugin_catalog", "plugin_search", "set_plugin_favorite", "cache_invalidation_by_binary_hash"],
        "vca_operations": ["add_vca_group", "assign_track_to_vca", "set_vca_group_gain"],
        "routing_operations": ["add_track", "add_aux_track", "set_route", "set_sidechain_link"],
        "latency_operations": ["set_low_latency_mode"],
        "tonal_operations": ["set_tonal_scale"],
        "composition_operations": ["generate_chord", "suggest_next_chords", "generate_arpeggio", "inspect_chord_track", "add_chord_event", "place_generated_chord", "remove_chord_events_range", "clear_chord_track"],
        "vocal_operations": ["preview_vocal_pitch_correction", "open_utau_import", "set_midi_note_articulation"],
        "audio_edit_operations": ["analyze_silence", "split_region", "split_region_with_crossfade", "set_region_fades", "set_region_warp", "set_region_pitch"],
        "mix_analysis_operations": ["analyze_mix", "analyze_dynamics", "analyze_silence"],
        "mix_snapshot_operations": ["take_mix_snapshot", "capture_mix_snapshot", "diff_mix_snapshots", "recall_mix_snapshot", "apply_mix_snapshot"],
        "midi_editor_operations": ["inspect_midi_notes", "describe_drum_lane"],
        "midi_read_operations": ["inspect_midi_notes"],
        "render_capabilities": {
            "backend": "native_audio_engine",
            "supported_codecs": ["wav_pcm16", "wav_rf64", "wave64_float32", "aiff_pcm16", "flac", "mp3"],
            "unsupported_codecs": ["aac"],
            "external_codec_provider": "ffmpeg",
            "supported_import_formats": ["wav", "rf64", "mp3", "flac", "aif", "aiff", "m4a", "ogg", "aac"],
            "status": "available",
            "async": true,
            "cancellation": true,
            "generation_guarded_publication": true
        },
        "transport_capabilities": {
            "play": true,
            "pause": true,
            "stop": true,
            "seek": true,
            "pause_preserves_playhead": true
        },
        "history_capabilities": {
            "schema_version": 1,
            "content_hash": "sha256",
            "entity_level_diff": true,
            "field_level_diff": true,
            "large_state_fields_hashed_only": true,
            "supported_sections": ["metadata", "tracks", "regions", "plugin_instances", "midi_learn_mappings", "midi_notes", "chord_track", "macro_mappings", "warp_markers", "render_targets", "openutau_vocals", "freeze_artifacts", "sidechain_routes", "feedback_routes", "audio_routes"]
        },
        "integration_capabilities": {
            "ara2": {
                "status": "scaffold",
                "timeline_random_access": true,
                "plugin_protocol_bridge": false,
                "external_sdk_required": true,
                "verified": false
            },
            "hardware_controllers": {
                "status": "protocol_core",
                "protocols": ["mcu", "hui", "eucon", "osc"],
                "device_driver_integration": false,
                "verified": false
            },
            "factory_content": {
                "status": "project_local",
                "content_delivery": false,
                "license_manifest": false,
                "verified": false
            },
            "immersive_audio": {
                "status": "engine_scaffold",
                "layouts": ["stereo", "surround", "7.1.4"],
                "dolby_renderer": false,
                "sony_360_renderer": false,
                "metadata_export": false,
                "hardware_renderer_link": false,
                "verified": false
            }
        },
        "extensions": {
            "status": "manifest_discovery_and_bounded_trusted_invocation",
            "catalog_includes": ["commands", "panels", "menus"],
            "execution_modes": ["sandboxed", "trusted_explicit_approval"],
            "permissions": ["project_read", "project_write", "audio_process", "filesystem_external", "network", "process_spawn"],
            "symlink_following": false,
            "code_execution": "trusted_only_with_explicit_process_spawn",
            "trusted_activation": "explicit_user_approval_and_audit_log"
        },
        "transport": { "input": "jsonl", "output": "jsonl", "one_request_per_line": true },
        "midi_capabilities": {
            "midi_1_cc": true,
            "midi_1_sysex": true,
            "midi_2_channel_voice": true,
            "max_sysex_payload_bytes": 4096,
            "midi_2_value_bits": 32
        },
        "clients": {
            "computer_use": {
                "supported": true,
                "interaction": "ui_or_command_api",
                "power_profile": "unrestricted_explicit",
                "requires": ["request_id", "transaction", "generation", "audit_log"]
            }
        },
        "safety": {
            "approval_required_for_apply": true,
            "unknown_operations_rejected": true,
            "computer_use_may_bypass_project_root_only_with_unrestricted": true
        }
    })
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CommandDocument {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    #[serde(default = "current_command_version")]
    pub command_version: u32,
    pub transaction: String,
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub expected_generation: Option<u64>,
    #[serde(default)]
    pub expected_audio_generation: Option<u64>,
    pub actions: Vec<CommandAction>,
}

fn current_schema_version() -> u32 {
    1
}
fn current_command_version() -> u32 {
    1
}

fn default_one() -> f32 {
    1.0
}

fn default_stem_tail_seconds() -> f32 {
    2.0
}

fn default_include_inserts() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    #[default]
    ReadOnly,
    ProjectWrite,
    SystemWrite,
    /// Explicit opt-in for power users and trusted local automation. This
    /// bypasses project-root path containment but never bypasses request
    /// logging, generation checks, or transaction/idempotency requirements.
    Unrestricted,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathPolicy {
    ProjectOnly,
    Unrestricted,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationClass {
    ReadOnly,
    Reversible,
    Irreversible,
    ExternalSideEffect,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CommandAction {
    ControlInspect,
    /// Return the canonical piano-roll/vocal note list, including lyrics.
    InspectMidiNotes,
    /// Return the canonical persisted chord-track event list.
    InspectChordTrack,
    /// Add one chord event to the canonical chord track.
    AddChordEvent {
        tick: u64,
        root: u8,
        intervals: Vec<u8>,
        name: String,
    },
    /// Place a generated voicing into a MIDI track.
    PlaceGeneratedChord {
        track_id: u32,
        start_sample: u64,
        length_samples: u64,
        velocity: u8,
        root: i32,
        octave: i32,
        quality: u32,
    },
    RemoveChordEventsRange {
        start_tick: u64,
        end_tick: u64,
    },
    ClearChordTrack,
    /// Expand a code-pad chord into a deterministic MIDI voicing.
    GenerateChord {
        root: i32,
        octave: i32,
        quality: u32,
    },
    SuggestNextChords {
        last_chord_name: String,
    },
    GenerateArpeggio {
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    },
    PlaceArpeggio {
        track_id: u32,
        start_sample: u64,
        step_samples: u64,
        gate_samples: u64,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    },
    /// Resolve a General MIDI percussion pitch to a stable drum-lane label.
    DescribeDrumLane {
        pitch: u8,
    },
    /// Analyze a bounded audio sample window and return deterministic
    /// compressor/gate suggestions without mutating the project.
    AnalyzeDynamics {
        samples: Vec<f32>,
        /// Optional owner track for UI/API diagnostics.  The analysis remains
        /// read-only and can still be used as a generic sample-window check.
        #[serde(default)]
        track_id: Option<u32>,
    },
    /// Analyze bounded stereo mix metrics without mutating project state.
    AnalyzeMix {
        left: Vec<f32>,
        right: Vec<f32>,
        #[serde(default)]
        reference_left: Vec<f32>,
        #[serde(default)]
        reference_right: Vec<f32>,
        #[serde(default)]
        ab_left: Vec<f32>,
        #[serde(default)]
        ab_right: Vec<f32>,
    },
    /// Find non-destructive silence ranges for event splitting.
    AnalyzeSilence {
        samples: Vec<f32>,
        #[serde(default = "default_silence_threshold")]
        threshold: f32,
        #[serde(default = "default_silence_min_length")]
        min_length: u32,
    },
    /// Split one audio region at the boundaries of detected silent runs.
    SplitRegionAtSilence {
        track_id: u32,
        region_id: u32,
        samples: Vec<f32>,
        #[serde(default = "default_silence_threshold")]
        threshold: f32,
        #[serde(default = "default_silence_min_length")]
        min_length: u32,
    },
    /// Produce a bounded, non-destructive vocal pitch-correction preview.
    PreviewVocalPitchCorrection {
        samples: Vec<f32>,
        #[serde(default = "default_preview_sample_rate")]
        sample_rate: f64,
        #[serde(default = "default_preview_speed")]
        speed: f32,
        #[serde(default = "default_preview_timing_ratio")]
        timing_ratio: f32,
    },
    /// Analyze a sample window and apply the four core compressor settings
    /// to an internal Aura/Compressor in one undo transaction.
    ApplyDynamicsSuggestion {
        track_id: u32,
        plugin_index: u32,
        samples: Vec<f32>,
    },
    /// Search the canonical project snapshot without mutating it.
    ProjectSearch {
        query: String,
    },
    ExtensionCatalog {
        root: String,
    },
    ExtensionValidate {
        root: String,
        extension_id: String,
        command_id: String,
        payload: serde_json::Value,
    },
    ExtensionInvoke {
        root: String,
        extension_id: String,
        command_id: String,
        payload: serde_json::Value,
        #[serde(default = "default_extension_timeout_ms")]
        timeout_ms: u64,
    },
    ExtensionSetEnabled {
        root: String,
        extension_id: String,
        enabled: bool,
    },
    AddTrack {
        name: String,
        #[serde(default)]
        track_type: u32,
    },
    AddAuxTrack {
        name: String,
    },
    RemoveTrack {
        track_id: u32,
    },
    DuplicateTrack {
        track_id: u32,
    },
    AddVcaGroup {
        group_id: u32,
        #[serde(default = "default_one")]
        gain: f32,
    },
    AssignTrackToVca {
        track_id: u32,
        group_id: u32,
    },
    SetVcaGroupGain {
        group_id: u32,
        gain: f32,
    },
    SetPluginFavorite {
        id: String,
        favorite: bool,
    },
    PluginSearch {
        #[serde(default)]
        query: String,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        favorites_only: bool,
    },
    AddPlugin {
        track_id: u32,
        plugin_type: u32,
    },
    FreezeTrack {
        track_id: u32,
        total_samples: u64,
        /// Optional project-local cache path. Without it the freeze remains
        /// an in-memory runtime snapshot.
        #[serde(default)]
        path: Option<String>,
    },
    /// Freeze using the current project end, avoiding callers having to
    /// guess the render length from a stale UI snapshot.
    FreezeTrackToProjectEnd {
        track_id: u32,
    },
    UnfreezeTrack {
        track_id: u32,
    },
    TrackFreezeStatus {
        track_id: u32,
    },
    RemovePlugin {
        track_id: u32,
        plugin_index: u32,
    },
    MovePlugin {
        track_id: u32,
        from_index: u32,
        to_index: u32,
    },
    SetPluginParameter {
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        value: f32,
    },
    SetPluginBypass {
        track_id: u32,
        plugin_index: u32,
        bypassed: bool,
    },
    SetMacroValue {
        macro_index: u32,
        value: f32,
    },
    AddMacroMapping {
        mapping_id: String,
        macro_index: u32,
        target_instance_id: String,
        target_parameter_id: String,
        #[serde(default)]
        min: f32,
        #[serde(default = "default_one")]
        max: f32,
        #[serde(default)]
        curve: f32,
        #[serde(default)]
        invert: bool,
    },
    RemoveMacroMapping {
        mapping_id: String,
    },
    AddMidiLearnMapping {
        mapping_id: String,
        device_id: String,
        channel: u32,
        controller: u32,
        target_instance_id: String,
        target_parameter_id: String,
        #[serde(default)]
        min: f32,
        #[serde(default = "default_one")]
        max: f32,
        #[serde(default)]
        curve: f32,
        #[serde(default)]
        pickup: bool,
    },
    RemoveMidiLearnMapping {
        mapping_id: String,
    },
    HumanizeMidi {
        timing_beats: f32,
        velocity: i32,
        seed: u64,
    },
    QuantizeMidi {
        grid_beats: f32,
        strength: f32,
    },
    ApplyMidiSwing {
        subdivision_beats: f32,
        amount: f32,
    },
    /// Apply a validated, data-driven Logical Editor rule to all canonical MIDI notes.
    ApplyMidiLogicalRule {
        rule: crate::midi_logical_editor::MidiLogicalRule,
    },
    TakeMixSnapshot {
        name: String,
        #[serde(default)]
        states: std::collections::HashMap<u32, f32>,
    },
    CaptureMixSnapshot {
        name: String,
    },
    DiffMixSnapshots {
        first: usize,
        second: usize,
    },
    RecallMixSnapshot {
        index: usize,
    },
    ApplyMixSnapshot {
        index: usize,
    },
    InsertNamedPlugin {
        track_id: u32,
        alias: String,
    },
    /// Insert a concrete installed plugin bundle.  This is the extensibility
    /// path for plugins that are not yet known by the catalog alias list.
    InsertPluginPath {
        track_id: u32,
        path: String,
    },
    #[serde(alias = "openutau_import")]
    OpenUtauImport {
        track_id: u32,
        source_path: String,
        rendered_audio_path: String,
    },
    /// Read-only structured note inspection for the in-DAW vocal editor.
    OpenUtauNotes {
        source_path: String,
    },
    /// Import the structured UST/USTX note stream into the canonical project
    /// MIDI model. The rendered vocal remains a separate audio-region import.
    OpenUtauImportMidi {
        track_id: u32,
        source_path: String,
        sample_rate: u32,
        ticks_per_beat: u32,
    },
    AddAudioRegion {
        track_id: u32,
        path: String,
        start: f64,
    },
    ReplaceRegionAudio {
        track_id: u32,
        region_id: u32,
        path: String,
    },
    PluginCatalog,
    SetVolume {
        track_id: u32,
        value: f32,
    },
    SetEq {
        track_id: u32,
        low_band: f32,
        low_cut: f32,
        high_band: f32,
        high_cut: f32,
    },
    /// Apply a bounded, relative gain-staging correction to a track fader.
    ApplyGainStaging {
        track_id: u32,
        gain_db: f32,
    },
    SetMasterGain {
        value: f32,
    },
    SetTrackDelay {
        track_id: u32,
        samples: u32,
    },
    SetLowLatencyMode {
        enabled: bool,
    },
    SetTonalScale {
        root: i32,
        scale_type: u32,
    },
    CreateTrackStack {
        stack_id: u32,
        name: String,
        member_track_ids: Vec<u32>,
        #[serde(default = "default_one")]
        master_gain: f32,
        #[serde(default)]
        collapsed: bool,
    },
    DeleteTrackStack {
        stack_id: u32,
    },
    UpsertMarker {
        marker_id: u32,
        label: String,
        beat: f64,
        #[serde(default)]
        color: String,
    },
    DeleteMarker {
        marker_id: u32,
    },
    SetTrackStackGain {
        stack_id: u32,
        master_gain: f32,
    },
    SetTrackStackCollapsed {
        stack_id: u32,
        collapsed: bool,
    },
    SetPan {
        track_id: u32,
        value: f32,
    },
    SetMute {
        track_id: u32,
        muted: bool,
    },
    SetSolo {
        track_id: u32,
        solo: bool,
    },
    SetTrackArmed {
        track_id: u32,
        armed: bool,
    },
    SetPhaseInvert {
        track_id: u32,
        inverted: bool,
    },
    SetRoute {
        source_id: u32,
        dest_id: u32,
        enabled: bool,
    },
    /// Set the gain of a normal audio route.  This is separate from
    /// feedback-route gain because normal sends are allowed to participate
    /// in the ordinary acyclic graph and must retain their own undo record.
    SetRouteGain {
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    },
    SetFeedbackRoute {
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    },
    SetSidechainLink {
        source_id: u32,
        dest_id: u32,
        tap_point: u32,
        plugin_index: u32,
        enabled: bool,
    },
    MoveRegion {
        track_id: u32,
        region_id: u32,
        start: f64,
    },
    SplitRegion {
        track_id: u32,
        region_id: u32,
        beat: f64,
    },
    SplitRegionWithCrossfade {
        track_id: u32,
        region_id: u32,
        beat: f64,
        ratio: f32,
    },
    DuplicateRegion {
        track_id: u32,
        region_id: u32,
        start: f64,
    },
    RemoveRegion {
        track_id: u32,
        region_id: u32,
    },
    SetRegionFades {
        track_id: u32,
        region_id: u32,
        fade_in: f32,
        fade_out: f32,
    },
    SetRegionTrim {
        track_id: u32,
        region_id: u32,
        start: f32,
        end: f32,
    },
    SetRegionLoop {
        track_id: u32,
        region_id: u32,
        count: u32,
    },
    SetRegionReverse {
        track_id: u32,
        region_id: u32,
        reverse: bool,
    },
    SetRegionMuted {
        track_id: u32,
        region_id: u32,
        muted: bool,
    },
    TransportPlay,
    TransportPause,
    TransportStop,
    SetPlayhead {
        position: u64,
    },
    SetLoop {
        enabled: bool,
    },
    SetMetronome {
        enabled: bool,
    },
    SetCycleRange {
        start_sample: u64,
        end_sample: u64,
        enabled: bool,
    },
    RecordArm {
        sample_rate: f32,
        channels: u16,
        max_frames: u64,
    },
    RecordStart {
        sample_rate: f32,
        channels: u16,
        max_frames: u64,
        start_sample: u64,
        /// Input frames to consume before opening the recording take.
        /// Defaults to zero for the legacy immediate-start behavior.
        #[serde(default)]
        count_in_frames: u64,
    },
    RecordStop,
    SelectRecordingTake {
        index: u32,
    },
    RegisterCompTake {
        take_id: u32,
        name: String,
        start_sample: u64,
        end_sample: u64,
    },
    SelectCompTake {
        take_id: u32,
    },
    RemoveCompTake {
        take_id: u32,
    },
    SetCompSegments {
        segments: Vec<CompSegmentCommand>,
    },
    RecordCommit {
        track_id: u32,
        project_path: Option<String>,
    },
    SetTempo {
        bpm: f32,
    },
    SetTimeSignature {
        beat: f64,
        numerator: u8,
        denominator: u8,
    },
    SetAutomation {
        track_id: u32,
        parameter_id: u32,
        /// Flat [time_samples, value, curve, ...] triples. Times are
        /// strictly increasing integer sample positions; values are 0..=1.
        points: Vec<f64>,
    },
    SetTrackDelayAutomation {
        track_id: u32,
        /// Flat [time_samples, normalized_delay, curve, ...] triples.
        points: Vec<f64>,
    },
    SetMidiNote {
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        #[serde(default)]
        lyric: String,
        #[serde(default)]
        phoneme: String,
        #[serde(default)]
        pitch_curve_cents: Vec<i16>,
        #[serde(default)]
        vibrato_depth_cents: u16,
        #[serde(default)]
        portamento_samples: u32,
    },
    ClearMidiNotes,
    RemoveMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
    },
    TransposeMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    },
    MoveMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    },
    SetRegionWarp {
        track_id: u32,
        region_id: u32,
        ratio: f64,
    },
    SetRegionGain {
        track_id: u32,
        region_id: u32,
        gain_db: f32,
    },
    SetRegionPitch {
        track_id: u32,
        region_id: u32,
        semitones: f32,
    },
    SetRegionAudioNoteSegment {
        track_id: u32,
        region_id: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        #[serde(default)]
        formant_offset_cents: f64,
    },
    ClearRegionAudioNoteSegments {
        track_id: u32,
        region_id: u32,
    },
    SetTrackName {
        track_id: u32,
        name: String,
    },
    Undo,
    Redo,
    ProjectInspect,
    RenderTargetCatalog,
    ProjectLoad {
        path: String,
    },
    SaveProject {
        path: String,
    },
    BounceProject {
        path: String,
        #[serde(default)]
        format: u32,
    },
    BounceStems {
        output_dir: String,
        #[serde(default)]
        format: u32,
        /// Optional explicit render targets. An empty list preserves the
        /// legacy behaviour of exporting every audio track.
        #[serde(default)]
        track_ids: Vec<u32>,
        /// Tail appended after the project end, in seconds.
        #[serde(default = "default_stem_tail_seconds")]
        tail_seconds: f32,
        #[serde(default)]
        pre_fader: bool,
        #[serde(default = "default_include_inserts")]
        include_inserts: bool,
    },
}

fn default_extension_timeout_ms() -> u64 {
    5_000
}
fn default_preview_sample_rate() -> f64 {
    48_000.0
}
fn default_preview_speed() -> f32 {
    1.0
}
fn default_preview_timing_ratio() -> f32 {
    1.0
}
fn default_silence_threshold() -> f32 {
    0.001
}
fn default_silence_min_length() -> u32 {
    256
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidatedCommand {
    pub schema_version: u32,
    pub command_version: u32,
    pub transaction: String,
    pub permission: Permission,
    pub expected_generation: Option<u64>,
    pub expected_audio_generation: Option<u64>,
    pub actions: Vec<CommandAction>,
    pub destructive: bool,
    pub mutation_class: MutationClass,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CommandDiff {
    pub index: usize,
    pub summary: String,
    pub destructive: bool,
}

pub fn diff(command: &ValidatedCommand) -> Vec<CommandDiff> {
    command
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let summary = match action {
                CommandAction::ControlInspect => "inspect macro and MIDI control mappings".into(),
                CommandAction::InspectMidiNotes => "inspect canonical MIDI notes and lyrics".into(),
                CommandAction::InspectChordTrack => "inspect canonical chord track".into(),
                CommandAction::AddChordEvent { tick, root, name, .. } => format!("add chord event {name} at tick {tick} root {root}"),
                CommandAction::PlaceGeneratedChord { track_id, start_sample, .. } => format!("place generated chord on track {track_id} at sample {start_sample}"),
                CommandAction::SuggestNextChords { last_chord_name } => format!("suggest next chords after {last_chord_name}"),
                CommandAction::GenerateArpeggio { steps, pattern, .. } => format!("generate arpeggio {steps} steps pattern {pattern}"),
                CommandAction::PlaceArpeggio { track_id, start_sample, steps, .. } => format!("place arpeggio on track {track_id} at {start_sample} for {steps} steps"),
                CommandAction::RemoveChordEventsRange { start_tick, end_tick } => format!("remove chord events from tick {start_tick} to {end_tick}"),
                CommandAction::ClearChordTrack => "clear chord track".into(),
                CommandAction::GenerateChord { root, octave, quality } => format!("generate chord root {root}, octave {octave}, quality {quality}"),
                CommandAction::DescribeDrumLane { pitch } => format!("describe drum lane pitch {pitch}"),
                CommandAction::AnalyzeDynamics { samples, track_id } => format!("analyze {} audio samples for dynamics suggestions{}", samples.len(), track_id.map(|id| format!(" on track {id}")).unwrap_or_default()),
                CommandAction::AnalyzeMix { left, right, .. } => format!("analyze stereo mix ({} left / {} right samples)", left.len(), right.len()),
                CommandAction::AnalyzeSilence { samples, threshold, min_length } => format!("analyze {} audio samples for silence at threshold {threshold} (minimum {min_length})", samples.len()),
                CommandAction::SplitRegionAtSilence { track_id, region_id, samples, .. } => format!("split region {region_id} on track {track_id} at detected silence ({} samples)", samples.len()),
                CommandAction::PreviewVocalPitchCorrection { samples, sample_rate, speed, timing_ratio } => format!("preview vocal pitch/timing correction for {} samples at {sample_rate} Hz speed {speed} timing {timing_ratio}", samples.len()),
                CommandAction::ApplyDynamicsSuggestion { track_id, plugin_index, samples } => format!("apply dynamics suggestion to plugin {plugin_index} on track {track_id} from {} samples", samples.len()),
                CommandAction::ProjectSearch { query } => format!("search project for: {query}"),
                CommandAction::ExtensionCatalog { root } => format!("discover extensions in {root}"),
                CommandAction::ExtensionInvoke { extension_id, command_id, .. } => format!("invoke extension command {extension_id}.{command_id}"),
                CommandAction::ExtensionValidate { extension_id, command_id, .. } => format!("validate extension command {extension_id}.{command_id}"),
                CommandAction::ExtensionSetEnabled { extension_id, enabled, .. } => format!("{} extension {extension_id}", if *enabled { "enable" } else { "disable" }),
                CommandAction::AddTrack { name, .. } => format!("add track: {name}"),
                CommandAction::AddAuxTrack { name } => format!("add Aux track: {name}"),
                CommandAction::RemoveTrack { track_id } => format!("remove track {track_id}"),
                CommandAction::DuplicateTrack { track_id } => format!("duplicate track {track_id}"),
                CommandAction::AddVcaGroup { group_id, gain } => format!("add VCA group {group_id} at gain {gain}"),
                CommandAction::AssignTrackToVca { track_id, group_id } => format!("assign track {track_id} to VCA group {group_id}"),
                CommandAction::SetVcaGroupGain { group_id, gain } => format!("set VCA group {group_id} gain to {gain}"),
                CommandAction::SetPluginFavorite { id, favorite } => format!("{} plugin favorite: {id}", if *favorite { "set" } else { "clear" }),
                CommandAction::PluginSearch { query, tag, favorites_only } => format!("search plugins query={query:?} tag={tag:?} favorites_only={favorites_only}"),
                CommandAction::AddPlugin {
                    track_id,
                    plugin_type,
                } => format!("insert plugin type {plugin_type} on track {track_id}"),
                CommandAction::FreezeTrack { track_id, total_samples, path } => format!("freeze track {track_id} for {total_samples} samples{}", path.as_deref().map(|value| format!(" into {value}")).unwrap_or_default()),
                CommandAction::FreezeTrackToProjectEnd { track_id } => format!("freeze track {track_id} to project end"),
                CommandAction::UnfreezeTrack { track_id } => format!("unfreeze track {track_id}"),
                CommandAction::TrackFreezeStatus { track_id } => format!("inspect freeze status for track {track_id}"),
                CommandAction::RemovePlugin { track_id, plugin_index } => format!("remove plugin {plugin_index} from track {track_id}"),
                CommandAction::MovePlugin { track_id, from_index, to_index } => format!("move plugin {from_index} to {to_index} on track {track_id}"),
                CommandAction::SetPluginParameter { track_id, plugin_index, parameter_id, value } => format!("set plugin {plugin_index} parameter {parameter_id} on track {track_id}: {value}"),
                CommandAction::SetPluginBypass { track_id, plugin_index, bypassed } => format!("{} plugin {plugin_index} on track {track_id}", if *bypassed { "bypass" } else { "enable" }),
                CommandAction::SetMacroValue { macro_index, value } => format!("set macro {macro_index}: {value}"),
                CommandAction::AddMacroMapping { mapping_id, macro_index, target_instance_id, target_parameter_id, .. } => format!("map macro {macro_index} ({mapping_id}) to {target_instance_id}:{target_parameter_id}"),
                CommandAction::RemoveMacroMapping { mapping_id } => format!("remove macro mapping {mapping_id}"),
                CommandAction::AddMidiLearnMapping { mapping_id, device_id, controller, target_instance_id, target_parameter_id, .. } => format!("map MIDI {device_id}:{controller} ({mapping_id}) to {target_instance_id}:{target_parameter_id}"),
                CommandAction::RemoveMidiLearnMapping { mapping_id } => format!("remove MIDI mapping {mapping_id}"),
                CommandAction::HumanizeMidi { timing_beats, velocity, seed } => format!("humanize MIDI timing {timing_beats}, velocity {velocity}, seed {seed}"),
                CommandAction::ApplyMidiSwing { subdivision_beats, amount } => format!("apply MIDI swing {amount} at {subdivision_beats} beats"),
                CommandAction::QuantizeMidi { grid_beats, strength } => format!("quantize MIDI to {grid_beats} beats ({strength})"),
                CommandAction::ApplyMidiLogicalRule { .. } => "apply MIDI Logical Editor rule".into(),
                CommandAction::TakeMixSnapshot { name, .. } => format!("capture MixConsole snapshot {name}"),
                CommandAction::CaptureMixSnapshot { name } => format!("capture live MixConsole snapshot {name}"),
                CommandAction::DiffMixSnapshots { first, second } => format!("compare MixConsole snapshots {first} and {second}"),
                CommandAction::RecallMixSnapshot { index } => format!("recall MixConsole snapshot {index}"),
                CommandAction::ApplyMixSnapshot { index } => format!("apply MixConsole snapshot {index}"),
                CommandAction::InsertNamedPlugin { track_id, alias } =>
                    format!("insert installed plugin {alias} on track {track_id}"),
                CommandAction::InsertPluginPath { track_id, path } =>
                    format!("insert plugin bundle {path} on track {track_id}"),
                CommandAction::OpenUtauImport { track_id, source_path, rendered_audio_path } =>
                    format!("attach OpenUtau source {source_path} and render {rendered_audio_path} to track {track_id}"),
                CommandAction::OpenUtauNotes { source_path } =>
                    format!("inspect OpenUtau notes from {source_path}"),
                CommandAction::OpenUtauImportMidi { track_id, source_path, .. } =>
                    format!("import OpenUtau MIDI notes from {source_path} into track {track_id}"),
                CommandAction::AddAudioRegion { track_id, path, start } => format!("add audio region {path} to track {track_id} at {start}"),
                CommandAction::ReplaceRegionAudio { track_id, region_id, path } => format!("replace audio for region {region_id} on track {track_id}: {path}"),
                CommandAction::PluginCatalog => "inspect installed plugin catalog".into(),
                CommandAction::SetVolume { track_id, value } => {
                    format!("set track {track_id} volume: {value}")
                }
                CommandAction::ApplyGainStaging { track_id, gain_db } => format!("apply {gain_db:.2} dB gain staging to track {track_id}"),
                CommandAction::SetEq { track_id, .. } => format!("set EQ on track {track_id}"),
                CommandAction::SetTrackDelay { track_id, samples } => {
                    format!("set track {track_id} delay: {samples} samples")
                }
                CommandAction::SetLowLatencyMode { enabled } => {
                    format!("{} low-latency monitoring", if *enabled { "enable" } else { "disable" })
                }
                CommandAction::SetTonalScale { root, scale_type } => {
                    format!("set tonal scale root {root}, type {scale_type}")
                }
                CommandAction::SetPan { track_id, value } => {
                    format!("set track {track_id} pan: {value}")
                }
                CommandAction::SetMute { track_id, muted } => format!("set track {track_id} mute: {muted}"),
                CommandAction::SetSolo { track_id, solo } => format!("set track {track_id} solo: {solo}"),
                CommandAction::SetTrackArmed { track_id, armed } => format!("{} recording arm for track {track_id}", if *armed { "enable" } else { "disable" }),
                CommandAction::SetPhaseInvert { track_id, inverted } => format!("{} phase invert on track {track_id}", if *inverted { "enable" } else { "disable" }),
                CommandAction::SetRoute { source_id, dest_id, enabled } => format!("{} route {source_id} -> {dest_id}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetRouteGain { source_id, dest_id, gain, enabled } => format!("{} route {source_id} -> {dest_id} gain {gain}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetFeedbackRoute { source_id, dest_id, gain, enabled } => format!("{} feedback route {source_id} -> {dest_id} gain {gain}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetSidechainLink { source_id, dest_id, tap_point, plugin_index, enabled } => format!("{} sidechain {source_id} -> {dest_id} tap {tap_point} plugin {plugin_index}", if *enabled { "enable" } else { "disable" }),
                CommandAction::MoveRegion { track_id, region_id, start } => format!("move region {region_id} on track {track_id} to beat {start}"),
                CommandAction::SplitRegion { track_id, region_id, beat } => format!("split region {region_id} on track {track_id} at beat {beat}"),
                CommandAction::SplitRegionWithCrossfade { track_id, region_id, beat, ratio } => format!("split region {region_id} on track {track_id} at beat {beat} with crossfade {ratio}"),
                CommandAction::DuplicateRegion { track_id, region_id, start } => format!("duplicate region {region_id} on track {track_id} at beat {start}"),
                CommandAction::RemoveRegion { track_id, region_id } => format!("remove region {region_id} from track {track_id}"),
                CommandAction::SetRegionFades { track_id, region_id, fade_in, fade_out } => format!("set region {region_id} on track {track_id} fades: {fade_in}/{fade_out}"),
                CommandAction::SetRegionTrim { track_id, region_id, start, end } => format!("trim region {region_id} on track {track_id}: {start}..{end}"),
                CommandAction::SetRegionLoop { track_id, region_id, count } => format!("loop region {region_id} on track {track_id}: {count}"),
                CommandAction::SetRegionReverse { track_id, region_id, reverse } => format!("{} reverse region {region_id} on track {track_id}", if *reverse { "enable" } else { "disable" }),
                CommandAction::SetRegionMuted { track_id, region_id, muted } => format!("{} mute region {region_id} on track {track_id}", if *muted { "enable" } else { "disable" }),
                CommandAction::TransportPlay => "start transport".into(),
                CommandAction::TransportPause => "pause transport".into(),
                CommandAction::TransportStop => "stop transport".into(),
                CommandAction::SetPlayhead { position } => format!("set playhead: {position}"),
                CommandAction::SetLoop { enabled } => format!("{} cycle loop", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetMetronome { enabled } => format!("{} metronome", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetCycleRange { start_sample, end_sample, enabled } => format!("{} cycle range {start_sample}..{end_sample}", if *enabled { "enable" } else { "disable" }),
                CommandAction::RecordArm { sample_rate, channels, max_frames } => format!("arm recording: {sample_rate} Hz, {channels} ch, {max_frames} frames"),
                CommandAction::RecordStart { sample_rate, channels, max_frames, start_sample, count_in_frames } => format!("start recording: {sample_rate} Hz, {channels} ch from {start_sample} for {max_frames} frames (count-in {count_in_frames})"),
                CommandAction::RecordStop => "stop recording capture".into(),
                CommandAction::SelectRecordingTake { index } => format!("select recording take {index}"),
                CommandAction::RegisterCompTake { take_id, name, .. } => format!("register comp take {take_id}: {name}"),
                CommandAction::SelectCompTake { take_id } => format!("select comp take {take_id}"),
                CommandAction::RemoveCompTake { take_id } => format!("remove comp take {take_id}"),
                CommandAction::SetCompSegments { segments } => format!("set {} comp segments", segments.len()),
                CommandAction::RecordCommit { track_id, project_path } => format!("commit recording to track {track_id}{}", project_path.as_deref().map(|path| format!(": {path}")).unwrap_or_default()),
                CommandAction::SetTempo { bpm } => format!("set tempo: {bpm} BPM"),
                CommandAction::SetTimeSignature { beat, numerator, denominator } => {
                    format!("set time signature at beat {beat}: {numerator}/{denominator}")
                }
                CommandAction::SetAutomation { track_id, parameter_id, points } => {
                    format!("set automation track {track_id} parameter {parameter_id} ({} points)", points.len() / 3)
                }
                CommandAction::SetTrackDelayAutomation { track_id, points } => {
                    format!("set track {track_id} delay automation ({} points)", points.len() / 3)
                }
                CommandAction::CreateTrackStack { stack_id, name, member_track_ids, .. } => {
                    format!("create track stack {stack_id} {name} ({} members)", member_track_ids.len())
                }
                CommandAction::DeleteTrackStack { stack_id } => format!("delete track stack {stack_id}"),
                CommandAction::UpsertMarker { marker_id, label, beat, .. } => format!("set marker {marker_id} {label} at {beat:.2} beats"),
                CommandAction::DeleteMarker { marker_id } => format!("delete marker {marker_id}"),
                CommandAction::SetTrackStackGain { stack_id, master_gain } => {
                    format!("set track stack {stack_id} gain {master_gain}")
                }
                CommandAction::SetTrackStackCollapsed { stack_id, collapsed } => {
                    format!("{} track stack {stack_id}", if *collapsed { "collapse" } else { "expand" })
                }
                CommandAction::SetMasterGain { value } => format!("set master gain {value}"),
                CommandAction::SetMidiNote { track_id, pitch, velocity, start_sample, length_samples, .. } => {
                    format!("set MIDI note track {track_id} pitch {pitch} velocity {velocity} at {start_sample} for {length_samples} samples")
                }
                CommandAction::ClearMidiNotes => "clear all MIDI notes".into(),
                CommandAction::RemoveMidiNotesRange { track_id, start_sample, end_sample } => format!("remove MIDI notes on track {track_id} in {start_sample}..{end_sample}"),
                CommandAction::TransposeMidiNotesRange { track_id, start_sample, end_sample, semitones } => format!("transpose MIDI notes on track {track_id} in {start_sample}..{end_sample} by {semitones}"),
                CommandAction::MoveMidiNotesRange { track_id, start_sample, end_sample, delta_samples } => format!("move MIDI notes on track {track_id} in {start_sample}..{end_sample} by {delta_samples} samples"),
                CommandAction::SetRegionWarp { track_id, region_id, ratio } => {
                    format!("set region {region_id} on track {track_id} warp ratio: {ratio}")
                }
                CommandAction::SetRegionGain { track_id, region_id, gain_db } => {
                    format!("set region {region_id} on track {track_id} gain: {gain_db} dB")
                }
                CommandAction::SetRegionPitch { track_id, region_id, semitones } => {
                    format!("set region {region_id} on track {track_id} pitch: {semitones} semitones")
                }
                CommandAction::SetRegionAudioNoteSegment { track_id, region_id, start_seconds, end_seconds, pitch_offset_cents, .. } => {
                    format!("edit region {region_id} on track {track_id} pitch segment {start_seconds}..{end_seconds}: {pitch_offset_cents} cents")
                }
                CommandAction::ClearRegionAudioNoteSegments { track_id, region_id } => {
                    format!("clear pitch segments on region {region_id} on track {track_id}")
                }
                CommandAction::SetTrackName { track_id, name } => {
                    format!("rename track {track_id}: {name}")
                }
                CommandAction::Undo => "undo transaction".into(),
                CommandAction::Redo => "redo transaction".into(),
                CommandAction::ProjectInspect => "inspect project".into(),
                CommandAction::RenderTargetCatalog => "list renderable track, bus, and master targets".into(),
                CommandAction::ProjectLoad { path } => format!("load project: {path}"),
                CommandAction::SaveProject { path } => format!("save project: {path}"),
                CommandAction::BounceProject { path, format } => {
                    format!("bounce format {format}: {path}")
                }
                CommandAction::BounceStems { output_dir, format, track_ids, tail_seconds, pre_fader, include_inserts } => {
                    if track_ids.is_empty() {
                        format!("bounce all stems format {format} tail {tail_seconds}s pre_fader={pre_fader} inserts={include_inserts} into: {output_dir}")
                    } else {
                        format!("bounce {} selected stems format {format} tail {tail_seconds}s pre_fader={pre_fader} inserts={include_inserts} into: {output_dir}", track_ids.len())
                    }
                }
            };
            CommandDiff {
                index,
                summary,
                destructive: is_destructive(action),
            }
        })
        .collect()
}

pub fn mutation_class(action: &CommandAction) -> MutationClass {
    match action {
        CommandAction::ControlInspect
        | CommandAction::InspectMidiNotes
        | CommandAction::InspectChordTrack
        | CommandAction::GenerateChord { .. }
        | CommandAction::SuggestNextChords { .. }
        | CommandAction::GenerateArpeggio { .. }
        | CommandAction::PlaceArpeggio { .. }
        | CommandAction::DescribeDrumLane { .. }
        | CommandAction::AnalyzeDynamics { .. }
        | CommandAction::AnalyzeMix { .. }
        | CommandAction::AnalyzeSilence { .. }
        | CommandAction::PreviewVocalPitchCorrection { .. }
        | CommandAction::ProjectSearch { .. }
        | CommandAction::ExtensionCatalog { .. }
        | CommandAction::ExtensionValidate { .. }
        | CommandAction::ProjectInspect
        | CommandAction::RenderTargetCatalog
        | CommandAction::PluginCatalog
        | CommandAction::PluginSearch { .. }
        | CommandAction::TrackFreezeStatus { .. } => MutationClass::ReadOnly,
        CommandAction::OpenUtauNotes { .. } => MutationClass::ReadOnly,
        CommandAction::ExtensionInvoke { .. } => MutationClass::ExternalSideEffect,
        CommandAction::ExtensionSetEnabled { .. } => MutationClass::Reversible,
        CommandAction::OpenUtauImportMidi { .. } => MutationClass::Reversible,
        CommandAction::ProjectLoad { .. }
        | CommandAction::RemoveTrack { .. }
        | CommandAction::RemovePlugin { .. }
        | CommandAction::MovePlugin { .. }
        | CommandAction::ClearMidiNotes => MutationClass::Irreversible,
        CommandAction::RemoveMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::AddChordEvent { .. } => MutationClass::Reversible,
        CommandAction::PlaceGeneratedChord { .. } => MutationClass::Reversible,
        CommandAction::RemoveChordEventsRange { .. } | CommandAction::ClearChordTrack => {
            MutationClass::Reversible
        }
        CommandAction::TransposeMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::MoveMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::ApplyMidiLogicalRule { .. } => MutationClass::Reversible,
        CommandAction::TakeMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::CaptureMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::DiffMixSnapshots { .. } => MutationClass::ReadOnly,
        CommandAction::RecallMixSnapshot { .. } => MutationClass::ReadOnly,
        CommandAction::ApplyMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::SaveProject { .. }
        | CommandAction::BounceProject { .. }
        | CommandAction::BounceStems { .. } => MutationClass::ExternalSideEffect,
        CommandAction::RecordCommit { .. } => MutationClass::ExternalSideEffect,
        CommandAction::Undo
        | CommandAction::Redo
        | CommandAction::AddTrack { .. }
        | CommandAction::AddAuxTrack { .. }
        | CommandAction::AddVcaGroup { .. }
        | CommandAction::AssignTrackToVca { .. }
        | CommandAction::SetVcaGroupGain { .. }
        | CommandAction::SetPluginFavorite { .. }
        | CommandAction::AddPlugin { .. }
        | CommandAction::FreezeTrack { .. }
        | CommandAction::FreezeTrackToProjectEnd { .. }
        | CommandAction::UnfreezeTrack { .. }
        | CommandAction::DuplicateTrack { .. }
        | CommandAction::SetPluginParameter { .. }
        | CommandAction::SetPluginBypass { .. }
        | CommandAction::SetMacroValue { .. }
        | CommandAction::AddMacroMapping { .. }
        | CommandAction::RemoveMacroMapping { .. }
        | CommandAction::AddMidiLearnMapping { .. }
        | CommandAction::RemoveMidiLearnMapping { .. }
        | CommandAction::HumanizeMidi { .. }
        | CommandAction::ApplyMidiSwing { .. }
        | CommandAction::QuantizeMidi { .. }
        | CommandAction::InsertNamedPlugin { .. }
        | CommandAction::InsertPluginPath { .. }
        | CommandAction::OpenUtauImport { .. }
        | CommandAction::AddAudioRegion { .. }
        | CommandAction::ReplaceRegionAudio { .. }
        | CommandAction::SetVolume { .. }
        | CommandAction::ApplyGainStaging { .. }
        | CommandAction::SetEq { .. }
        | CommandAction::SetMasterGain { .. }
        | CommandAction::SetTrackDelay { .. }
        | CommandAction::SetLowLatencyMode { .. }
        | CommandAction::SetTonalScale { .. }
        | CommandAction::SetPan { .. }
        | CommandAction::SetMute { .. }
        | CommandAction::SetSolo { .. }
        | CommandAction::SetTrackArmed { .. }
        | CommandAction::SetPhaseInvert { .. }
        | CommandAction::SetRoute { .. }
        | CommandAction::SetRouteGain { .. }
        | CommandAction::SetFeedbackRoute { .. }
        | CommandAction::SetSidechainLink { .. }
        | CommandAction::MoveRegion { .. }
        | CommandAction::SplitRegion { .. }
        | CommandAction::SplitRegionWithCrossfade { .. }
        | CommandAction::SplitRegionAtSilence { .. }
        | CommandAction::DuplicateRegion { .. }
        | CommandAction::RemoveRegion { .. }
        | CommandAction::SetRegionFades { .. }
        | CommandAction::SetRegionTrim { .. }
        | CommandAction::SetRegionLoop { .. }
        | CommandAction::SetRegionReverse { .. }
        | CommandAction::SetRegionMuted { .. }
        | CommandAction::TransportPlay
        | CommandAction::TransportPause
        | CommandAction::TransportStop
        | CommandAction::SetPlayhead { .. }
        | CommandAction::SetLoop { .. }
        | CommandAction::SetMetronome { .. }
        | CommandAction::SetCycleRange { .. }
        | CommandAction::SetTempo { .. }
        | CommandAction::SetTimeSignature { .. }
        | CommandAction::SetAutomation { .. }
        | CommandAction::SetTrackDelayAutomation { .. }
        | CommandAction::CreateTrackStack { .. }
        | CommandAction::DeleteTrackStack { .. }
        | CommandAction::UpsertMarker { .. }
        | CommandAction::DeleteMarker { .. }
        | CommandAction::SetTrackStackGain { .. }
        | CommandAction::SetTrackStackCollapsed { .. }
        | CommandAction::SetMidiNote { .. }
        | CommandAction::SetRegionWarp { .. }
        | CommandAction::SetRegionGain { .. }
        | CommandAction::SetRegionPitch { .. }
        | CommandAction::SetRegionAudioNoteSegment { .. }
        | CommandAction::ClearRegionAudioNoteSegments { .. }
        | CommandAction::SetTrackName { .. }
        | CommandAction::ApplyDynamicsSuggestion { .. } => MutationClass::Reversible,
        CommandAction::RecordArm { .. }
        | CommandAction::RecordStart { .. }
        | CommandAction::RecordStop
        | CommandAction::SelectRecordingTake { .. }
        | CommandAction::RegisterCompTake { .. }
        | CommandAction::SelectCompTake { .. }
        | CommandAction::RemoveCompTake { .. }
        | CommandAction::SetCompSegments { .. } => MutationClass::Reversible,
    }
}

fn is_destructive(action: &CommandAction) -> bool {
    !matches!(
        mutation_class(action),
        MutationClass::ReadOnly | MutationClass::Reversible
    )
}

pub fn validate(document: CommandDocument) -> Result<ValidatedCommand, String> {
    let transaction = document.transaction.trim().to_owned();
    if document.schema_version != 1 || document.command_version != 1 {
        return Err("unsupported command schema or version".into());
    }
    if !valid_token(&transaction, 128) {
        return Err("transaction must be 1..=128 characters".into());
    }
    if document.actions.is_empty() || document.actions.len() > 256 {
        return Err("actions must contain 1..=256 entries".into());
    }
    for action in &document.actions {
        match action {
            CommandAction::AddTrack { name, .. }
            | CommandAction::AddAuxTrack { name }
            | CommandAction::SetTrackName { name, .. } => {
                if name.trim().is_empty() || name.len() > 256 {
                    return Err("track names must be 1..=256 characters".into());
                }
            }
            CommandAction::SetVolume { value, .. }
                if !value.is_finite() || !(-2.0..=2.0).contains(value) =>
            {
                return Err("volume must be finite and within -2..=2".into());
            }
            CommandAction::ApplyGainStaging { gain_db, .. }
                if !gain_db.is_finite() || !(-24.0..=24.0).contains(gain_db) =>
            {
                return Err("gain staging correction must be finite and within -24..=24 dB".into());
            }
            CommandAction::SetEq {
                low_band,
                low_cut,
                high_band,
                high_cut,
                ..
            } if [low_band, low_cut, high_band, high_cut]
                .iter()
                .any(|value| !value.is_finite()) =>
            {
                return Err("EQ parameters must be finite".into());
            }
            CommandAction::SetTrackDelay { samples, .. } if *samples > 8192 => {
                return Err("track delay must be within 0..=8192 samples".into());
            }
            CommandAction::SetTonalScale { root, scale_type }
                if !(-128..=127).contains(root) || *scale_type > 10 =>
            {
                return Err("tonal scale root/type is outside the supported range".into());
            }
            CommandAction::GenerateChord {
                root,
                octave,
                quality,
            } if !(-128..=127).contains(root) || !(-1..=10).contains(octave) || *quality > 5 => {
                return Err("chord root, octave, or quality is outside the supported range".into());
            }
            CommandAction::SuggestNextChords { last_chord_name }
                if last_chord_name.len() > 128 || last_chord_name.contains('\0') =>
            {
                return Err("chord name is invalid".into());
            }
            CommandAction::GenerateArpeggio {
                pitches,
                velocities,
                pattern,
                octaves,
                steps,
            } if pitches.is_empty()
                || pitches.len() > 128
                || velocities.len() != pitches.len()
                || pitches.iter().any(|pitch| *pitch > 127)
                || velocities.iter().any(|velocity| *velocity == 0)
                || *pattern > 3
                || *octaves == 0
                || *octaves > 4
                || *steps == 0
                || *steps > 4096 =>
            {
                return Err("arpeggio parameters are invalid".into());
            }
            CommandAction::PlaceArpeggio {
                track_id,
                step_samples,
                gate_samples,
                pitches,
                velocities,
                pattern,
                octaves,
                steps,
                ..
            } if *track_id == 0
                || *step_samples == 0
                || *gate_samples == 0
                || *gate_samples > *step_samples
                || pitches.is_empty()
                || pitches.len() > 128
                || velocities.len() != pitches.len()
                || pitches.iter().any(|pitch| *pitch > 127)
                || velocities.iter().any(|velocity| *velocity == 0)
                || *pattern > 3
                || *octaves == 0
                || *octaves > 4
                || *steps == 0
                || *steps > 4096 =>
            {
                return Err("arpeggio placement parameters are invalid".into());
            }
            CommandAction::AddChordEvent {
                root,
                intervals,
                name,
                ..
            } if *root > 127
                || intervals.len() > 32
                || intervals.iter().any(|interval| *interval > 127)
                || name.trim().is_empty()
                || name.chars().count() > 128
                || name.contains('\0') =>
            {
                return Err("chord event fields are invalid".into());
            }
            CommandAction::PlaceGeneratedChord {
                track_id,
                length_samples,
                velocity,
                root,
                octave,
                quality,
                ..
            } if *track_id == 0
                || *length_samples == 0
                || *velocity == 0
                || !(-128..=127).contains(root)
                || !(-1..=10).contains(octave)
                || *quality > 5 =>
            {
                return Err("generated chord placement fields are invalid".into());
            }
            CommandAction::RemoveChordEventsRange {
                start_tick,
                end_tick,
            } if start_tick > end_tick => {
                return Err("chord event range is reversed".into());
            }
            CommandAction::SetPan { value, .. }
                if !value.is_finite() || !(-1.0..=1.0).contains(value) =>
            {
                return Err("pan must be finite and within -1..=1".into());
            }
            CommandAction::SetPluginParameter { value, .. } if !value.is_finite() => {
                return Err("plugin parameter must be finite".into());
            }
            CommandAction::AddVcaGroup { group_id, gain }
            | CommandAction::SetVcaGroupGain { group_id, gain }
                if *group_id == 0 || !gain.is_finite() || !(0.0..=8.0).contains(gain) =>
            {
                return Err(
                    "VCA group id must be nonzero and gain must be finite within 0..=8".into(),
                );
            }
            CommandAction::SetPluginFavorite { id, .. } if !valid_token(id, 256) => {
                return Err("plugin favorite id must be 1..=256 token characters".into());
            }
            CommandAction::AssignTrackToVca { track_id, group_id }
                if *track_id == 0 || *group_id == 0 =>
            {
                return Err("VCA track and group ids must be nonzero".into());
            }
            CommandAction::SetMacroValue { macro_index, value }
                if *macro_index >= 128 || !value.is_finite() || !(0.0..=1.0).contains(value) =>
            {
                return Err("macro index must be below 128 and value within 0..=1".into());
            }
            CommandAction::AddMacroMapping {
                mapping_id,
                macro_index,
                target_instance_id,
                target_parameter_id,
                min,
                max,
                curve,
                ..
            } => {
                if !valid_token(mapping_id, 128)
                    || *macro_index >= 128
                    || !valid_token(target_instance_id, 256)
                    || target_parameter_id.trim().is_empty()
                    || target_parameter_id.len() > 128
                    || !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || !curve.is_finite()
                    || !(-1.0..=1.0).contains(curve)
                {
                    return Err("invalid macro mapping".into());
                }
            }
            CommandAction::RemoveMacroMapping { mapping_id } if !valid_token(mapping_id, 128) => {
                return Err("invalid macro mapping id".into());
            }
            CommandAction::AddMidiLearnMapping {
                mapping_id,
                device_id,
                channel,
                controller,
                target_instance_id,
                target_parameter_id,
                min,
                max,
                curve,
                ..
            } => {
                if !valid_token(mapping_id, 128)
                    || device_id.trim().is_empty()
                    || device_id.len() > 256
                    || *channel > 15
                    || *controller > 16_383
                    || !valid_token(target_instance_id, 256)
                    || target_parameter_id.trim().is_empty()
                    || target_parameter_id.len() > 128
                    || !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || !curve.is_finite()
                    || !(-1.0..=1.0).contains(curve)
                {
                    return Err("invalid MIDI learn mapping".into());
                }
            }
            CommandAction::RemoveMidiLearnMapping { mapping_id }
                if !valid_token(mapping_id, 128) =>
            {
                return Err("invalid MIDI mapping id".into());
            }
            CommandAction::HumanizeMidi {
                timing_beats,
                velocity,
                ..
            } if !timing_beats.is_finite()
                || !(-4.0..=4.0).contains(timing_beats)
                || !(-127..=127).contains(velocity) =>
            {
                return Err("MIDI humanize parameters are out of range".into());
            }
            CommandAction::ApplyMidiSwing {
                subdivision_beats,
                amount,
            } if !subdivision_beats.is_finite()
                || !(0.001..=16.0).contains(subdivision_beats)
                || !amount.is_finite()
                || !(-1.0..=1.0).contains(amount) =>
            {
                return Err("MIDI swing parameters are out of range".into());
            }
            CommandAction::QuantizeMidi {
                grid_beats,
                strength,
            } if !grid_beats.is_finite()
                || !(0.001..=16.0).contains(grid_beats)
                || !strength.is_finite()
                || !(0.0..=1.0).contains(strength) =>
            {
                return Err("MIDI quantize parameters are out of range".into());
            }
            CommandAction::SetAutomation { points, .. } => {
                if points.len() % 3 != 0 || points.len() > 24576 {
                    return Err(
                        "automation points must contain 0..=8192 time/value/curve triples".into(),
                    );
                }
                let mut previous_time = f64::NEG_INFINITY;
                for triple in points.chunks_exact(3) {
                    let time = triple[0];
                    let value = triple[1];
                    let curve = triple[2];
                    if !time.is_finite() || !value.is_finite() || !curve.is_finite() {
                        return Err("automation points must be finite".into());
                    }
                    if time < 0.0 || time.fract() != 0.0 {
                        return Err(
                            "automation time must be a non-negative integer sample position".into(),
                        );
                    }
                    if !(0.0..=1.0).contains(&value) {
                        return Err("automation value must be normalized within 0..=1".into());
                    }
                    if !(-1.0..=1.0).contains(&curve) {
                        return Err("automation curve must be normalized within -1..=1".into());
                    }
                    if time <= previous_time {
                        return Err("automation times must be strictly increasing".into());
                    }
                    previous_time = time;
                }
            }
            CommandAction::SetTrackDelayAutomation { points, .. } => {
                if points.len() % 3 != 0 || points.len() > 24576 {
                    return Err("track delay automation must contain 0..=8192 triples".into());
                }
                let mut previous_time = f64::NEG_INFINITY;
                for triple in points.chunks_exact(3) {
                    let time = triple[0];
                    if !time.is_finite()
                        || time < 0.0
                        || time.fract() != 0.0
                        || time <= previous_time
                        || !triple[1].is_finite()
                        || !(0.0..=1.0).contains(&triple[1])
                        || !triple[2].is_finite()
                        || !(-1.0..=1.0).contains(&triple[2])
                    {
                        return Err("track delay automation requires increasing sample times and normalized values".into());
                    }
                    previous_time = time;
                }
            }
            CommandAction::CreateTrackStack {
                stack_id,
                name,
                member_track_ids,
                master_gain,
                ..
            } => {
                let mut unique_members =
                    std::collections::HashSet::with_capacity(member_track_ids.len());
                if *stack_id == 0
                    || name.trim().is_empty()
                    || name.len() > 256
                    || member_track_ids.is_empty()
                    || member_track_ids.iter().any(|id| *id == 0)
                    || member_track_ids
                        .iter()
                        .any(|id| !unique_members.insert(*id))
                    || !master_gain.is_finite()
                    || !(0.0..=2.0).contains(master_gain)
                {
                    return Err("invalid track stack definition".into());
                }
            }
            CommandAction::DeleteTrackStack { stack_id } => {
                if *stack_id == 0 {
                    return Err("track stack id must be nonzero".into());
                }
            }
            CommandAction::UpsertMarker {
                marker_id,
                label,
                beat,
                color,
            } => {
                if *marker_id == 0
                    || label.trim().is_empty()
                    || label.len() > 128
                    || label.contains('\0')
                    || !beat.is_finite()
                    || *beat < 0.0
                    || color.len() > 32
                    || color.contains('\0')
                {
                    return Err("invalid arrangement marker".into());
                }
            }
            CommandAction::DeleteMarker { marker_id } => {
                if *marker_id == 0 {
                    return Err("marker id must be nonzero".into());
                }
            }
            CommandAction::SetTrackStackGain { master_gain, .. } => {
                if !master_gain.is_finite() || !(0.0..=2.0).contains(master_gain) {
                    return Err("track stack gain is outside 0..=2".into());
                }
            }
            CommandAction::SetMasterGain { value } => {
                if !value.is_finite() || !(0.0..=2.0).contains(value) {
                    return Err("master gain is outside 0..=2".into());
                }
            }
            CommandAction::RegisterCompTake {
                take_id,
                name,
                start_sample,
                end_sample,
            } => {
                if *take_id == 0
                    || name.trim().is_empty()
                    || name.len() > 128
                    || name.contains('\0')
                    || *end_sample <= *start_sample
                {
                    return Err("invalid comp take metadata".into());
                }
            }
            CommandAction::SelectCompTake { take_id } => {
                if *take_id == 0 {
                    return Err("comp take id must be nonzero".into());
                }
            }
            CommandAction::RemoveCompTake { take_id } => {
                if *take_id == 0 {
                    return Err("comp take id must be nonzero".into());
                }
            }
            CommandAction::SetCompSegments { segments } => {
                if segments.len() > 262_144 {
                    return Err("too many comp segments".into());
                }
                let mut sorted = segments.iter().collect::<Vec<_>>();
                sorted.sort_by_key(|segment| segment.start_sample);
                for segment in &sorted {
                    if segment.take_id == 0
                        || segment.length_samples == 0
                        || segment.crossfade_samples as u64 > segment.length_samples
                        || segment
                            .start_sample
                            .checked_add(segment.length_samples)
                            .is_none()
                    {
                        return Err("invalid comp segment bounds".into());
                    }
                }
                if sorted.windows(2).any(|pair| {
                    pair[0]
                        .start_sample
                        .checked_add(pair[0].length_samples)
                        .is_none_or(|end| end > pair[1].start_sample)
                }) {
                    return Err("comp segments overlap".into());
                }
            }
            CommandAction::SetMidiNote {
                pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme,
                pitch_curve_cents,
                ..
            } => {
                if *pitch > 127 || *velocity == 0 || *length_samples == 0 {
                    return Err(
                        "MIDI note pitch/velocity must be 0..=127 and length must be positive"
                            .into(),
                    );
                }
                if start_sample.checked_add(*length_samples).is_none() {
                    return Err("MIDI note range overflows the sample timeline".into());
                }
                if lyric.len() > 1_024 || lyric.contains('\0') {
                    return Err(
                        "MIDI note lyric must be at most 1024 bytes and contain no NUL".into(),
                    );
                }
                if phoneme.len() > 128 || phoneme.contains('\0') {
                    return Err(
                        "MIDI note phoneme must be at most 128 bytes and contain no NUL".into(),
                    );
                }
                if pitch_curve_cents.len() > 256 {
                    return Err("MIDI note pitch curve must contain at most 256 points".into());
                }
            }
            CommandAction::RemoveMidiNotesRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("MIDI note range must be ordered and non-empty".into());
            }
            CommandAction::TransposeMidiNotesRange {
                start_sample,
                end_sample,
                semitones,
                ..
            } if start_sample >= end_sample || !(-127..=127).contains(semitones) => {
                return Err("MIDI transpose range or semitones are invalid".into());
            }
            CommandAction::MoveMidiNotesRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("MIDI move range must be ordered and non-empty".into());
            }
            CommandAction::SetRegionWarp { ratio, .. }
                if !ratio.is_finite() || !(0.25..=4.0).contains(ratio) =>
            {
                return Err("region warp ratio must be finite and within 0.25..=4.0".into());
            }
            CommandAction::SetRegionGain { gain_db, .. }
                if !gain_db.is_finite() || !(-24.0..=24.0).contains(gain_db) =>
            {
                return Err("region gain must be finite and within -24..=24 dB".into());
            }
            CommandAction::SetRegionPitch { semitones, .. }
                if !semitones.is_finite() || !(-48.0..=48.0).contains(semitones) =>
            {
                return Err("region pitch must be finite and within -48..=48 semitones".into());
            }
            CommandAction::SetRegionAudioNoteSegment {
                start_seconds,
                end_seconds,
                pitch_offset_cents,
                formant_offset_cents,
                ..
            } if !start_seconds.is_finite()
                || !end_seconds.is_finite()
                || *end_seconds <= *start_seconds
                || *end_seconds - *start_seconds > 24.0 * 60.0
                || !pitch_offset_cents.is_finite()
                || pitch_offset_cents.abs() > 4800.0
                || !formant_offset_cents.is_finite()
                || formant_offset_cents.abs() > 2400.0 =>
            {
                return Err("audio note segment timing or offsets are invalid".into());
            }
            CommandAction::SetTempo { bpm }
                if !bpm.is_finite() || !(20.0..=400.0).contains(bpm) =>
            {
                return Err("tempo must be finite and within 20..=400 BPM".into());
            }
            CommandAction::SetTimeSignature {
                beat,
                numerator,
                denominator,
            } if !beat.is_finite()
                || *beat < 0.0
                || *numerator == 0
                || *numerator > 32
                || !matches!(*denominator, 1 | 2 | 4 | 8 | 16 | 32) =>
            {
                return Err("time signature must be a valid beat and meter".into());
            }
            CommandAction::RecordArm {
                sample_rate,
                channels,
                max_frames,
            } => {
                if !sample_rate.is_finite()
                    || !(8_000.0..=384_000.0).contains(sample_rate)
                    || *channels == 0
                    || *channels > 64
                    || *max_frames == 0
                    || *max_frames > 16_777_216
                {
                    return Err("recording format is outside supported limits".into());
                }
            }
            CommandAction::RecordStart {
                sample_rate,
                channels,
                max_frames,
                count_in_frames,
                ..
            } => {
                if !sample_rate.is_finite()
                    || !(8_000.0..=384_000.0).contains(sample_rate)
                    || *channels == 0
                    || *channels > 64
                    || *max_frames == 0
                    || *max_frames > 16_777_216
                    || *count_in_frames > 16_777_216
                {
                    return Err("recording format is outside supported limits".into());
                }
            }
            CommandAction::SetCycleRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("cycle range must be ordered and non-empty".into());
            }
            CommandAction::SplitRegion { beat, .. } if !beat.is_finite() || *beat <= 0.0 => {
                return Err("region split beat must be finite and positive".into());
            }
            CommandAction::SplitRegionWithCrossfade { beat, ratio, .. }
                if !beat.is_finite()
                    || *beat <= 0.0
                    || !ratio.is_finite()
                    || !(0.0..=1.0).contains(ratio) =>
            {
                return Err(
                    "crossfade split requires a positive beat and ratio within 0..=1".into(),
                );
            }
            CommandAction::DuplicateRegion { start, .. } if !start.is_finite() || *start < 0.0 => {
                return Err("region duplicate start must be finite and non-negative".into());
            }
            CommandAction::MoveRegion { start, .. } if !start.is_finite() || *start < 0.0 => {
                return Err("region start must be finite and non-negative".into());
            }
            CommandAction::RemoveRegion {
                track_id,
                region_id,
            } if *track_id == 0 || *region_id == 0 => {
                return Err("region removal requires non-zero track and region ids".into());
            }
            CommandAction::SetRegionFades {
                fade_in, fade_out, ..
            } if !fade_in.is_finite()
                || !fade_out.is_finite()
                || *fade_in < 0.0
                || *fade_out < 0.0
                || *fade_in > 1.0
                || *fade_out > 1.0 =>
            {
                return Err("region fades must be finite and within 0..=1".into());
            }
            CommandAction::SetRegionTrim { start, end, .. }
                if !start.is_finite()
                    || !end.is_finite()
                    || *start < 0.0
                    || *end > 1.0
                    || *start >= *end =>
            {
                return Err("region trim must be finite, ordered, and within 0..=1".into());
            }
            CommandAction::SetRegionLoop { count, .. } if !(1..=1024).contains(count) => {
                return Err("region loop count must be within 1..=1024".into());
            }
            CommandAction::InsertNamedPlugin { alias, .. }
                if alias.trim().is_empty() || alias.len() > 256 =>
            {
                return Err("plugin alias must be 1..=256 characters".into());
            }
            CommandAction::InsertPluginPath { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("plugin path must be 1..=4096 characters".into());
            }
            CommandAction::FreezeTrack {
                track_id,
                total_samples,
                ..
            } if *track_id == 0 || *total_samples == 0 || *total_samples > 64 * 1024 * 1024 => {
                return Err("freeze track id or sample range is invalid".into());
            }
            CommandAction::FreezeTrackToProjectEnd { track_id } if *track_id == 0 => {
                return Err("freeze track id is invalid".into());
            }
            CommandAction::SetFeedbackRoute {
                source_id,
                dest_id,
                gain,
                ..
            } if *source_id == *dest_id || !gain.is_finite() || !(0.0..=2.0).contains(gain) => {
                return Err("feedback route requires distinct nodes and gain within 0..=2".into());
            }
            CommandAction::SetRouteGain {
                source_id,
                dest_id,
                gain,
                enabled,
            } if *source_id == *dest_id
                || !gain.is_finite()
                || !(0.0..=2.0).contains(gain)
                || (*enabled && *gain <= 0.0) =>
            {
                return Err("route gain requires distinct nodes, a finite gain within 0..=2, and positive gain when enabled".into());
            }
            CommandAction::FreezeTrack {
                path: Some(path), ..
            } if path.trim().is_empty() || path.len() > 4096 => {
                return Err("freeze cache path must be 1..=4096 characters".into());
            }
            CommandAction::OpenUtauImport {
                source_path,
                rendered_audio_path,
                ..
            } if source_path.trim().is_empty()
                || rendered_audio_path.trim().is_empty()
                || source_path.len() > 4096
                || rendered_audio_path.len() > 4096 =>
            {
                return Err("OpenUtau paths must be 1..=4096 characters".into());
            }
            CommandAction::OpenUtauNotes { source_path }
                if source_path.trim().is_empty()
                    || source_path.len() > 4096
                    || source_path.contains('\0') =>
            {
                return Err("OpenUtau source path must be 1..=4096 characters without NUL".into());
            }
            CommandAction::OpenUtauImportMidi {
                track_id,
                source_path,
                sample_rate,
                ticks_per_beat,
            } if *track_id == 0
                || source_path.trim().is_empty()
                || source_path.len() > 4096
                || source_path.contains('\0')
                || *sample_rate == 0
                || *sample_rate > 384_000
                || *ticks_per_beat == 0
                || *ticks_per_beat > 32_768 =>
            {
                return Err(
                    "OpenUtau MIDI import has invalid track, path, sample rate, or PPQ".into(),
                );
            }
            CommandAction::AddAudioRegion { path, start, .. }
                if path.trim().is_empty()
                    || path.len() > 4096
                    || !start.is_finite()
                    || *start < 0.0 =>
            {
                return Err("audio region path/start is invalid".into());
            }
            CommandAction::ReplaceRegionAudio { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("replacement audio path is invalid".into());
            }
            CommandAction::ProjectLoad { path }
            | CommandAction::SaveProject { path }
            | CommandAction::BounceProject { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("file paths must be 1..=4096 characters".into());
            }
            CommandAction::BounceStems { output_dir, .. }
                if output_dir.trim().is_empty() || output_dir.len() > 4096 =>
            {
                return Err("stem output directory must be 1..=4096 characters".into());
            }
            CommandAction::BounceStems {
                track_ids,
                tail_seconds,
                ..
            } => {
                if track_ids.len() > 4096 || track_ids.iter().any(|id| *id == 0) {
                    return Err("stem track selection is invalid".into());
                }
                let mut unique = std::collections::HashSet::with_capacity(track_ids.len());
                if track_ids.iter().any(|id| !unique.insert(id)) {
                    return Err("stem track selection contains duplicates".into());
                }
                if !tail_seconds.is_finite() || !(0.0..=60.0).contains(tail_seconds) {
                    return Err("stem tail_seconds must be finite and within 0..=60".into());
                }
            }
            CommandAction::RecordCommit {
                project_path: Some(path),
                ..
            } if path.trim().is_empty() || path.len() > 4096 => {
                return Err("recording project path must be 1..=4096 characters".into());
            }
            CommandAction::ExtensionCatalog { root }
                if root.trim().is_empty() || root.len() > 4096 =>
            {
                return Err("extension root must be 1..=4096 characters".into());
            }
            CommandAction::ProjectSearch { query }
                if query.trim().is_empty() || query.len() > 256 || query.contains('\0') =>
            {
                return Err("project search query must be 1..=256 characters without NUL".into());
            }
            CommandAction::AnalyzeDynamics { samples, .. }
                if samples.len() > 262_144 || samples.iter().any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "dynamics analysis samples must be finite and contain at most 262144 values"
                        .into(),
                );
            }
            CommandAction::AnalyzeMix { left, right, .. }
                if left.len() > 262_144
                    || right.len() > 262_144
                    || left.iter().chain(right).any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "mix analysis requires finite channels with at most 262144 samples each".into(),
                );
            }
            CommandAction::AnalyzeSilence {
                samples,
                threshold,
                min_length,
            } if samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !threshold.is_finite()
                || !(0.0..=1.0).contains(threshold)
                || *min_length == 0
                || *min_length as usize > 262_144 =>
            {
                return Err("silence analysis requires finite samples, threshold 0..=1, and a valid minimum length".into());
            }
            CommandAction::SplitRegionAtSilence {
                track_id,
                region_id,
                samples,
                threshold,
                min_length,
            } if *track_id == 0
                || *region_id == 0
                || samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !threshold.is_finite()
                || !(0.0..=1.0).contains(threshold)
                || *min_length == 0
                || *min_length > 262_144 =>
            {
                return Err("silence split parameters are invalid or exceed bounds".into());
            }
            CommandAction::PreviewVocalPitchCorrection {
                samples,
                sample_rate,
                speed,
                timing_ratio,
            } if samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !sample_rate.is_finite()
                || !(8_000.0..=384_000.0).contains(sample_rate)
                || !speed.is_finite()
                || !(0.0..=1.0).contains(speed)
                || !timing_ratio.is_finite()
                || !(0.25..=4.0).contains(timing_ratio) =>
            {
                return Err("vocal preview requires finite samples, sample rate 8000..384000, and speed 0..=1".into());
            }
            CommandAction::ApplyDynamicsSuggestion {
                track_id, samples, ..
            } if *track_id == 0
                || samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "dynamics target is invalid or samples exceed 262144 finite values".into(),
                );
            }
            CommandAction::ExtensionValidate {
                root,
                extension_id,
                command_id,
                ..
            } if root.trim().is_empty()
                || root.len() > 4096
                || !valid_token(extension_id, 128)
                || !valid_token(command_id, 128) =>
            {
                return Err("extension validation request is invalid".into());
            }
            CommandAction::ExtensionInvoke {
                root,
                extension_id,
                command_id,
                timeout_ms,
                ..
            } if root.trim().is_empty()
                || root.len() > 4096
                || root.contains('\0')
                || !valid_token(extension_id, 128)
                || !valid_token(command_id, 128)
                || !(1..=30_000).contains(timeout_ms) =>
            {
                return Err("extension invocation request is invalid".into());
            }
            CommandAction::ExtensionSetEnabled {
                root, extension_id, ..
            } => {
                if root.trim().is_empty()
                    || root.len() > 4096
                    || root.contains('\0')
                    || !valid_token(extension_id, 128)
                {
                    return Err("extension activation request is invalid".into());
                }
            }
            CommandAction::TakeMixSnapshot { name, states } => {
                if name.trim().is_empty()
                    || name.len() > 128
                    || states.len() > 65_536
                    || states.values().any(|value| !value.is_finite())
                {
                    return Err("mix snapshot name or state is invalid".into());
                }
            }
            CommandAction::DiffMixSnapshots { .. }
            | CommandAction::ApplyMidiLogicalRule { .. }
            | CommandAction::CaptureMixSnapshot { .. }
            | CommandAction::RecallMixSnapshot { .. }
            | CommandAction::ApplyMixSnapshot { .. } => {}
            _ => {}
        }
    }
    if document.permission == Permission::ReadOnly
        && document.actions.iter().any(|action| {
            !matches!(
                action,
                CommandAction::ControlInspect
                    | CommandAction::InspectMidiNotes
                    | CommandAction::InspectChordTrack
                    | CommandAction::AnalyzeDynamics { .. }
                    | CommandAction::AnalyzeMix { .. }
                    | CommandAction::AnalyzeSilence { .. }
                    | CommandAction::PreviewVocalPitchCorrection { .. }
                    | CommandAction::ProjectSearch { .. }
                    | CommandAction::ExtensionCatalog { .. }
                    | CommandAction::ExtensionValidate { .. }
                    | CommandAction::ProjectInspect
                    | CommandAction::RenderTargetCatalog
                    | CommandAction::PluginCatalog
                    | CommandAction::TrackFreezeStatus { .. }
                    | CommandAction::DiffMixSnapshots { .. }
                    | CommandAction::RecallMixSnapshot { .. }
            )
        })
    {
        return Err("read_only permission allows project inspection only".into());
    }
    let mutation_class = document
        .actions
        .iter()
        .map(mutation_class)
        .max_by_key(|class| match class {
            MutationClass::ReadOnly => 0,
            MutationClass::Reversible => 1,
            MutationClass::Irreversible => 2,
            MutationClass::ExternalSideEffect => 3,
        })
        .unwrap_or(MutationClass::ReadOnly);
    let destructive = document.actions.iter().any(is_destructive);
    if mutation_class == MutationClass::ExternalSideEffect
        && !matches!(
            document.permission,
            Permission::SystemWrite | Permission::Unrestricted
        )
    {
        return Err("external side effects require system_write permission".into());
    }
    Ok(ValidatedCommand {
        schema_version: document.schema_version,
        command_version: document.command_version,
        transaction,
        permission: document.permission,
        expected_generation: document.expected_generation,
        expected_audio_generation: document.expected_audio_generation,
        actions: document.actions,
        destructive,
        mutation_class,
    })
}

/// Resolve a command-provided path without allowing traversal or symlink
/// escape from the project root. Nonexistent output files are checked using
/// their canonical parent, so this is safe for save and bounce destinations.
pub fn validate_project_path(project_root: &Path, requested: &str) -> Result<PathBuf, BridgeError> {
    validate_command_path(project_root, requested, PathPolicy::ProjectOnly)
}

/// Resolve a command path according to an explicit user-selected policy.
/// `ProjectOnly` is the default safe policy. `Unrestricted` is intentionally
/// opt-in for trusted local automation and preserves canonicalization and
/// basic path validation while allowing external paths and symlinks.
pub fn validate_command_path(
    project_root: &Path,
    requested: &str,
    policy: PathPolicy,
) -> Result<PathBuf, BridgeError> {
    if requested.trim().is_empty() || requested.len() > 4096 {
        return Err(BridgeError::new(
            "invalid_path",
            "path must be 1..=4096 characters",
        ));
    }
    let root = project_root
        .canonicalize()
        .map_err(|error| BridgeError::new("project_root_unavailable", error.to_string()))?;
    let candidate = Path::new(requested);
    if policy == PathPolicy::Unrestricted {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            project_root.join(candidate)
        };
        return if joined.exists() {
            joined
                .canonicalize()
                .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))
        } else {
            let parent = joined.parent().unwrap_or_else(|| Path::new("."));
            let parent = parent
                .canonicalize()
                .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))?;
            Ok(parent.join(joined.file_name().unwrap_or_default()))
        };
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BridgeError::new(
            "path_traversal",
            "parent-directory components are not allowed",
        ));
    }
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let mut symlink_cursor = if candidate.is_absolute() {
        PathBuf::new()
    } else {
        root.clone()
    };
    for component in candidate.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        symlink_cursor.push(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&symlink_cursor) {
            if metadata.file_type().is_symlink() {
                return Err(BridgeError::new(
                    "symlink_path",
                    "symlink components are not allowed for command output paths",
                ));
            }
        }
    }
    let check_path = if joined.exists() {
        joined.canonicalize()
    } else {
        joined
            .parent()
            .unwrap_or(&root)
            .canonicalize()
            .map(|parent| parent.join(joined.file_name().unwrap_or_default()))
    }
    .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))?;
    if !check_path.starts_with(&root) {
        return Err(BridgeError::new(
            "path_outside_project",
            "path must remain inside the project root",
        ));
    }
    let relative = check_path.strip_prefix(&root).unwrap_or(Path::new(""));
    let mut cursor = root.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        cursor.push(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                return Err(BridgeError::new(
                    "symlink_path",
                    "symlink components are not allowed for command output paths",
                ));
            }
        }
    }
    Ok(check_path)
}

pub fn validate_project_output_path(
    project_root: &Path,
    requested: &str,
    allow_overwrite: bool,
) -> Result<PathBuf, BridgeError> {
    let path = validate_project_path(project_root, requested)?;
    if path.exists() && !allow_overwrite {
        return Err(BridgeError::new(
            "overwrite_confirmation_required",
            "existing output requires explicit overwrite confirmation",
        ));
    }
    Ok(path)
}

/// Validate every filesystem path carried by a command before any native
/// action is started. Keeping this at the command boundary prevents one
/// action in a multi-action transaction from bypassing the same root policy
/// used by the other actions.
pub fn validate_command_action_paths(
    actions: &[CommandAction],
    permission: Permission,
    project_root: &Path,
) -> Result<(), BridgeError> {
    let policy = if permission == Permission::Unrestricted {
        PathPolicy::Unrestricted
    } else {
        PathPolicy::ProjectOnly
    };
    for action in actions {
        if let CommandAction::InsertPluginPath { path, .. } = action {
            if permission != Permission::Unrestricted
                && !crate::plugin_catalog::is_admitted_path(path)
            {
                return Err(BridgeError::new(
                    "plugin_path_not_admitted",
                    "plugin path must refer to a discovered installed plugin",
                )
                .object(path));
            }
        }
        let paths: Vec<&str> = match action {
            CommandAction::ProjectLoad { path }
            | CommandAction::SaveProject { path }
            | CommandAction::BounceProject { path, .. } => vec![path],
            CommandAction::BounceStems { output_dir, .. } => vec![output_dir],
            CommandAction::RecordCommit {
                project_path: Some(path),
                ..
            } => vec![path],
            CommandAction::ExtensionCatalog { root } => vec![root],
            CommandAction::ExtensionValidate { root, .. } => vec![root],
            CommandAction::ExtensionInvoke { root, .. } => vec![root],
            CommandAction::ExtensionSetEnabled { root, .. } => vec![root],
            CommandAction::OpenUtauImport {
                source_path,
                rendered_audio_path,
                ..
            } => vec![source_path, rendered_audio_path],
            CommandAction::OpenUtauNotes { source_path } => vec![source_path],
            CommandAction::OpenUtauImportMidi { source_path, .. } => vec![source_path],
            CommandAction::FreezeTrack {
                path: Some(path), ..
            } => vec![path],
            CommandAction::AddAudioRegion { path, .. }
            | CommandAction::ReplaceRegionAudio { path, .. } => vec![path],
            _ => Vec::new(),
        };
        for requested in paths {
            validate_command_path(project_root, requested, policy)?;
        }
    }
    Ok(())
}

/// Persistent request ledger for at-least-once transports.
///
/// A request is durably marked `prepared` before native mutation starts. This
/// is deliberately fail-closed: if the process dies after mutating the
/// project but before publishing the result, a retry sees an in-flight entry
/// and is refused instead of applying the mutation twice. Recovery tooling can
/// inspect that entry and decide whether to reconcile the project snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerState {
    Prepared,
    Applying,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub transaction_id: Option<String>,
    pub state: LedgerState,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub prepared_unix_seconds: u64,
}

/// Read-only audit projection used by CLI/AI clients. It intentionally omits
/// internal ledger details while retaining enough information to inspect a
/// request, identify its transaction, and replay a committed outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub request_id: String,
    pub transaction_id: Option<String>,
    pub state: LedgerState,
    pub prepared_unix_seconds: u64,
    pub replayable: bool,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LedgerBegin {
    Started,
    Replayed(serde_json::Value),
}

/// Outcome of an idempotent mutation submission.  A replay is successful and
/// returns the original durable result; callers should not execute the native
/// side effect again.
#[derive(Debug, Clone, PartialEq)]
pub enum LedgerOutcome {
    Applied(serde_json::Value),
    Replayed(serde_json::Value),
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RequestLedger {
    entries: std::collections::BTreeMap<String, LedgerEntry>,
}

struct LedgerLock {
    path: PathBuf,
    nonce: String,
}

impl LedgerLock {
    fn acquire(ledger_path: &Path) -> Result<Self, BridgeError> {
        let path = ledger_path.with_extension("lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .or_else(|error| {
                if reclaim_dead_ledger_lock(&path) {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                } else {
                    Err(error)
                }
            })
            .map_err(|error| BridgeError::new("ledger_busy", error.to_string()))?;
        let nonce = Uuid::new_v4().to_string();
        let owner = format!("pid={} nonce={nonce}\n", std::process::id());
        if let Err(error) = file
            .write_all(owner.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(BridgeError::new("ledger_lock_failed", error.to_string()));
        }
        Ok(Self { path, nonce })
    }
}

fn reclaim_dead_ledger_lock(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<i32>().ok())
    else {
        return false;
    };
    #[cfg(unix)]
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true);
    #[cfg(not(unix))]
    let alive = true;
    !alive && std::fs::remove_file(path).is_ok()
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        let owns_lock = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("nonce=").map(str::to_owned))
            })
            .is_some_and(|nonce| nonce == self.nonce);
        if owns_lock {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Holds the project-wide apply lock for the entire native transaction. The
/// ledger lock is intentionally short-lived so it can protect individual
/// durable updates; it must not be reused as the mutation lock because it is
/// released between `begin` and `complete`.
pub struct CommandTransactionLock {
    path: PathBuf,
    nonce: String,
}

impl CommandTransactionLock {
    pub fn acquire(ledger_path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let path = ledger_path.as_ref().with_extension("transaction.lock");
        // The transaction lock is acquired before RequestLedger::with_locked
        // gets a chance to create the ledger directory.  New projects
        // therefore need this directory creation here as well; otherwise the
        // first CLI mutation fails with a misleading transaction_busy/ENOENT.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .or_else(|error| {
                if reclaim_dead_ledger_lock(&path) {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                } else {
                    Err(error)
                }
            })
            .map_err(|error| BridgeError::new("transaction_busy", error.to_string()))?;
        let nonce = Uuid::new_v4().to_string();
        let owner = format!("pid={} nonce={nonce}\n", std::process::id());
        if let Err(error) = file
            .write_all(owner.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(BridgeError::new(
                "transaction_lock_failed",
                error.to_string(),
            ));
        }
        Ok(Self { path, nonce })
    }
}

impl Drop for CommandTransactionLock {
    fn drop(&mut self) {
        let owns_lock = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("nonce=").map(str::to_owned))
            })
            .is_some_and(|nonce| nonce == self.nonce);
        if owns_lock {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl RequestLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = read(path)
            .map_err(|error| BridgeError::new("ledger_read_failed", error.to_string()))?;
        if let Ok(ledger) = serde_json::from_slice(&bytes) {
            return Ok(ledger);
        }
        // Migrate the original result-only ledger format. Existing successful
        // entries are safe to replay and are treated as committed records.
        let legacy: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&bytes)
                .map_err(|error| BridgeError::new("ledger_corrupt", error.to_string()))?;
        Ok(Self {
            entries: legacy
                .into_iter()
                .map(|(key, result)| {
                    (
                        key,
                        LedgerEntry {
                            transaction_id: None,
                            state: LedgerState::Committed,
                            result: Some(result),
                            prepared_unix_seconds: 0,
                        },
                    )
                })
                .collect(),
        })
    }

    pub fn replay(&self, request_id: &str) -> Option<serde_json::Value> {
        self.entries
            .get(request_id)
            .and_then(|entry| match entry.state {
                LedgerState::Committed | LedgerState::Failed => entry.result.clone(),
                LedgerState::Prepared | LedgerState::Applying => None,
            })
    }

    /// Return a deterministic, read-only audit log for external automation.
    /// Results are included because a lost response must be recoverable
    /// without reapplying the mutation.
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.entries
            .iter()
            .map(|(request_id, entry)| AuditEntry {
                request_id: request_id.clone(),
                transaction_id: entry.transaction_id.clone(),
                state: entry.state.clone(),
                prepared_unix_seconds: entry.prepared_unix_seconds,
                replayable: matches!(entry.state, LedgerState::Committed | LedgerState::Failed),
                result: entry.result.clone(),
            })
            .collect()
    }

    pub fn record(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        self.record_once(request_id, None, result, path)
    }

    pub fn record_once(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        match self.begin(request_id, transaction_id, &path)? {
            LedgerBegin::Started => self.complete(request_id, result, &path),
            LedgerBegin::Replayed(_) => Err(BridgeError::new(
                "duplicate_request",
                "request or transaction has already been executed",
            )),
        }
    }

    /// Idempotent mutation helper for CLI/LLM clients.  Unlike the legacy
    /// `record_once`, a duplicate request is not surfaced as an error: the
    /// previously committed result is returned and the caller can safely
    /// present it as a replay.  This is the stable retry contract for clients
    /// that may lose the response after the side effect commits.
    pub fn record_or_replay(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<LedgerOutcome, BridgeError> {
        match self.begin(request_id, transaction_id, &path)? {
            LedgerBegin::Started => {
                self.complete(request_id, result.clone(), &path)?;
                Ok(LedgerOutcome::Applied(result))
            }
            LedgerBegin::Replayed(previous) => Ok(LedgerOutcome::Replayed(previous)),
        }
    }

    /// Durably mark a prepared request as applying immediately before the
    /// native side effect.  Recovery can now distinguish a request that was
    /// only admitted from one that may have reached the external boundary.
    pub fn mark_applying(
        &mut self,
        request_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            if !matches!(entry.state, LedgerState::Prepared) {
                return Err(BridgeError::new(
                    "ledger_not_prepared",
                    "request is not in prepared state",
                ));
            }
            let updated = LedgerEntry {
                state: LedgerState::Applying,
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    /// Persist the in-flight marker before any native side effect occurs.
    pub fn begin(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        path: impl AsRef<Path>,
    ) -> Result<LedgerBegin, BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            validate_ledger_ids(request_id, transaction_id)?;
            let transaction_key = transaction_id.map(|value| format!("transaction:{value}"));
            let existing = ledger.entries.get(request_id).or_else(|| {
                transaction_key
                    .as_deref()
                    .and_then(|key| ledger.entries.get(key))
            });
            if let Some(entry) = existing {
                return match entry.state {
                    LedgerState::Committed | LedgerState::Failed => Ok(LedgerBegin::Replayed(
                        entry
                            .result
                            .clone()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )),
                    LedgerState::Prepared | LedgerState::Applying => Err(BridgeError::new(
                        "ledger_in_flight",
                        "request is already applying; reconciliation is required before retry",
                    )),
                };
            }
            let entry = LedgerEntry {
                transaction_id: transaction_id.map(str::to_owned),
                state: LedgerState::Prepared,
                result: None,
                prepared_unix_seconds: unix_seconds(),
            };
            ledger.entries.insert(request_id.to_owned(), entry.clone());
            if let Some(key) = transaction_key {
                ledger.entries.insert(key, entry);
            }
            Ok(LedgerBegin::Started)
        })
    }

    /// Publish the result of a mutation that was previously marked prepared.
    pub fn complete(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            if !matches!(entry.state, LedgerState::Prepared | LedgerState::Applying) {
                return Err(BridgeError::new(
                    "ledger_not_applying",
                    "request is not in flight",
                ));
            }
            let updated = LedgerEntry {
                state: LedgerState::Committed,
                result: Some(result),
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    /// Record a terminal failure so automatic retries cannot repeat a
    /// mutation whose rollback status is unknown.
    pub fn fail(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            let updated = LedgerEntry {
                state: LedgerState::Failed,
                result: Some(result),
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    fn with_locked<T>(
        &mut self,
        path: &Path,
        operation: impl FnOnce(&mut Self) -> Result<T, BridgeError>,
    ) -> Result<T, BridgeError> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let _lock = LedgerLock::acquire(path)?;
        if path.exists() {
            *self = Self::open(path)?;
        }
        let result = operation(self)?;
        self.persist(path)?;
        Ok(result)
    }

    fn persist(&self, path: &Path) -> Result<(), BridgeError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| BridgeError::new("ledger_encode_failed", error.to_string()))?;
        let temp = path.with_extension(format!(
            "tmp-{}-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
            crate::project_history::content_hash(&bytes)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| BridgeError::new("ledger_write_failed", error.to_string()))?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(&temp);
            return Err(BridgeError::new("ledger_write_failed", error.to_string()));
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(BridgeError::new("ledger_publish_failed", error.to_string()));
        }
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    BridgeError::new("ledger_directory_sync_failed", error.to_string())
                })?;
        }
        Ok(())
    }
}

/// Public JSON boundary for read-only audit consumers such as the CLI,
/// Codex, and other automation clients.
pub fn audit_log_json(path: impl AsRef<Path>) -> Result<serde_json::Value, BridgeError> {
    let ledger = RequestLedger::open(path)?;
    serde_json::to_value(ledger.audit_log())
        .map_err(|error| BridgeError::new("audit_log_encode_failed", error.to_string()))
}

fn validate_ledger_ids(request_id: &str, transaction_id: Option<&str>) -> Result<(), BridgeError> {
    if !valid_token(request_id, 128) {
        return Err(BridgeError::new("invalid_request_id", "invalid request ID"));
    }
    if transaction_id.is_some_and(|value| !valid_token(value, 128)) {
        return Err(BridgeError::new(
            "invalid_transaction_id",
            "invalid transaction ID",
        ));
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Validates a command against the generations observed by the caller.
/// Commands originating from an older UI/CLI snapshot must be rejected before
/// they reach native mutation; callers can then refresh and present a new
/// diff instead of applying edits to the wrong project.
pub fn validate_for_generations(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, String> {
    let command = validate(document)?;
    if let Some(expected) = command.expected_generation {
        if expected != current_project_generation {
            return Err(format!(
                "stale_project_generation: expected {expected}, current {current_project_generation}"
            ));
        }
    }
    if let Some(expected) = command.expected_audio_generation {
        if expected != current_audio_generation {
            return Err(format!(
                "stale_audio_generation: expected {expected}, current {current_audio_generation}"
            ));
        }
    }
    Ok(command)
}

/// Apply-time validation is stricter than dry-run validation.  A mutation
/// without the generations from the snapshot it was derived from is
/// inherently unsafe: it can target a project that changed between diff and
/// apply.  Read-only inspection is the sole exception.
pub fn validate_for_apply(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, String> {
    let command = validate_for_generations(
        document,
        current_project_generation,
        current_audio_generation,
    )?;
    let read_only = command.actions.len() == 1
        && matches!(
            command.actions[0],
            CommandAction::ControlInspect
                | CommandAction::AnalyzeDynamics { .. }
                | CommandAction::AnalyzeMix { .. }
                | CommandAction::AnalyzeSilence { .. }
                | CommandAction::PreviewVocalPitchCorrection { .. }
                | CommandAction::ProjectSearch { .. }
                | CommandAction::OpenUtauNotes { .. }
                | CommandAction::DiffMixSnapshots { .. }
                | CommandAction::RecallMixSnapshot { .. }
                | CommandAction::ExtensionCatalog { .. }
                | CommandAction::ExtensionValidate { .. }
                | CommandAction::ProjectInspect
                | CommandAction::RenderTargetCatalog
                | CommandAction::PluginCatalog
                | CommandAction::PluginSearch { .. }
        );
    if !read_only && command.expected_generation.is_none() {
        return Err(
            "missing_project_generation: apply commands must include the snapshot generation"
                .into(),
        );
    }
    if !read_only && command.expected_audio_generation.is_none() {
        return Err("missing_audio_generation: apply commands must include the audio configuration generation".into());
    }
    Ok(command)
}

/// Structured counterpart used by new adapters. The string-returning
/// function above remains for compatibility with existing CLI callers.
pub fn validate_for_generations_diagnostic(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, BridgeError> {
    let command =
        validate(document).map_err(|message| BridgeError::new("invalid_command", message))?;
    if let Some(expected) = command.expected_generation {
        if expected != current_project_generation {
            return Err(BridgeError::new(
                "stale_project_generation",
                format!("expected {expected}, current {current_project_generation}"),
            )
            .retryable(true)
            .at_generation(current_project_generation));
        }
    }
    if let Some(expected) = command.expected_audio_generation {
        if expected != current_audio_generation {
            return Err(BridgeError::new(
                "stale_audio_generation",
                format!("expected {expected}, current {current_audio_generation}"),
            )
            .retryable(true)
            .at_generation(current_audio_generation));
        }
    }
    Ok(command)
}

pub fn validate_for_apply_diagnostic(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, BridgeError> {
    let command = validate_for_generations_diagnostic(
        document,
        current_project_generation,
        current_audio_generation,
    )?;
    let read_only = command.actions.len() == 1
        && matches!(
            command.actions[0],
            CommandAction::ControlInspect
                | CommandAction::AnalyzeDynamics { .. }
                | CommandAction::AnalyzeMix { .. }
                | CommandAction::AnalyzeSilence { .. }
                | CommandAction::PreviewVocalPitchCorrection { .. }
                | CommandAction::ProjectSearch { .. }
                | CommandAction::ExtensionCatalog { .. }
                | CommandAction::ExtensionValidate { .. }
                | CommandAction::ProjectInspect
                | CommandAction::RenderTargetCatalog
                | CommandAction::PluginCatalog
        );
    if !read_only && command.expected_generation.is_none() {
        return Err(BridgeError::new(
            "missing_project_generation",
            "apply commands must include the snapshot generation",
        )
        .retryable(true)
        .at_generation(current_project_generation));
    }
    if !read_only && command.expected_audio_generation.is_none() {
        return Err(BridgeError::new(
            "missing_audio_generation",
            "apply commands must include the audio configuration generation",
        )
        .retryable(true)
        .at_generation(current_audio_generation));
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamics_analysis_is_read_only_and_bounded() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "dynamics-analysis".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeDynamics {
                samples: vec![0.0, 0.25, -0.5, 0.1],
                track_id: Some(7),
            }],
        };
        let command = validate_for_apply(document, 1, 1).expect("analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        let core = crate::AuraCore::new().expect("core must initialize");
        let report =
            crate::command_executor::execute(&core, &command).expect("analysis should execute");
        assert_eq!(report.results[0]["operation"], "analyze_dynamics");
        assert_eq!(report.results[0]["track_id"], 7);
        assert_eq!(report.results[0]["gain_staging_target_db"], -6.0);
        assert!(report.results[0]["recommended_gain_db"].is_number());
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-dynamics".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeDynamics {
                samples: vec![f32::NAN],
                track_id: None
            }],
        })
        .is_err());
    }

    #[test]
    fn silence_analysis_is_read_only_and_returns_bounded_ranges() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "silence-analysis".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeSilence {
                samples: vec![0.0, 0.0, 0.8, 0.0],
                threshold: 0.01,
                min_length: 2,
            }],
        };
        let command =
            validate_for_apply(document, 1, 1).expect("silence analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-silence".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeSilence {
                samples: vec![0.0],
                threshold: 2.0,
                min_length: 1,
            }],
        })
        .is_err());
    }

    #[test]
    fn mix_analysis_is_read_only_and_rejects_non_finite_channels() {
        let command = validate_for_apply(
            CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: "mix-analysis".into(),
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::AnalyzeMix {
                    left: vec![0.5; 8],
                    right: vec![-0.5; 8],
                    reference_left: Vec::new(),
                    reference_right: Vec::new(),
                    ab_left: Vec::new(),
                    ab_right: Vec::new(),
                }],
            },
            1,
            1,
        )
        .expect("mix analysis should be admitted");
        assert_eq!(command.mutation_class, MutationClass::ReadOnly);
        assert!(validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "bad-mix".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AnalyzeMix {
                left: vec![f32::NAN],
                right: vec![0.0],
                reference_left: Vec::new(),
                reference_right: Vec::new(),
                ab_left: Vec::new(),
                ab_right: Vec::new()
            }],
        })
        .is_err());
    }

    #[test]
    fn dynamics_suggestion_apply_is_reversible_and_requires_generations() {
        let document = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "dynamics-apply".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(7),
            expected_audio_generation: Some(3),
            actions: vec![CommandAction::ApplyDynamicsSuggestion {
                track_id: 1,
                plugin_index: 0,
                samples: vec![0.0, 0.25, -0.5, 0.1],
            }],
        };
        let command = validate_for_apply(document, 7, 3).expect("apply should be admitted");
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate_for_apply(
            CommandDocument {
                expected_generation: None,
                expected_audio_generation: Some(3),
                ..CommandDocument {
                    schema_version: 1,
                    command_version: 1,
                    transaction: "dynamics-apply".into(),
                    permission: Permission::ProjectWrite,
                    expected_generation: None,
                    expected_audio_generation: Some(3),
                    actions: vec![CommandAction::ApplyDynamicsSuggestion {
                        track_id: 1,
                        plugin_index: 0,
                        samples: vec![0.0],
                    }],
                }
            },
            7,
            3
        )
        .is_err());
    }

    fn route_gain_document(gain: f32, enabled: bool) -> CommandDocument {
        CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "route-gain-test".into(),
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetRouteGain {
                source_id: 1,
                dest_id: 2,
                gain,
                enabled,
            }],
        }
    }

    #[test]
    fn route_gain_command_accepts_bounded_values() {
        assert!(validate(route_gain_document(0.0, false)).is_ok());
        assert!(validate(route_gain_document(1.0, true)).is_ok());
        assert!(validate(route_gain_document(2.0, true)).is_ok());
    }

    #[test]
    fn route_gain_command_rejects_invalid_values() {
        assert!(validate(route_gain_document(-0.01, false)).is_err());
        assert!(validate(route_gain_document(2.01, true)).is_err());
        assert!(validate(route_gain_document(0.0, true)).is_err());
        let mut self_route = route_gain_document(1.0, true);
        self_route.actions = vec![CommandAction::SetRouteGain {
            source_id: 7,
            dest_id: 7,
            gain: 1.0,
            enabled: true,
        }];
        assert!(validate(self_route).is_err());
    }

    #[test]
    fn rejects_invalid_protocol_envelopes() {
        let request = ProtocolRequest {
            protocol: "aura.command.v0".into(),
            request_id: "1".into(),
            client: "codex".into(),
            command: CommandDocument {
                transaction: "inspect".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::ProjectInspect],
            },
        };
        assert_eq!(
            validate_request_envelope(&request).unwrap_err().code,
            "unsupported_protocol"
        );
    }

    #[test]
    fn rejects_untraceable_request_metadata() {
        let request = ProtocolRequest {
            protocol: PROTOCOL_VERSION.into(),
            request_id: "".into(),
            client: "".into(),
            command: CommandDocument {
                transaction: "inspect".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ReadOnly,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::ProjectInspect],
            },
        };
        assert_eq!(
            validate_request_envelope(&request).unwrap_err().code,
            "invalid_request_id"
        );
    }

    #[test]
    fn validates_declarative_batch() {
        let command = validate(CommandDocument {
            transaction: "vocal-polish".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddTrack {
                name: "Vocal FX".into(),
                track_type: 0,
            }],
        })
        .unwrap();
        assert_eq!(command.transaction, "vocal-polish");
    }

    #[test]
    fn validates_recording_lifecycle_limits() {
        let command = validate(CommandDocument {
            transaction: "record-vocal".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecordStart {
                sample_rate: 48_000.0,
                channels: 2,
                max_frames: 48_000 * 60 * 5,
                start_sample: 0,
                count_in_frames: 0,
            }],
        })
        .unwrap();
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate(CommandDocument {
            transaction: "invalid-record".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecordStart {
                sample_rate: 48_000.0,
                channels: 2,
                max_frames: 16_777_217,
                start_sample: 0,
                count_in_frames: 0,
            }],
        })
        .is_err());
    }

    #[test]
    fn validates_macro_mapping_wire_contract() {
        let command = validate(CommandDocument {
            transaction: "macro-bind".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddMacroMapping {
                mapping_id: "cutoff".into(),
                macro_index: 3,
                target_instance_id: "track:1:slot:0".into(),
                target_parameter_id: "12".into(),
                min: 0.1,
                max: 0.9,
                curve: 0.2,
                invert: false,
            }],
        })
        .unwrap();
        assert_eq!(command.mutation_class, MutationClass::Reversible);
        assert!(validate(CommandDocument {
            transaction: "bad-macro-bind".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::AddMacroMapping {
                mapping_id: "cutoff".into(),
                macro_index: 3,
                target_instance_id: "../../escape".into(),
                target_parameter_id: "12".into(),
                min: 0.1,
                max: 0.9,
                curve: 0.2,
                invert: false,
            }],
        })
        .is_err());
    }

    #[test]
    fn openutau_operation_accepts_canonical_and_legacy_wire_names() {
        let base = serde_json::json!({
            "track_id": 1,
            "source_path": "voice.ustx",
            "rendered_audio_path": "voice.wav"
        });
        let canonical = serde_json::from_value::<CommandAction>({
            let mut value = base.clone();
            value["op"] = serde_json::json!("open_utau_import");
            value
        })
        .unwrap();
        let legacy = serde_json::from_value::<CommandAction>({
            let mut value = base;
            value["op"] = serde_json::json!("openutau_import");
            value
        })
        .unwrap();
        assert_eq!(canonical, legacy);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_values() {
        for value in [f32::NAN, f32::INFINITY, 3.0] {
            assert!(validate(CommandDocument {
                transaction: "bad".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ProjectWrite,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::SetVolume { track_id: 1, value }],
            })
            .is_err());
        }
    }

    #[test]
    fn eq_command_is_reversible_and_rejects_non_finite_bands() {
        let valid = CommandDocument {
            transaction: "eq-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetEq {
                track_id: 1,
                low_band: 0.2,
                low_cut: 0.1,
                high_band: 0.7,
                high_cut: 0.8,
            }],
        };
        let validated = validate(valid).expect("EQ command should validate");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        let invalid = CommandDocument {
            transaction: "bad-eq".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetEq {
                track_id: 1,
                low_band: f32::NAN,
                low_cut: 0.1,
                high_band: 0.7,
                high_cut: 0.8,
            }],
        };
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn validates_sample_automation_and_rejects_unsorted_points() {
        let valid = CommandDocument {
            transaction: "automation".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetAutomation {
                track_id: 1,
                parameter_id: 7,
                points: vec![0.0, 0.2, 0.0, 22050.0, 0.8, 0.2, 44100.0, 0.4, 0.0],
            }],
        };
        assert!(validate(valid).is_ok());

        let mut unsorted = CommandDocument {
            transaction: "automation-unsorted".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetAutomation {
                track_id: 1,
                parameter_id: 7,
                points: vec![22050.0, 0.2, 0.0, 0.0, 0.8, 0.0],
            }],
        };
        assert!(validate(unsorted.clone()).is_err());
        unsorted.actions = vec![CommandAction::SetAutomation {
            track_id: 1,
            parameter_id: 7,
            points: vec![0.0, 0.2, 0.0, 22050.0, 1.1, 0.0],
        }];
        assert!(validate(unsorted).is_err());
    }

    #[test]
    fn read_only_commands_cannot_mutate_project_state() {
        let document = CommandDocument {
            transaction: "read-only-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::SetVolume {
                track_id: 1,
                value: 0.5,
            }],
        };
        assert_eq!(
            validate(document).unwrap_err(),
            "read_only permission allows project inspection only"
        );
    }

    #[test]
    fn read_only_project_inspection_remains_allowed() {
        let document = CommandDocument {
            transaction: "inspect".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        };
        assert!(validate(document).is_ok());
    }

    #[test]
    fn rejects_commands_from_stale_project_or_audio_generations() {
        let command = CommandDocument {
            transaction: "stale".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(4),
            expected_audio_generation: Some(9),
            actions: vec![CommandAction::Undo],
        };
        assert!(validate_for_generations(command.clone(), 3, 9)
            .unwrap_err()
            .starts_with("stale_project_generation:"));
        assert!(validate_for_generations(command, 4, 8)
            .unwrap_err()
            .starts_with("stale_audio_generation:"));
    }

    #[test]
    fn accepts_commands_when_both_generations_match() {
        let command = CommandDocument {
            transaction: "current".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(4),
            expected_audio_generation: Some(9),
            actions: vec![CommandAction::Undo],
        };
        assert!(validate_for_generations(command, 4, 9).is_ok());
    }

    #[test]
    fn apply_rejects_mutation_without_both_snapshot_generations() {
        let base = CommandDocument {
            transaction: "unsafe-apply".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::SetVolume {
                track_id: 1,
                value: 1.0,
            }],
        };
        assert!(validate_for_apply(base.clone(), 1, 1)
            .unwrap_err()
            .starts_with("missing_project_generation:"));

        let mut audio_missing = base;
        audio_missing.expected_generation = Some(1);
        assert!(validate_for_apply(audio_missing, 1, 1)
            .unwrap_err()
            .starts_with("missing_audio_generation:"));
    }

    #[test]
    fn structured_apply_validation_preserves_generation_context() {
        let error = validate_for_apply_diagnostic(
            CommandDocument {
                transaction: "edit".into(),
                schema_version: 1,
                command_version: 1,
                permission: Permission::ProjectWrite,
                expected_generation: None,
                expected_audio_generation: None,
                actions: vec![CommandAction::Undo],
            },
            42,
            7,
        )
        .unwrap_err();
        assert_eq!(error.code, "missing_project_generation");
        assert_eq!(error.generation, Some(42));
        assert!(error.retryable);
    }

    #[test]
    fn apply_allows_read_only_inspection_without_generations() {
        let command = CommandDocument {
            transaction: "inspect".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        };
        assert!(validate_for_apply(command, 1, 1).is_ok());
    }

    #[test]
    fn apply_allows_plugin_catalog_without_generations() {
        let command = CommandDocument {
            transaction: "catalog".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::PluginCatalog],
        };
        assert!(validate_for_apply(command, 1, 1).is_ok());
    }

    #[test]
    fn snapshot_generation_is_stable_and_content_scoped() {
        assert_eq!(
            snapshot_generation(b"layout"),
            snapshot_generation(b"layout")
        );
        assert_ne!(
            snapshot_generation(b"layout"),
            snapshot_generation(b"layout-v2")
        );
    }

    #[test]
    fn mutation_classes_distinguish_reversible_and_external_effects() {
        assert_eq!(
            mutation_class(&CommandAction::ProjectInspect),
            MutationClass::ReadOnly
        );
        assert_eq!(
            mutation_class(&CommandAction::SetPan {
                track_id: 1,
                value: 0.0
            }),
            MutationClass::Reversible
        );
        assert_eq!(
            mutation_class(&CommandAction::BounceProject {
                path: "mix.wav".into(),
                format: 0
            }),
            MutationClass::ExternalSideEffect
        );
        assert_eq!(
            mutation_class(&CommandAction::ProjectLoad {
                path: "project.aura".into(),
            }),
            MutationClass::Irreversible
        );
        assert_eq!(
            mutation_class(&CommandAction::RemoveTrack { track_id: 1 }),
            MutationClass::Irreversible
        );
        assert_eq!(
            mutation_class(&CommandAction::SetMute {
                track_id: 1,
                muted: true,
            }),
            MutationClass::Reversible
        );
        assert_eq!(
            mutation_class(&CommandAction::ExtensionSetEnabled {
                root: "project".into(),
                extension_id: "example".into(),
                enabled: false,
            }),
            MutationClass::Reversible
        );
    }

    #[test]
    fn daily_mix_actions_round_trip_and_have_explicit_diffs() {
        let document = CommandDocument {
            transaction: "mix-edit".into(),
            schema_version: 1,
            command_version: 1,
            permission: Permission::ProjectWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![
                CommandAction::SetMute {
                    track_id: 7,
                    muted: true,
                },
                CommandAction::RemovePlugin {
                    track_id: 7,
                    plugin_index: 2,
                },
            ],
        };
        let encoded = serde_json::to_value(&document).unwrap();
        let decoded: CommandDocument = serde_json::from_value(encoded).unwrap();
        let validated = validate(decoded).unwrap();
        let summaries = diff(&validated)
            .into_iter()
            .map(|item| item.summary)
            .collect::<Vec<_>>();
        assert_eq!(summaries[0], "set track 7 mute: true");
        assert_eq!(summaries[1], "remove plugin 2 from track 7");
        assert_eq!(validated.mutation_class, MutationClass::Irreversible);
    }

    #[test]
    fn midi_inspection_is_read_only_and_round_trips() {
        let document = CommandDocument {
            permission: Permission::ReadOnly,
            actions: vec![CommandAction::InspectMidiNotes],
            ..validated_command_document("inspect-midi")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::ReadOnly);
        assert_eq!(
            diff(&validated)[0].summary,
            "inspect canonical MIDI notes and lyrics"
        );
    }

    #[test]
    fn transport_pause_round_trips_and_is_reversible() {
        let document = CommandDocument {
            actions: vec![CommandAction::TransportPause],
            ..validated_command_document("pause")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.actions, vec![CommandAction::TransportPause]);
        assert_eq!(diff(&validated)[0].summary, "pause transport");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
    }

    #[test]
    fn vca_actions_round_trip_validate_and_describe() {
        let document = CommandDocument {
            actions: vec![
                CommandAction::AddVcaGroup {
                    group_id: 9,
                    gain: 0.75,
                },
                CommandAction::AssignTrackToVca {
                    track_id: 12,
                    group_id: 9,
                },
                CommandAction::SetVcaGroupGain {
                    group_id: 9,
                    gain: 0.5,
                },
            ],
            ..validated_command_document("vca-edit")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        let summaries = diff(&validated)
            .into_iter()
            .map(|item| item.summary)
            .collect::<Vec<_>>();
        assert_eq!(summaries[0], "add VCA group 9 at gain 0.75");
        assert_eq!(summaries[1], "assign track 12 to VCA group 9");
        assert_eq!(summaries[2], "set VCA group 9 gain to 0.5");
        assert_eq!(validated.mutation_class, MutationClass::Reversible);

        for action in [
            CommandAction::AddVcaGroup {
                group_id: 0,
                gain: 1.0,
            },
            CommandAction::SetVcaGroupGain {
                group_id: 9,
                gain: f32::NAN,
            },
            CommandAction::AssignTrackToVca {
                track_id: 0,
                group_id: 9,
            },
        ] {
            let invalid = CommandDocument {
                actions: vec![action],
                ..validated_command_document("bad-vca")
            };
            assert!(validate(invalid).is_err());
        }
    }

    #[test]
    fn low_latency_action_round_trips_as_reversible() {
        let document = CommandDocument {
            actions: vec![CommandAction::SetLowLatencyMode { enabled: true }],
            ..validated_command_document("latency-edit")
        };
        let decoded: CommandDocument =
            serde_json::from_value(serde_json::to_value(&document).unwrap()).unwrap();
        let validated = validate(decoded).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        assert_eq!(diff(&validated)[0].summary, "enable low-latency monitoring");
    }

    #[test]
    fn tonal_scale_action_validates_root_and_mode() {
        let document = CommandDocument {
            actions: vec![CommandAction::SetTonalScale {
                root: 7,
                scale_type: 0,
            }],
            ..validated_command_document("tonal-edit")
        };
        let validated = validate(document).unwrap();
        assert_eq!(validated.mutation_class, MutationClass::Reversible);
        assert_eq!(
            diff(&validated)[0].summary,
            "set tonal scale root 7, type 0"
        );

        for action in [
            CommandAction::SetTonalScale {
                root: 12,
                scale_type: 11,
            },
            CommandAction::SetTonalScale {
                root: -129,
                scale_type: 0,
            },
        ] {
            assert!(validate(CommandDocument {
                actions: vec![action],
                ..validated_command_document("invalid-tonal-edit")
            })
            .is_err());
        }
    }

    #[test]
    fn project_load_requires_generations_and_project_write() {
        let command = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "load".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(7),
            expected_audio_generation: Some(3),
            actions: vec![CommandAction::ProjectLoad {
                path: "other.aura".into(),
            }],
        };
        assert!(validate_for_apply(command, 7, 3).is_ok());

        let missing_generations = CommandDocument {
            expected_generation: None,
            expected_audio_generation: None,
            ..CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: "load-missing-generation".into(),
                permission: Permission::ProjectWrite,
                expected_generation: Some(7),
                expected_audio_generation: Some(3),
                actions: vec![CommandAction::ProjectLoad {
                    path: "other.aura".into(),
                }],
            }
        };
        let error = validate_for_apply_diagnostic(missing_generations, 7, 3).unwrap_err();
        assert_eq!(error.code, "missing_project_generation");
    }

    #[test]
    fn project_load_paths_use_the_same_root_policy_as_render_paths() {
        let root = std::env::temp_dir().join(format!("aura-command-load-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let actions = vec![CommandAction::ProjectLoad {
            path: "../outside.aura".into(),
        }];
        let error =
            validate_command_action_paths(&actions, Permission::ProjectWrite, &root).unwrap_err();
        assert_eq!(error.code, "path_traversal");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn external_file_operations_require_system_write_permission() {
        let base = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "save".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::SaveProject {
                path: "project.aura".into(),
            }],
        };
        assert!(validate(base.clone()).is_err());
        let allowed = CommandDocument {
            permission: Permission::SystemWrite,
            ..base
        };
        assert!(validate(allowed).is_ok());
    }

    #[test]
    fn selected_stem_targets_are_validated_and_described() {
        let command = CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "selected-stems".into(),
            permission: Permission::SystemWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: vec![CommandAction::BounceStems {
                output_dir: "stems".into(),
                format: 0,
                track_ids: vec![2, 5],
                tail_seconds: 2.0,
                pre_fader: false,
                include_inserts: true,
            }],
        };
        let validated = validate(command).unwrap();
        assert!(diff(&validated)[0].summary.contains("2 selected stems"));

        let duplicate = CommandDocument {
            actions: vec![CommandAction::BounceStems {
                output_dir: "stems".into(),
                format: 0,
                track_ids: vec![2, 2],
                tail_seconds: 2.0,
                pre_fader: false,
                include_inserts: true,
            }],
            ..validated_command_document("duplicate-stems")
        };
        assert!(validate(duplicate).is_err());
    }

    fn validated_command_document(transaction: &str) -> CommandDocument {
        CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: transaction.into(),
            permission: Permission::SystemWrite,
            expected_generation: Some(1),
            expected_audio_generation: Some(1),
            actions: Vec::new(),
        }
    }

    #[test]
    fn project_paths_cannot_escape_root() {
        let root = std::env::temp_dir().join(format!("aura-command-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(validate_project_path(&root, "mix.wav").is_ok());
        assert_eq!(
            validate_project_path(&root, "../outside.wav")
                .unwrap_err()
                .code,
            "path_traversal"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unrestricted_policy_allows_explicit_external_paths() {
        let root =
            std::env::temp_dir().join(format!("aura-unrestricted-root-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("aura-unrestricted-output-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let requested = outside.join("render.wav");
        let resolved =
            validate_command_path(&root, requested.to_str().unwrap(), PathPolicy::Unrestricted)
                .unwrap();
        assert_eq!(resolved, outside.canonicalize().unwrap().join("render.wav"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn output_paths_require_explicit_overwrite() {
        let root = std::env::temp_dir().join(format!("aura-output-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mix.wav"), b"existing").unwrap();
        assert_eq!(
            validate_project_output_path(&root, "mix.wav", false)
                .unwrap_err()
                .code,
            "overwrite_confirmation_required"
        );
        assert!(validate_project_output_path(&root, "mix.wav", true).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn command_paths_reject_symlink_components() {
        let root = std::env::temp_dir().join(format!("aura-symlink-root-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("aura-symlink-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        assert_eq!(
            validate_project_path(&root, "linked/output.wav")
                .unwrap_err()
                .code,
            "symlink_path"
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn request_ledger_replays_and_rejects_duplicate_requests() {
        let root = std::env::temp_dir().join(format!("aura-ledger-{}", std::process::id()));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        ledger
            .record("req-1", serde_json::json!({"ok": true}), &path)
            .unwrap();
        assert_eq!(
            ledger.replay("req-1"),
            Some(serde_json::json!({"ok": true}))
        );
        assert!(ledger
            .record("req-1", serde_json::json!({"ok": false}), &path)
            .is_err());
        assert!(ledger
            .record_once(
                "req-2",
                Some("tx-1"),
                serde_json::json!({"ok": true}),
                &path
            )
            .is_ok());
        assert!(ledger
            .record_once(
                "req-3",
                Some("tx-1"),
                serde_json::json!({"ok": false}),
                &path
            )
            .is_err());
        let lock_path = path.with_extension("lock");
        std::fs::write(&lock_path, b"other process").unwrap();
        assert_eq!(
            ledger
                .record("req-4", serde_json::json!({}), &path)
                .unwrap_err()
                .code,
            "ledger_busy"
        );
        let _ = std::fs::remove_file(lock_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_persists_in_flight_before_completion() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-in-flight-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .begin("req-flight", Some("tx-flight"), &path)
                .unwrap(),
            LedgerBegin::Started
        );

        let reopened = RequestLedger::open(&path).unwrap();
        assert_eq!(reopened.replay("req-flight"), None);
        let mut retry = reopened;
        assert_eq!(
            retry
                .begin("req-flight", Some("tx-flight"), &path)
                .unwrap_err()
                .code,
            "ledger_in_flight"
        );

        ledger
            .complete("req-flight", serde_json::json!({"ok": true}), &path)
            .unwrap();
        let completed = RequestLedger::open(&path).unwrap();
        assert_eq!(
            completed.replay("req-flight"),
            Some(serde_json::json!({"ok": true}))
        );
        assert_eq!(
            completed.replay("transaction:tx-flight"),
            Some(serde_json::json!({"ok": true}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_replay_is_a_successful_idempotent_outcome() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-replay-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .record_or_replay(
                    "req-replay",
                    Some("tx-replay"),
                    serde_json::json!({"created": "track-1"}),
                    &path,
                )
                .unwrap(),
            LedgerOutcome::Applied(serde_json::json!({"created": "track-1"}))
        );
        assert_eq!(
            ledger
                .record_or_replay(
                    "req-replay",
                    Some("tx-replay"),
                    serde_json::json!({"created": "track-2"}),
                    &path,
                )
                .unwrap(),
            LedgerOutcome::Replayed(serde_json::json!({"created": "track-1"}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audit_log_is_deterministic_and_marks_replayable_results() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-audit-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        ledger
            .record("req-b", serde_json::json!({"ok": true}), &path)
            .unwrap();
        ledger
            .record("req-a", serde_json::json!({"ok": false}), &path)
            .unwrap();
        let log = ledger.audit_log();
        assert_eq!(
            log.iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["req-a", "req-b"]
        );
        assert!(log.iter().all(|entry| entry.replayable));
        assert_eq!(log[0].result, Some(serde_json::json!({"ok": false})));
        let json = audit_log_json(&path).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["request_id"], "req-a");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_publishes_applying_state_before_side_effect() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-applying-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        let mut ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .begin("req-applying", Some("tx-applying"), &path)
                .unwrap(),
            LedgerBegin::Started
        );
        ledger.mark_applying("req-applying", &path).unwrap();
        let mut reopened = RequestLedger::open(&path).unwrap();
        let entry = reopened.entries.get("req-applying").unwrap();
        assert!(matches!(entry.state, LedgerState::Applying));
        assert!(reopened
            .begin("req-applying", Some("tx-applying"), &path)
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn transaction_lock_creates_parent_for_a_new_project() {
        let root = std::env::temp_dir().join(format!(
            "aura-transaction-lock-parent-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let ledger_path = root.join(".aura").join("request-ledger.json");
        let lock = CommandTransactionLock::acquire(&ledger_path).unwrap();
        assert!(root.join(".aura").is_dir());
        assert!(ledger_path.with_extension("transaction.lock").is_file());
        drop(lock);
        assert!(!ledger_path.with_extension("transaction.lock").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_ledger_migrates_result_only_format() {
        let root = std::env::temp_dir().join(format!(
            "aura-ledger-legacy-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = root.join("requests.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, br#"{"legacy-request":{"ok":true}}"#).unwrap();
        let ledger = RequestLedger::open(&path).unwrap();
        assert_eq!(
            ledger.replay("legacy-request"),
            Some(serde_json::json!({"ok": true}))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ledger_lock_drop_does_not_remove_a_replaced_owner() {
        let root = std::env::temp_dir().join(format!("aura-ledger-owner-{}", std::process::id()));
        let path = root.join("requests.json");
        std::fs::create_dir_all(&root).unwrap();
        let lock_path = path.with_extension("lock");
        let lock = LedgerLock::acquire(&path).unwrap();
        std::fs::write(&lock_path, "pid=999 nonce=replaced-owner\n").unwrap();
        drop(lock);
        assert!(lock_path.exists());
        let _ = std::fs::remove_file(lock_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capabilities_advertise_all_history_cli_operations() {
        let advertised = capabilities();
        assert_eq!(
            advertised["midi_read_operations"],
            serde_json::json!(["inspect_midi_notes"])
        );
        let operations = advertised
            .get("operations")
            .and_then(serde_json::Value::as_array)
            .expect("capabilities must expose operations");
        for operation in [
            "project.load",
            "analyze_mix",
            "analyze_silence",
            "add_aux_track",
            "split_region_with_crossfade",
            "select_comp_take",
            "suggest_next_chords",
            "generate_arpeggio",
            "place_arpeggio",
            "history.status",
            "history.log",
            "history.diff",
            "history.commit",
            "history.branch",
            "history.checkout",
            "history.tag",
            "history.revert",
            "history.cherry_pick",
        ] {
            assert!(
                operations
                    .iter()
                    .any(|value| value.as_str() == Some(operation)),
                "missing advertised operation {operation}"
            );
        }
    }

    #[test]
    fn capabilities_do_not_claim_unverified_industry_integrations() {
        let advertised = capabilities();
        let integrations = advertised
            .get("integration_capabilities")
            .expect("integration capability matrix must be public");
        assert_eq!(integrations["ara2"]["verified"], false);
        assert_eq!(integrations["ara2"]["plugin_protocol_bridge"], false);
        assert_eq!(
            integrations["hardware_controllers"]["device_driver_integration"],
            false
        );
        assert_eq!(integrations["immersive_audio"]["dolby_renderer"], false);
        assert_eq!(integrations["immersive_audio"]["metadata_export"], false);
    }

    #[test]
    fn capabilities_advertise_distinct_pause_semantics() {
        let value = capabilities();
        assert_eq!(value["transport_capabilities"]["pause"], true);
        assert_eq!(
            value["transport_capabilities"]["pause_preserves_playhead"],
            true
        );
    }

    #[test]
    fn capabilities_expose_computer_use_power_profile_without_bypassing_audit() {
        let advertised = capabilities();
        let computer_use = &advertised["clients"]["computer_use"];
        assert_eq!(computer_use["supported"], true);
        assert_eq!(computer_use["power_profile"], "unrestricted_explicit");
        assert!(computer_use["requires"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "audit_log"));
        assert_eq!(advertised["safety"]["unknown_operations_rejected"], true);
    }

    #[test]
    fn capabilities_match_external_codec_support() {
        let advertised = capabilities();
        let render = advertised
            .get("render_capabilities")
            .expect("render capabilities must be present");
        let supported = render
            .get("supported_codecs")
            .and_then(serde_json::Value::as_array)
            .expect("supported codecs must be an array");
        let unsupported = render
            .get("unsupported_codecs")
            .and_then(serde_json::Value::as_array)
            .expect("unsupported codecs must be an array");
        for codec in ["aiff_pcm16", "flac", "mp3"] {
            assert!(supported.iter().any(|value| value.as_str() == Some(codec)));
            assert!(!unsupported
                .iter()
                .any(|value| value.as_str() == Some(codec)));
        }
        assert_eq!(
            render
                .get("external_codec_provider")
                .and_then(serde_json::Value::as_str),
            Some("ffmpeg")
        );
        let imports = render
            .get("supported_import_formats")
            .and_then(serde_json::Value::as_array)
            .expect("supported import formats must be an array");
        for format in ["wav", "mp3", "flac", "aiff", "m4a", "ogg", "aac"] {
            assert!(imports.iter().any(|value| value.as_str() == Some(format)));
        }
        let operations = advertised
            .get("operations")
            .and_then(serde_json::Value::as_array)
            .expect("operations must be an array");
        for operation in [
            "plugin_search",
            "set_plugin_favorite",
            "add_vca_group",
            "assign_track_to_vca",
            "set_vca_group_gain",
        ] {
            assert!(operations
                .iter()
                .any(|value| value.as_str() == Some(operation)));
        }
    }
}
