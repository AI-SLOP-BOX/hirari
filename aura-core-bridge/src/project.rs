use crate::persistence::{ProjectMetadata, SovereignPersistence};
use crate::project_contracts::{
    validate_contracts, AutomationPointContract, MacroMappingContract, MidiLearnMappingContract, MidiNoteContract, OpenUtauVocalContract, PluginFormat, TempoEventContract,
    PluginInstanceContract, RenderTargetContract, WarpMarkerContract, CompSegmentContract,
    CompTakeContract, TrackStackContract, MarkerContract, PROJECT_CONTRACT_VERSION,
    FreezeArtifactContract, TimeSignatureEventContract, SidechainRouteContract,
    FeedbackRouteContract, AudioRouteContract, VcaGroupContract,
};
use crate::midi::{MIDIEvent, MIDIEventKind};
use crate::hardware_insert::HardwareInsert;
use crate::harmonic::ChordEvent;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use uuid::Uuid;

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const PLUGIN_STATE_SCHEMA_VERSION: u32 = 1;

/// Return stable JSON pointer paths whose values differ between revisions.
pub fn diff_project_json(previous: &serde_json::Value, current: &serde_json::Value) -> Vec<String> {
    fn walk(a: &serde_json::Value, b: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        match (a, b) {
            (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
                let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect(); keys.sort(); keys.dedup();
                for key in keys { let child = format!("{}/{}", path, key.replace('~', "~0").replace('/', "~1")); match (left.get(key), right.get(key)) { (Some(x), Some(y)) => walk(x,y,&child,out), _ => out.push(child) } }
            }
            (serde_json::Value::Array(left), serde_json::Value::Array(right)) => { if left.len() != right.len() { out.push(format!("{}/length", path)); } for i in 0..left.len().min(right.len()) { walk(&left[i], &right[i], &format!("{}/{}", path, i), out); } }
            _ if a != b => out.push(if path.is_empty() { "/".into() } else { path.into() }),
            _ => {}
        }
    }
    let mut out = Vec::new(); walk(previous, current, "", &mut out); out
}

// These are aggregate admission limits, not performance tuning knobs.  They
// keep a malformed or accidentally enormous project from forcing the native
// hydration path to allocate unbounded memory before it can reject it.
pub const MAX_PROJECT_TRACKS: usize = 4_096;
pub const MAX_PROJECT_REGIONS: usize = 262_144;
pub const MAX_PROJECT_PLUGIN_INSTANCES: usize = 16_384;
pub const MAX_PROJECT_STATE_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_PLUGIN_PARAMETERS: usize = 65_536;
pub const MAX_PROJECT_STRING_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PROJECT_MIDI_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PROJECT_COMP_TAKES: usize = 16_384;
pub const MAX_PROJECT_COMP_SEGMENTS: usize = 262_144;
pub const MAX_FREEZE_SAMPLES: u64 = 64 * 1024 * 1024;
pub const MAX_FREEZE_CACHE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDocument {
    pub schema_version: u32,
    #[serde(default = "default_contract_version")]
    pub contract_version: u32,
    /// Stable identity independent of the project's filesystem path.
    #[serde(default = "new_project_id")]
    pub project_id: String,
    pub metadata: ProjectMetadata,
    pub sample_rate: f64,
    #[serde(default = "default_master_gain")]
    pub master_gain: f32,
    #[serde(default)]
    pub cycle_start_sample: u64,
    #[serde(default)]
    pub cycle_end_sample: u64,
    #[serde(default)]
    pub cycle_enabled: bool,
    #[serde(default)]
    pub metronome_enabled: bool,
    pub tracks: Vec<ProjectTrack>,
    /// Track IDs whose signal path is a Bus but whose user-facing role is Aux.
    #[serde(default)]
    pub aux_track_ids: Vec<u32>,
    pub regions: Vec<ProjectRegion>,
    #[serde(default)]
    pub plugin_instances: Vec<PluginInstanceContract>,
    #[serde(default)]
    pub midi_learn_mappings: Vec<MidiLearnMappingContract>,
    #[serde(default)]
    pub midi_notes: Vec<MidiNoteContract>,
    /// Musical chord-track events used by the composition view and generators.
    #[serde(default)]
    pub chord_track: Vec<ChordEvent>,
    /// Sample-accurate control-plane MIDI, including bounded SysEx and MIDI
    /// 2.0 events.  Scheduled notes remain a separate musical model, while
    /// this stream preserves automation/controller messages across save,
    /// history checkout, and application restart.
    #[serde(default)]
    pub midi_events: Vec<MIDIEvent>,
    #[serde(default)]
    pub tempo_events: Vec<TempoEventContract>,
    #[serde(default)]
    pub time_signature_events: Vec<TimeSignatureEventContract>,
    #[serde(default)]
    pub macro_mappings: Vec<MacroMappingContract>,
    #[serde(default)]
    pub warp_markers: Vec<WarpMarkerContract>,
    #[serde(default)]
    pub render_targets: Vec<RenderTargetContract>,
    #[serde(default)]
    pub freeze_artifacts: Vec<FreezeArtifactContract>,
    #[serde(default)]
    pub sidechain_routes: Vec<SidechainRouteContract>,
    #[serde(default)]
    pub feedback_routes: Vec<FeedbackRouteContract>,
    #[serde(default)]
    pub audio_routes: Vec<AudioRouteContract>,
    #[serde(default)]
    pub openutau_vocals: Vec<OpenUtauVocalContract>,
    #[serde(default)]
    pub comp_takes: Vec<CompTakeContract>,
    #[serde(default)]
    pub comp_segments: Vec<CompSegmentContract>,
    #[serde(default)]
    pub track_stacks: Vec<TrackStackContract>,
    #[serde(default)]
    pub markers: Vec<MarkerContract>,
    #[serde(default)]
    pub vca_groups: Vec<VcaGroupContract>,
    /// External hardware inserts are project-scoped routing contracts. The
    /// audio device layer may be unavailable during offline editing, so the
    /// routing and PDC intent must survive save/reload independently.
    #[serde(default)]
    pub hardware_inserts: Vec<HardwareInsert>,
    /// Canonical monitor/control-room state.  Older projects may still keep
    /// this in the adjacent sidecar; the loader falls back to that format.
    #[serde(default)]
    pub control_room: Option<crate::control_room::ControlRoomState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectTrack {
    pub id: u32,
    pub name: String,
    pub track_type: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    #[serde(default)]
    pub record_armed: bool,
    #[serde(default)]
    pub phase_invert: bool,
    #[serde(default)]
    pub track_delay_samples: u32,
    #[serde(default)]
    pub volume_automation: Vec<AutomationPointContract>,
    #[serde(default)]
    pub pan_automation: Vec<AutomationPointContract>,
    #[serde(default)]
    pub track_delay_automation: Vec<AutomationPointContract>,
    #[serde(default)]
    pub plugin_types: Vec<u32>,
    /// Bypass is part of the insert-slot state, not a property of the
    /// serialized plugin blob. Keep one entry per plugin slot so a reload
    /// cannot silently enable a plugin that was intentionally bypassed.
    #[serde(default)]
    pub plugin_bypasses: Vec<bool>,
    #[serde(default)]
    pub plugin_parameter_values: Vec<Vec<f32>>,
    #[serde(default)]
    pub plugin_states: Vec<Vec<u8>>,
    #[serde(default)]
    pub plugin_gui_states: Vec<Vec<u8>>,
    #[serde(default)]
    pub plugin_state_versions: Vec<u32>,
    #[serde(default)]
    pub sandbox_plugin_paths: Vec<String>,
    #[serde(default)]
    pub sandbox_plugin_states: Vec<Vec<u8>>,
    #[serde(default)]
    pub sandbox_plugin_state_versions: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectRegion {
    pub id: u32,
    pub track_id: u32,
    pub name: String,
    pub path: String,
    pub start: u64,
    pub length: u64,
    #[serde(default)]
    pub source_offset: u64,
    #[serde(default)]
    pub base_source_offset: u64,
    #[serde(default)]
    pub base_length: u64,
    pub muted: bool,
    pub clip_gain: f32,
    pub fade_in_samples: u64,
    pub fade_out_samples: u64,
    #[serde(default = "default_warp_ratio")]
    pub warp_ratio: f64,
    #[serde(default)]
    pub pitch_semitones: f32,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default = "default_loop_count")]
    pub loop_count: u32,
}

include!("project_bundles.rs");
include!("project_hydration.rs");
include!("project_validation.rs");
include!("project_mutations.rs");

fn default_contract_version() -> u32 {
    PROJECT_CONTRACT_VERSION
}

fn new_project_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Debug, Deserialize)]
struct LayoutTrack {
    id: u32,
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default = "default_track_type")]
    track_type: String,
    #[serde(default = "default_volume")]
    volume: f32,
    #[serde(default)]
    pan: f32,
    #[serde(default, alias = "mute")]
    muted: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default, alias = "recordArmed")]
    record_armed: bool,
    #[serde(default, alias = "phaseInvert")]
    phase_invert: bool,
    #[serde(default, alias = "trackDelaySamples")]
    track_delay_samples: u32,
    #[serde(default)]
    frozen: bool,
    #[serde(default)]
    frozen_total_samples: u64,
    #[serde(default)]
    frozen_sample_rate: u32,
    #[serde(default)]
    frozen_path: String,
    #[serde(default)]
    volume_automation: Vec<AutomationPointContract>,
    #[serde(default)]
    pan_automation: Vec<AutomationPointContract>,
    #[serde(default, alias = "trackDelayAutomation")]
    track_delay_automation: Vec<AutomationPointContract>,
    #[serde(default)]
    plugin_types: Vec<u32>,
    #[serde(default, rename = "plugin_bypass", alias = "pluginBypass", alias = "plugin_bypasses")]
    plugin_bypasses: Vec<bool>,
    #[serde(default, alias = "pluginParameterValues")]
    plugin_parameter_values: Vec<Vec<f32>>,
    #[serde(default)]
    plugin_state_hex: Vec<String>,
    #[serde(default)]
    plugin_gui_state_hex: Vec<String>,
    #[serde(default)]
    plugin_state_versions: Vec<u32>,
    #[serde(default)]
    sandbox_plugin_paths: Vec<String>,
    #[serde(default)]
    sandbox_plugin_state_hex: Vec<String>,
    #[serde(default)]
    sandbox_plugin_state_versions: Vec<u32>,
    #[serde(default)]
    sidechain_routes: Vec<SidechainRouteContract>,
    #[serde(default)]
    feedback_routes: Vec<FeedbackRouteContract>,
    #[serde(default)]
    regions: Vec<LayoutRegion>,
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        bail!("plugin state hex has odd length");
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let high = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("plugin state contains invalid hex"))?;
        let low = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("plugin state contains invalid hex"))?;
        bytes.push(((high << 4) | low) as u8);
    }
    Ok(bytes)
}

fn state_versions(versions: &[u32], state_count: usize) -> Result<Vec<u32>> {
    if versions.is_empty() {
        return Ok(vec![PLUGIN_STATE_SCHEMA_VERSION; state_count]);
    }
    if versions.len() != state_count {
        bail!("plugin state version count does not match state count");
    }
    versions
        .iter()
        .map(|version| match *version {
            // Version zero was the pre-versioned raw plugin blob. Its wire
            // bytes are unchanged, so migration is metadata-only and safe.
            0 | PLUGIN_STATE_SCHEMA_VERSION => Ok(PLUGIN_STATE_SCHEMA_VERSION),
            other => bail!("unsupported plugin state version {other}"),
        })
        .collect()
}

fn plugin_bypasses(values: &[bool], plugin_count: usize) -> Result<Vec<bool>> {
    if values.is_empty() {
        return Ok(vec![false; plugin_count]);
    }
    if values.len() != plugin_count {
        bail!("plugin bypass/type counts differ");
    }
    Ok(values.to_vec())
}

fn plugin_parameter_values(values: &[Vec<f32>], plugin_count: usize) -> Result<Vec<Vec<f32>>> {
    if values.is_empty() {
        return Ok(vec![Vec::new(); plugin_count]);
    }
    if values.len() != plugin_count {
        bail!("plugin parameter/type counts differ");
    }
    if values.iter().any(|parameters| parameters.len() > MAX_PLUGIN_PARAMETERS) {
        bail!("plugin parameter values exceed aggregate limit");
    }
    Ok(values.to_vec())
}

#[derive(Debug, Deserialize)]
struct LayoutRegion {
    id: u32,
    #[serde(default)]
    name: String,
    path: String,
    start: u64,
    len: u64,
    #[serde(default)]
    source_offset: u64,
    #[serde(default)]
    base_source_offset: u64,
    #[serde(default)]
    base_length: u64,
    #[serde(default)]
    muted: bool,
    #[serde(default = "default_volume")]
    clip_gain: f32,
    #[serde(default)]
    fade_in_samples: u64,
    #[serde(default)]
    fade_out_samples: u64,
    #[serde(default = "default_warp_ratio")]
    warp_ratio: f64,
    #[serde(default)]
    pitch_semitones: f32,
    #[serde(default)]
    reverse: bool,
    #[serde(default = "default_loop_count")]
    loop_count: u32,
}

fn default_track_type() -> String {
    "Audio".to_string()
}

fn default_volume() -> f32 {
    1.0
}

fn default_master_gain() -> f32 {
    1.0
}

fn default_warp_ratio() -> f64 {
    1.0
}

fn default_loop_count() -> u32 {
    1
}

fn plugin_format_for_path(path: &str) -> PluginFormat {
    match std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("clap") => PluginFormat::Clap,
        Some("vst3") => PluginFormat::Vst3,
        Some("component") => PluginFormat::AudioUnit,
        _ => PluginFormat::BuiltIn,
    }
}

include!("project_tests.rs");
