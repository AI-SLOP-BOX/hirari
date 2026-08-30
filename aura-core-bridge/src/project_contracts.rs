//! Persisted seams for the next generation of the project model.
//!
//! These types deliberately describe control-plane state only.  They do not
//! instantiate plugins or perform audio work, which keeps project hydration,
//! undo, offline rendering, and real-time processing independently evolvable.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const PROJECT_CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginFormat {
    Clap,
    Vst3,
    AudioUnit,
    BuiltIn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginInstanceContract {
    pub instance_id: String,
    #[serde(default)]
    pub track_id: u32,
    #[serde(default)]
    pub slot_index: u32,
    pub format: PluginFormat,
    pub bundle_path: String,
    pub plugin_id: String,
    #[serde(default)]
    pub bus_layout: Vec<String>,
    /// Component identifiers for multi-component formats.
    #[serde(default)]
    pub component_ids: Vec<String>,
    #[serde(default)]
    pub input_channels: u16,
    #[serde(default)]
    pub output_channels: u16,
    #[serde(default)]
    pub sidechain_channels: u16,
    #[serde(default)]
    pub parameter_ids: Vec<String>,
    #[serde(default)]
    pub parameter_values: Vec<f32>,
    #[serde(default)]
    pub latency_samples: u32,
    #[serde(default)]
    pub state_blob: Vec<u8>,
    #[serde(default)]
    pub gui_state: Vec<u8>,
    #[serde(default)]
    pub bypassed: bool,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub quarantined: bool,
    /// Fingerprint captured at admission time.  Empty is allowed for legacy
    /// projects and means the instance must be re-admitted before trusting a
    /// cached state blob.
    #[serde(default)]
    pub binary_hash: String,
    #[serde(default)]
    pub capability: String,
    #[serde(default)]
    pub plugin_version: String,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub state_schema_version: u32,
    #[serde(default)]
    pub state_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiLearnMappingContract {
    pub mapping_id: String,
    pub device_id: String,
    pub channel: u8,
    pub controller: u16,
    pub target_instance_id: String,
    pub target_parameter_id: String,
    #[serde(default = "zero")]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    #[serde(default)]
    pub curve: f32,
    #[serde(default)]
    pub pickup: bool,
    #[serde(default)]
    pub macro_group: Option<String>,
}

/// A scheduled note is part of the project document, not transient engine
/// state. Keeping the primitive representation here lets every host surface
/// (GUI, CLI, history and reload) use the same validation rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiNoteContract {
    pub track_id: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub start_sample: u64,
    pub length_samples: u64,
    /// Optional vocal lyric preserved by OpenUtau import. Ordinary MIDI
    /// notes keep this empty while sharing the same canonical timeline model.
    #[serde(default)]
    pub lyric: String,
    /// Optional per-note vocal articulation shared by OpenUtau and native
    /// vocal editors. Pitch offsets are stored in cents at normalized points.
    #[serde(default)]
    pub phoneme: String,
    #[serde(default)]
    pub pitch_curve_cents: Vec<i16>,
    #[serde(default)]
    pub vibrato_depth_cents: u16,
    #[serde(default)]
    pub portamento_samples: u32,
    #[serde(default = "default_note_probability")]
    pub probability: u8,
    #[serde(default = "default_note_repeat_count")]
    pub repeat_count: u16,
}

fn default_note_probability() -> u8 { 100 }
fn default_note_repeat_count() -> u16 { 1 }

/// Persisted logical sidechain edge. Runtime audio buffers are intentionally
/// not serialized; hydration recreates the owned buffer publication from the
/// source and destination track identities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidechainRouteContract {
    pub source_id: u32,
    pub destination_id: u32,
    pub plugin_index: u32,
    #[serde(default = "default_sidechain_tap_point")]
    pub tap_point: u32,
}

fn default_sidechain_tap_point() -> u32 { 2 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackRouteContract {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
}

/// A normal acyclic audio edge.  Unlike a feedback route, this participates
/// in the compiled graph and therefore carries the same stable track IDs and
/// gain through JSON persistence, history, and project hydration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioRouteContract {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
}

impl MidiNoteContract {
    pub fn validate(&self) -> Result<()> {
        if self.pitch > 127 || self.velocity == 0 || self.length_samples == 0 {
            bail!("invalid MIDI note fields");
        }
        if self.start_sample.checked_add(self.length_samples).is_none() {
            bail!("MIDI note range overflows sample position");
        }
        if self.phoneme.len() > 128 || self.phoneme.contains('\0') {
            bail!("MIDI note phoneme is invalid");
        }
        if self.pitch_curve_cents.len() > 256 {
            bail!("MIDI note pitch curve is too large");
        }
        if u64::from(self.portamento_samples) > self.length_samples {
            bail!("MIDI note portamento exceeds note length");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutomationPointContract {
    pub time: f64,
    pub value: f32,
    #[serde(default)]
    pub curve: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TempoEventContract {
    pub beat: f64,
    pub bpm: f64,
    #[serde(default)]
    pub ramp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeSignatureEventContract {
    pub beat: f64,
    pub numerator: u8,
    pub denominator: u8,
}

impl TimeSignatureEventContract {
    pub fn validate(&self) -> Result<()> {
        if !self.beat.is_finite() || self.beat < 0.0
            || !(1..=32).contains(&self.numerator)
            || !matches!(self.denominator, 1 | 2 | 4 | 8 | 16 | 32)
        {
            bail!("invalid time signature event");
        }
        Ok(())
    }
}

impl TempoEventContract {
    pub fn validate(&self) -> Result<()> {
        if !self.beat.is_finite() || self.beat < 0.0
            || !self.bpm.is_finite() || !(20.0..=300.0).contains(&self.bpm) {
            bail!("invalid tempo event");
        }
        Ok(())
    }
}

impl AutomationPointContract {
    pub fn validate(&self) -> Result<()> {
        if !self.time.is_finite() || self.time < 0.0 || self.time.fract() != 0.0
            || !self.value.is_finite() || !(0.0..=1.0).contains(&self.value)
            || !self.curve.is_finite() || !(-1.0..=1.0).contains(&self.curve) {
            bail!("invalid automation point");
        }
        Ok(())
    }
}

/// A persisted macro fan-out edge.  Macro values stay normalized (0..=1),
/// while the edge carries the target parameter range and curve so the same
/// mapping can be reconstructed after reload on any plugin format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MacroMappingContract {
    pub mapping_id: String,
    pub macro_index: u8,
    pub target_instance_id: String,
    pub target_parameter_id: String,
    #[serde(default = "zero")]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    #[serde(default)]
    pub curve: f32,
    #[serde(default)]
    pub invert: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WarpMarkerContract {
    pub marker_id: String,
    pub region_id: u32,
    pub source_sample: u64,
    pub timeline_sample: u64,
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
    #[serde(default)]
    pub pitch_semitones: f32,
    #[serde(default)]
    pub transient: bool,
    #[serde(default)]
    pub analysis_generation: u64,
    #[serde(default)]
    pub cache_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RenderTargetKind {
    Track,
    Bus,
    Master,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderTargetContract {
    pub target_id: String,
    pub kind: RenderTargetKind,
    pub source_id: u32,
    #[serde(default)]
    pub pre_fader: bool,
    #[serde(default)]
    pub include_inserts: bool,
    #[serde(default)]
    pub include_tail: bool,
    #[serde(default)]
    pub offline_generation: u64,
}

/// Persisted identity for a track freeze render. The file itself is an asset
/// and may be missing on another machine; the metadata lets the loader mark it
/// stale instead of silently using an unrelated replacement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FreezeArtifactContract {
    pub track_id: u32,
    pub project_generation: u64,
    pub audio_generation: u64,
    pub total_samples: u64,
    pub sample_rate: u32,
    pub path: String,
    pub content_checksum: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenUtauVocalContract {
    pub source_path: String,
    pub rendered_audio_path: String,
    #[serde(default)]
    pub singer: String,
    #[serde(default)]
    pub source_generation: u64,
    /// Content identities make a source/render pair auditable after the
    /// project is moved, re-rendered, or edited by an external vocal tool.
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub rendered_audio_hash: String,
    #[serde(default)]
    pub rendered_audio_bytes: u64,
    #[serde(default)]
    pub rendered_sample_rate: Option<u32>,
    #[serde(default)]
    pub rendered_channels: Option<u16>,
    #[serde(default)]
    pub rendered_frames: Option<u64>,
    #[serde(default)]
    pub source_note_count: u64,
    #[serde(default)]
    pub source_singers: Vec<String>,
    /// Normalized Aura tuning controls used to produce the rendered score.
    /// Keeping them beside the source/render hashes makes a vocal take
    /// reproducible after reload instead of treating tuning as UI-only state.
    #[serde(default = "default_tuning")]
    pub tuning: OpenUtauTuningContract,
}

/// Persisted Track Stack topology.  The stack is a control-plane grouping;
/// DSP routing remains owned by the native engine after hydration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackStackContract {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub member_track_ids: Vec<u32>,
    #[serde(default = "one")]
    pub master_gain: f32,
    #[serde(default)]
    pub collapsed: bool,
}

/// Persisted VCA control topology. Unlike an audio bus, a VCA multiplies
/// member track faders without creating an audio summing node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VcaGroupContract {
    pub id: u32,
    #[serde(default = "one")]
    pub gain: f32,
    #[serde(default)]
    pub track_ids: Vec<u32>,
}

impl VcaGroupContract {
    pub fn validate(&self) -> Result<()> {
        if self.id == 0 || !self.gain.is_finite() || !(0.0..=8.0).contains(&self.gain)
            || self.track_ids.iter().any(|id| *id == 0)
            || self.track_ids.windows(2).any(|ids| ids[0] == ids[1]) {
            bail!("invalid VCA group");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarkerContract {
    pub id: u32,
    pub label: String,
    pub beat: f64,
    #[serde(default)]
    pub color: String,
}

impl MarkerContract {
    pub fn validate(&self) -> Result<()> {
        if self.id == 0 || self.label.trim().is_empty() || self.label.len() > 128
            || self.label.contains('\0') || !self.beat.is_finite() || self.beat < 0.0
            || self.color.len() > 32 || self.color.contains('\0') {
            bail!("invalid arrangement marker");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenUtauTuningContract {
    #[serde(default = "default_tuning_value")]
    pub scoop: f32,
    #[serde(default = "default_tuning_value")]
    pub vibrato: f32,
    #[serde(default = "default_tuning_value")]
    pub dynamics: f32,
    #[serde(default = "default_tuning_value")]
    pub consonants: f32,
}

impl Default for OpenUtauTuningContract {
    fn default() -> Self {
        default_tuning()
    }
}

fn default_tuning_value() -> f32 { 0.5 }
fn default_tuning() -> OpenUtauTuningContract {
    OpenUtauTuningContract { scoop: 0.35, vibrato: 0.45, dynamics: 0.60, consonants: 0.50 }
}

/// A recorded take available to the non-destructive comp editor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompTakeContract {
    pub id: u32,
    pub name: String,
    pub start_sample: u64,
    pub end_sample: u64,
}

impl CompTakeContract {
    pub fn validate(&self) -> Result<()> {
        if self.id == 0 || self.name.trim().is_empty() || self.name.contains('\0')
            || self.name.len() > 128 || self.end_sample <= self.start_sample {
            bail!("invalid comp take metadata");
        }
        Ok(())
    }
}

/// One non-destructive section selected from a recorded take.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompSegmentContract {
    pub take_id: u32,
    pub start_sample: u64,
    pub length_samples: u64,
    #[serde(default)]
    pub crossfade_samples: u32,
}

impl CompSegmentContract {
    pub fn validate(&self) -> Result<()> {
        if self.take_id == 0 || self.length_samples == 0
            || self.crossfade_samples as u64 > self.length_samples
            || self.start_sample.checked_add(self.length_samples).is_none() {
            bail!("invalid comp segment bounds");
        }
        Ok(())
    }
}

fn zero() -> f32 {
    0.0
}
fn one() -> f32 {
    1.0
}
fn default_algorithm() -> String {
    "elastic".to_string()
}

pub fn validate_contracts(
    plugins: &[PluginInstanceContract],
    mappings: &[MidiLearnMappingContract],
    macro_mappings: &[MacroMappingContract],
    markers: &[WarpMarkerContract],
    targets: &[RenderTargetContract],
) -> Result<()> {
    let mut ids = HashSet::new();
    for plugin in plugins {
        if plugin.instance_id.trim().is_empty() || !ids.insert(&plugin.instance_id) {
            bail!("plugin instance ids must be non-empty and unique");
        }
        if plugin.plugin_id.trim().is_empty() || plugin.bundle_path.contains('\0') {
            bail!(
                "plugin instance {} has invalid identity",
                plugin.instance_id
            );
        }
        if plugin.component_ids.len() > 64
            || plugin.component_ids.iter().any(|component| {
                component.trim().is_empty() || component.len() > 256 || component.contains('\0')
            })
            || plugin.input_channels > 256
            || plugin.output_channels > 256
            || plugin.sidechain_channels > 256
            || (plugin.sidechain_channels > 0 && plugin.output_channels == 0)
        {
            bail!("plugin instance {} has invalid bus layout", plugin.instance_id);
        }
        if !plugin.binary_hash.is_empty() && plugin.binary_hash.len() != 64 {
            bail!("plugin instance {} has invalid binary hash", plugin.instance_id);
        }
        if plugin.plugin_version.len() > 256 || plugin.architecture.len() > 32 ||
            plugin.plugin_version.contains('\0') || plugin.architecture.contains('\0') {
            bail!("plugin instance {} has invalid binary identity", plugin.instance_id);
        }
        if plugin.state_schema_version > 1 {
            bail!("plugin instance {} has unsupported state schema", plugin.instance_id);
        }
        if plugin.state_blob.len() > 4 * 1024 * 1024 || plugin.gui_state.len() > 1024 * 1024 {
            bail!(
                "plugin instance {} exceeds state limits",
                plugin.instance_id
            );
        }
        if plugin.latency_samples > 16 * 1024 * 1024 {
            bail!("plugin instance {} has invalid latency", plugin.instance_id);
        }
        if plugin.parameter_values.len() > 65_536
            || (!plugin.parameter_ids.is_empty()
                && plugin.parameter_ids.len() != plugin.parameter_values.len())
            || plugin.parameter_values.iter().any(|value| {
                !value.is_finite() || !(0.0..=1.0).contains(value)
            })
        {
            bail!("plugin instance {} has invalid parameter state", plugin.instance_id);
        }
        let mut parameter_ids = HashSet::with_capacity(plugin.parameter_ids.len());
        if plugin.parameter_ids.iter().any(|parameter| {
            parameter.trim().is_empty()
                || parameter.len() > 256
                || parameter.contains('\0')
                || !parameter_ids.insert(parameter)
        }) {
            bail!("plugin instance {} has duplicate or invalid parameter ids", plugin.instance_id);
        }
    }

    let mut macro_ids = HashSet::new();
    for mapping in macro_mappings {
        if mapping.mapping_id.trim().is_empty() || !macro_ids.insert(&mapping.mapping_id) {
            bail!("macro mapping ids must be non-empty and unique");
        }
        let Some(target_plugin) = plugins
            .iter()
            .find(|plugin| plugin.instance_id == mapping.target_instance_id)
        else {
            bail!("macro mapping {} targets an unknown plugin", mapping.mapping_id);
        };
        if mapping.macro_index >= 128
            || mapping.target_parameter_id.trim().is_empty()
            || (!target_plugin.parameter_ids.is_empty()
                && !target_plugin
                    .parameter_ids
                    .iter()
                    .any(|parameter| parameter == &mapping.target_parameter_id))
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
            || !(-1.0..=1.0).contains(&mapping.curve)
        {
            bail!("macro mapping {} is invalid", mapping.mapping_id);
        }
    }

    let mut mapping_ids = HashSet::new();
    for mapping in mappings {
        if mapping.mapping_id.trim().is_empty() || !mapping_ids.insert(&mapping.mapping_id) {
            bail!("MIDI mapping ids must be non-empty and unique");
        }
        if mapping.device_id.trim().is_empty()
            || mapping.target_instance_id.trim().is_empty()
            || mapping.target_parameter_id.trim().is_empty()
            || mapping.channel > 15
            || mapping.controller > 16_383
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
        {
            bail!("MIDI mapping {} is invalid", mapping.mapping_id);
        }
    }

    let mut marker_ids = HashSet::new();
    for marker in markers {
        if marker.marker_id.trim().is_empty() || !marker_ids.insert(&marker.marker_id) {
            bail!("warp marker ids must be non-empty and unique");
        }
        if !marker.pitch_semitones.is_finite() || !(-48.0..=48.0).contains(&marker.pitch_semitones)
        {
            bail!("warp marker {} has invalid pitch", marker.marker_id);
        }
    }

    let mut target_ids = HashSet::new();
    for target in targets {
        if target.target_id.trim().is_empty() || !target_ids.insert(&target.target_id) {
            bail!("render target ids must be non-empty and unique");
        }
        if target.offline_generation == 0 {
            bail!(
                "render target {} has no offline generation",
                target.target_id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_reject_duplicate_plugin_ids_and_bad_midi_ranges() {
        let plugin = PluginInstanceContract {
            instance_id: "p1".into(),
            track_id: 1,
            slot_index: 0,
            format: PluginFormat::Clap,
            bundle_path: "plugin.clap".into(),
            plugin_id: "gain".into(),
            bus_layout: vec![],
            component_ids: vec![],
            input_channels: 0,
            output_channels: 2,
            sidechain_channels: 0,
            parameter_ids: vec![],
            parameter_values: vec![],
            latency_samples: 0,
            state_blob: vec![],
            gui_state: vec![],
            bypassed: false,
            offline: false,
            quarantined: false,
            binary_hash: String::new(),
            capability: String::new(),
            plugin_version: String::new(),
            architecture: String::new(),
            state_schema_version: 1,
            state_generation: 0,
        };
        assert!(validate_contracts(&[plugin.clone(), plugin], &[], &[], &[], &[]).is_err());
        let mapping = MidiLearnMappingContract {
            mapping_id: "m1".into(),
            device_id: "dev".into(),
            channel: 16,
            controller: 1,
            target_instance_id: "p1".into(),
            target_parameter_id: "gain".into(),
            min: 0.0,
            max: 1.0,
            curve: 0.0,
            pickup: false,
            macro_group: None,
        };
        assert!(validate_contracts(&[], &[mapping], &[], &[], &[]).is_err());
    }

    #[test]
    fn plugin_bus_layout_supports_components_and_sidechain() {
        let plugin = PluginInstanceContract {
            instance_id: "p-layout".into(),
            track_id: 1,
            slot_index: 0,
            format: PluginFormat::Vst3,
            bundle_path: "processor.vst3".into(),
            plugin_id: "vendor.processor".into(),
            bus_layout: vec!["stereo".into(), "sidechain".into()],
            component_ids: vec!["processor".into(), "editor".into()],
            input_channels: 2,
            output_channels: 2,
            sidechain_channels: 2,
            parameter_ids: vec![],
            parameter_values: vec![],
            latency_samples: 128,
            state_blob: vec![],
            gui_state: vec![],
            bypassed: false,
            offline: false,
            quarantined: false,
            binary_hash: String::new(),
            capability: "sandbox".into(),
            plugin_version: "1.0.0".into(),
            architecture: "arm64".into(),
            state_schema_version: 1,
            state_generation: 1,
        };
        assert!(validate_contracts(&[plugin.clone()], &[], &[], &[], &[]).is_ok());

        let mut invalid = plugin;
        invalid.output_channels = 0;
        assert!(validate_contracts(&[invalid], &[], &[], &[], &[]).is_err());
    }

    #[test]
    fn contracts_round_trip_through_json() {
        let target = RenderTargetContract {
            target_id: "master".into(),
            kind: RenderTargetKind::Master,
            source_id: 1,
            pre_fader: false,
            include_inserts: true,
            include_tail: true,
            offline_generation: 1,
        };
        let json = serde_json::to_string(&target).unwrap();
        assert_eq!(
            serde_json::from_str::<RenderTargetContract>(&json).unwrap(),
            target
        );
    }

    #[test]
    fn macro_mapping_requires_existing_plugin_and_round_trips() {
        let plugin = PluginInstanceContract {
            instance_id: "track:1:slot:0".into(),
            track_id: 1,
            slot_index: 0,
            format: PluginFormat::BuiltIn,
            bundle_path: "builtin://0".into(),
            plugin_id: "builtin:0".into(),
            bus_layout: vec![],
            component_ids: vec![],
            input_channels: 0,
            output_channels: 2,
            sidechain_channels: 0,
            parameter_ids: vec!["12".into()],
            parameter_values: vec![0.5],
            latency_samples: 0,
            state_blob: vec![],
            gui_state: vec![],
            bypassed: false,
            offline: false,
            quarantined: false,
            binary_hash: String::new(),
            capability: "builtin".into(),
            plugin_version: String::new(),
            architecture: String::new(),
            state_schema_version: 1,
            state_generation: 0,
        };
        let mapping = MacroMappingContract {
            mapping_id: "macro-cutoff".into(),
            macro_index: 2,
            target_instance_id: "track:1:slot:0".into(),
            target_parameter_id: "12".into(),
            min: 0.1,
            max: 0.9,
            curve: 0.25,
            invert: false,
        };
        validate_contracts(&[plugin.clone()], &[], &[mapping.clone()], &[], &[]).unwrap();
        let json = serde_json::to_string(&mapping).unwrap();
        assert_eq!(serde_json::from_str::<MacroMappingContract>(&json).unwrap(), mapping);
        let mut missing = mapping;
        missing.target_instance_id = "track:9:slot:0".into();
        assert!(validate_contracts(&[plugin], &[], &[missing], &[], &[]).is_err());
    }
}
