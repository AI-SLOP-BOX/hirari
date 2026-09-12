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

fn default_note_probability() -> u8 {
    100
}
fn default_note_repeat_count() -> u16 {
    1
}

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

fn default_sidechain_tap_point() -> u32 {
    2
}

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
        if !self.beat.is_finite()
            || self.beat < 0.0
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
        if !self.beat.is_finite()
            || self.beat < 0.0
            || !self.bpm.is_finite()
            || !(20.0..=300.0).contains(&self.bpm)
        {
            bail!("invalid tempo event");
        }
        Ok(())
    }
}

impl AutomationPointContract {
    pub fn validate(&self) -> Result<()> {
        if !self.time.is_finite()
            || self.time < 0.0
            || self.time.fract() != 0.0
            || !self.value.is_finite()
            || !(0.0..=1.0).contains(&self.value)
            || !self.curve.is_finite()
            || !(-1.0..=1.0).contains(&self.curve)
        {
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
        if self.id == 0
            || !self.gain.is_finite()
            || !(0.0..=8.0).contains(&self.gain)
            || self.track_ids.contains(&0)
            || self.track_ids.windows(2).any(|ids| ids[0] == ids[1])
        {
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
        if self.id == 0
            || self.label.trim().is_empty()
            || self.label.len() > 128
            || self.label.contains('\0')
            || !self.beat.is_finite()
            || self.beat < 0.0
            || self.color.len() > 32
            || self.color.contains('\0')
        {
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

fn default_tuning_value() -> f32 {
    0.5
}
fn default_tuning() -> OpenUtauTuningContract {
    OpenUtauTuningContract {
        scoop: 0.35,
        vibrato: 0.45,
        dynamics: 0.60,
        consonants: 0.50,
    }
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
        if self.id == 0
            || self.name.trim().is_empty()
            || self.name.contains('\0')
            || self.name.len() > 128
            || self.end_sample <= self.start_sample
        {
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
        if self.take_id == 0
            || self.length_samples == 0
            || self.crossfade_samples as u64 > self.length_samples
            || self.start_sample.checked_add(self.length_samples).is_none()
        {
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

include!("project_contracts_validation.rs");

#[cfg(test)]
#[path = "project_contracts_tests.rs"]
mod project_contracts_tests;
