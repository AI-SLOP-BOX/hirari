use serde::{Deserialize, Serialize};

include!("control_room_state.rs");

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListenMode {
    PreFader,
    AfterFader,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportActivity {
    Stopped,
    Playback,
    Recording,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AutoDisableTalkback {
    Never,
    Recording,
    PlaybackAndRecording,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorSource {
    pub id: u32,
    pub name: String,
    pub channels: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorDestination {
    pub id: u32,
    pub name: String,
    pub channels: u8,
    pub device_ports: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CueSource {
    #[default]
    Mix,
    External(u32),
    CueSends,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CueMix {
    pub id: u32,
    pub name: String,
    pub gain_db: f32,
    pub talkback_send_db: f32,
    pub dim_during_talkback: bool,
    #[serde(default = "default_true")]
    pub talkback_enabled: bool,
    #[serde(default)]
    pub source: CueSource,
    #[serde(default)]
    pub click_enabled: bool,
    #[serde(default)]
    pub click_level_db: f32,
    #[serde(default)]
    pub click_pan: f32,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpeakerConfiguration {
    Mono,
    Stereo,
    Lcr,
    Quad,
    Surround51,
    Surround71,
}

impl SpeakerConfiguration {
    pub fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Lcr => 3,
            Self::Quad => 4,
            Self::Surround51 => 6,
            Self::Surround71 => 8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DownmixPreset {
    pub id: u32,
    pub name: String,
    pub monitor_id: u32,
    pub source_channels: u8,
    pub output: SpeakerConfiguration,
    /// Row-major output-by-input linear gain matrix.
    pub coefficients: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PhonesSource {
    Mix,
    External(u32),
    Cue(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PhonesChannel {
    pub enabled: bool,
    pub device_ports: [String; 2],
    pub source: PhonesSource,
    pub level_db: f32,
    pub click_enabled: bool,
    pub click_level_db: f32,
    pub click_pan: f32,
    pub listen_enabled: bool,
    pub listen_level_db: f32,
    pub use_as_preview: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MonitorCalibration {
    pub monitor_id: u32,
    pub input_gain_db: f32,
    pub phase_inverted: Vec<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MonitorChannelControls {
    pub calibrations: std::collections::BTreeMap<u32, MonitorCalibration>,
    pub soloed_speakers: std::collections::BTreeSet<u8>,
    pub solo_to_center: bool,
    pub surround_to_front: bool,
}

impl MonitorChannelControls {
    pub fn to_json(&self, room: &ControlRoomConsole) -> Result<String, String> {
        if !self.validate(room) {
            return Err("invalid monitor channel controls".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str, room: &ControlRoomConsole) -> Result<Self, String> {
        let controls: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if controls.validate(room) {
            Ok(controls)
        } else {
            Err("invalid monitor channel controls".into())
        }
    }

    pub fn set_calibration(
        &mut self,
        room: &ControlRoomConsole,
        calibration: MonitorCalibration,
    ) -> bool {
        if !calibration.validate(room) {
            return false;
        }
        self.calibrations
            .insert(calibration.monitor_id, calibration);
        true
    }

    pub fn set_speaker_solo(&mut self, room: &ControlRoomConsole, speaker: u8, solo: bool) -> bool {
        let Some(monitor) = room
            .monitors
            .iter()
            .find(|monitor| monitor.id == room.active_monitor)
        else {
            return false;
        };
        if speaker >= monitor.channels {
            return false;
        }
        if solo {
            self.soloed_speakers.insert(speaker);
        } else if !self.soloed_speakers.remove(&speaker) {
            return false;
        }
        true
    }

    pub fn clear_speaker_solos(&mut self) {
        self.soloed_speakers.clear();
    }

    /// Apply monitor calibration and speaker-check routing to interleaved PCM.
    pub fn process(
        &self,
        room: &ControlRoomConsole,
        interleaved: &[f32],
    ) -> Result<Vec<f32>, String> {
        if !self.validate(room) {
            return Err("invalid monitor channel controls".into());
        }
        let monitor = room
            .monitors
            .iter()
            .find(|monitor| monitor.id == room.active_monitor)
            .ok_or_else(|| "active monitor is missing".to_owned())?;
        let channels = monitor.channels as usize;
        if !interleaved.len().is_multiple_of(channels)
            || interleaved.iter().any(|sample| !sample.is_finite())
        {
            return Err("invalid monitor audio".into());
        }
        let calibration = self.calibrations.get(&monitor.id);
        let gain = calibration
            .map(|item| 10.0f32.powf(item.input_gain_db / 20.0))
            .unwrap_or(1.0);
        let mut output = Vec::with_capacity(interleaved.len());
        for frame in interleaved.chunks_exact(channels) {
            let calibrated = frame
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let phase = if calibration.is_some_and(|item| item.phase_inverted[index]) {
                        -1.0
                    } else {
                        1.0
                    };
                    sample * gain * phase
                })
                .collect::<Vec<_>>();
            if self.soloed_speakers.is_empty() {
                output.extend(calibrated);
                continue;
            }
            let mut routed = vec![0.0f32; channels];
            if self.solo_to_center {
                let signal: f32 = self
                    .soloed_speakers
                    .iter()
                    .map(|index| calibrated[*index as usize])
                    .sum();
                if channels >= 3 {
                    routed[2] = signal;
                } else if channels == 2 {
                    routed[0] = signal * 0.5;
                    routed[1] = signal * 0.5;
                } else {
                    routed[0] = signal;
                }
            } else {
                for index in &self.soloed_speakers {
                    let index = *index as usize;
                    if self.surround_to_front && index >= 3 && channels >= 2 {
                        routed[(index - 3) % 2] += calibrated[index];
                    } else {
                        routed[index] = calibrated[index];
                    }
                }
            }
            output.extend(routed);
        }
        Ok(output)
    }

    pub fn validate(&self, room: &ControlRoomConsole) -> bool {
        self.calibrations.len() <= 4
            && self.calibrations.iter().all(|(id, calibration)| {
                *id == calibration.monitor_id && calibration.validate(room)
            })
            && room
                .monitors
                .iter()
                .find(|monitor| monitor.id == room.active_monitor)
                .is_some_and(|monitor| {
                    self.soloed_speakers
                        .iter()
                        .all(|speaker| *speaker < monitor.channels)
                })
    }
}

impl MonitorCalibration {
    fn validate(&self, room: &ControlRoomConsole) -> bool {
        self.input_gain_db.is_finite()
            && (-24.0..=24.0).contains(&self.input_gain_db)
            && room
                .monitors
                .iter()
                .find(|monitor| monitor.id == self.monitor_id)
                .is_some_and(|monitor| self.phase_inverted.len() == monitor.channels as usize)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRoomConsole {
    pub enabled: bool,
    pub sources: Vec<MonitorSource>,
    pub active_source: u32,
    pub monitors: Vec<MonitorDestination>,
    pub active_monitor: u32,
    pub cues: Vec<CueMix>,
    pub downmix_presets: Vec<DownmixPreset>,
    pub active_downmix: Option<u32>,
    pub phones: Option<PhonesChannel>,
    pub control_level_db: f32,
    pub reference_level_db: f32,
    pub reference_level_active: bool,
    pub dim: bool,
    pub main_dim_db: f32,
    pub talkback: bool,
    pub talkback_momentary: bool,
    pub talkback_gain_db: f32,
    pub talkback_dim_db: f32,
    pub auto_disable_talkback: AutoDisableTalkback,
    pub transport: TransportActivity,
    pub listen_mode: ListenMode,
    pub listen_level_db: f32,
    pub listen_dim_db: f32,
    pub listen_channels: Vec<u32>,
    pub exclusive_monitor_ports: bool,
}

impl Default for ControlRoomConsole {
    fn default() -> Self {
        Self {
            enabled: true,
            sources: vec![MonitorSource {
                id: 1,
                name: "Main Mix".into(),
                channels: 2,
            }],
            active_source: 1,
            monitors: vec![MonitorDestination {
                id: 1,
                name: "Main".into(),
                channels: 2,
                device_ports: vec!["Out 1".into(), "Out 2".into()],
            }],
            active_monitor: 1,
            cues: Vec::new(),
            downmix_presets: Vec::new(),
            active_downmix: None,
            phones: None,
            control_level_db: 0.0,
            reference_level_db: -20.0,
            reference_level_active: false,
            dim: false,
            main_dim_db: -20.0,
            talkback: false,
            talkback_momentary: false,
            talkback_gain_db: 0.0,
            talkback_dim_db: -20.0,
            auto_disable_talkback: AutoDisableTalkback::Never,
            transport: TransportActivity::Stopped,
            listen_mode: ListenMode::AfterFader,
            listen_level_db: 0.0,
            listen_dim_db: -20.0,
            listen_channels: Vec::new(),
            exclusive_monitor_ports: true,
        }
    }
}

include!("control_room_console.rs");

include!("control_room_models.rs");

include!("control_room_tests.rs");
