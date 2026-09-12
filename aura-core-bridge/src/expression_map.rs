use serde::{Deserialize, Serialize};

include!("expression_map_basic.rs");

include!("expression_map_model.rs");

include!("expression_map_dynamics.rs");

#[derive(Clone, Debug, Default)]
pub struct ExpressionMapRuntime {
    directions: std::collections::BTreeMap<u8, String>,
    attributes: std::collections::BTreeSet<String>,
    latched_remote: Option<String>,
    active_slot: Option<String>,
    active_add_ons: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExpressionLaneEvent {
    Articulation(String),
    ResetGroup(u8),
    ResetAllDirections,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionSlotTransition {
    pub from_slot: Option<String>,
    pub to_slot: String,
    pub off_outputs: Vec<MidiOutput>,
    pub on_outputs: Vec<MidiOutput>,
    /// Negative means the note must be scheduled early.
    pub note_start_offset_ticks: i32,
    pub switch_offset_ticks: i32,
    pub note_length_ticks: Option<u32>,
}

/// Host-extracted VST 3 key-switch metadata. The plug-in adapter owns the VST
/// query; this transport-neutral value can safely cross the core boundary.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VstKeySwitchInfo {
    pub name: String,
    pub note: u8,
    pub velocity: u8,
    pub length_ticks: u32,
}

include!("expression_map_pro.rs");

include!("expression_map_runtime.rs");

include!("expression_map_validation.rs");

include!("expression_map_persistence_tests.rs");

include!("expression_map_sound_slot.rs");

include!("expression_map_pro_tests.rs");

include!("expression_map_remote_tests.rs");
