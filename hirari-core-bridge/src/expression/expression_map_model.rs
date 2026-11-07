#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArticulationRole {
    Direction,
    Attribute,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MidiOutput {
    KeySwitch {
        note: u8,
        velocity: u8,
        length_ticks: u32,
    },
    ProgramChange {
        bank_msb: Option<u8>,
        bank_lsb: Option<u8>,
        program: u8,
    },
    ControlChange {
        controller: u8,
        value: u8,
    },
    ChannelPressure {
        value: u8,
    },
    PitchBend {
        value: i16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteTrigger {
    Key { note: u8 },
    Program { program: u8 },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteTriggerMode {
    #[default]
    KeySwitch,
    ProgramChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupMove {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSlotMove {
    Up,
    Down,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProArticulation {
    pub id: String,
    pub name: String,
    pub role: ArticulationRole,
    pub group: u8,
    pub playback_technique: String,
    pub alias_for: Option<String>,
    pub fallback: Option<String>,
    pub remote_trigger: Option<RemoteTrigger>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SoundSlot {
    pub id: String,
    pub name: String,
    pub articulation_ids: Vec<String>,
    pub outputs: Vec<MidiOutput>,
    #[serde(default)]
    pub off_outputs: Vec<MidiOutput>,
    pub channel: Option<u8>,
    pub transpose: i8,
    pub velocity_scale: f32,
    pub pitch_range: Option<(u8, u8)>,
    pub velocity_range: Option<(u8, u8)>,
    #[serde(default)]
    pub add_on: bool,
    #[serde(default)]
    pub note_length_ticks: Option<u32>,
    #[serde(default)]
    pub attack_compensation_ticks: u32,
    #[serde(default)]
    pub separation_ticks: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ExpressionMapPro {
    pub name: String,
    pub articulations: Vec<ProArticulation>,
    pub sound_slots: Vec<SoundSlot>,
    pub default_slot: Option<String>,
    #[serde(default)]
    pub remote_trigger_mode: RemoteTriggerMode,
    pub latch_remote_triggers: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedExpressionNote {
    pub note: u8,
    pub velocity: u8,
    pub channel: u8,
    pub slot_id: String,
    pub add_on_slot_ids: Vec<String>,
    pub outputs: Vec<MidiOutput>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DynamicSymbol {
    Pppp,
    Ppp,
    Pp,
    P,
    Mp,
    Mf,
    F,
    Ff,
    Fff,
    Ffff,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DynamicRange {
    PpToFf,
    PpppToFfff,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DynamicVolumeOutput {
    Off,
    MainVolumeCc7,
    ExpressionCc11,
    Vst3Volume,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DynamicMappingEntry {
    pub symbol: DynamicSymbol,
    pub velocity_percent: f32,
    pub volume_value: u8,
    pub controller_value: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DynamicsMap {
    pub range: DynamicRange,
    pub change_velocities: bool,
    pub volume_output: DynamicVolumeOutput,
    pub send_controller: Option<u8>,
    pub entries: Vec<DynamicMappingEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedDynamics {
    pub velocity: u8,
    pub midi_outputs: Vec<MidiOutput>,
    pub vst3_volume: Option<f32>,
}
