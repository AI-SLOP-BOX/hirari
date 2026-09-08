use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRoomCueState {
    pub id: u32,
    pub gain: f32,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRoomState {
    pub monitor_outputs: Vec<String>,
    #[serde(default = "default_monitor_output_gains")]
    pub monitor_output_gains: Vec<f32>,
    #[serde(default = "default_monitor_output_enabled")]
    pub monitor_output_enabled: Vec<bool>,
    pub active_output: usize,
    pub dim: bool,
    pub dim_db: f32,
    pub talkback: bool,
    pub talkback_gain: f32,
    pub cue_gain_db: f32,
    pub reference_track: Option<String>,
    pub reference_enabled: bool,
    #[serde(default)]
    pub cues: Vec<ControlRoomCueState>,
}
impl Default for ControlRoomState {
    fn default() -> Self {
        Self {
            monitor_outputs: vec!["Main".into()],
            monitor_output_gains: vec![1.0],
            monitor_output_enabled: vec![true],
            active_output: 0,
            dim: false,
            dim_db: -20.0,
            talkback: false,
            talkback_gain: 1.0,
            cue_gain_db: 0.0,
            reference_track: None,
            reference_enabled: false,
            cues: Vec::new(),
        }
    }
}
fn default_monitor_output_gains() -> Vec<f32> { vec![1.0] }
fn default_monitor_output_enabled() -> Vec<bool> { vec![true] }
impl ControlRoomState {
    pub fn validate(&self) -> bool {
        !self.monitor_outputs.is_empty()
            && self.monitor_outputs.len() <= 16
            && self.monitor_output_gains.len() == self.monitor_outputs.len()
            && self.monitor_output_enabled.len() == self.monitor_outputs.len()
            && self.monitor_output_gains.iter().all(|gain| gain.is_finite() && (0.0..=4.0).contains(gain))
            && self
                .monitor_outputs
                .iter()
                .all(|o| !o.trim().is_empty() && o.len() <= 128 && !o.contains('\0'))
            && self.monitor_outputs.iter().enumerate().all(|(i, o)| {
                self.monitor_outputs[..i]
                    .iter()
                    .all(|p| !p.trim().eq_ignore_ascii_case(o.trim()))
            })
            && self.active_output < self.monitor_outputs.len()
            && self.dim_db.is_finite()
            && (-60.0..=0.0).contains(&self.dim_db)
            && self.talkback_gain.is_finite()
            && (0.0..=4.0).contains(&self.talkback_gain)
            && self.cue_gain_db.is_finite()
            && (-120.0..=24.0).contains(&self.cue_gain_db)
            && self
                .reference_track
                .as_ref()
                .map(|p| !p.trim().is_empty() && p.len() <= 4096 && !p.contains('\0'))
                .unwrap_or(true)
            && (!self.reference_enabled || self.reference_track.is_some())
    }
    pub fn select_output(&mut self, index: usize) -> bool {
        if index >= self.monitor_outputs.len() {
            false
        } else {
            self.active_output = index;
            true
        }
    }
    pub fn set_dim_db(&mut self, db: f32) -> bool {
        if !db.is_finite() || !(-60.0..=0.0).contains(&db) {
            return false;
        }
        self.dim_db = db;
        true
    }
    pub fn set_cue_gain_db(&mut self, db: f32) -> bool {
        if !db.is_finite() || !(-120.0..=24.0).contains(&db) {
            return false;
        }
        self.cue_gain_db = db;
        true
    }
    pub fn set_talkback_gain(&mut self, gain: f32) -> bool {
        if !gain.is_finite() || !(0.0..=4.0).contains(&gain) {
            return false;
        }
        self.talkback_gain = gain;
        true
    }
    pub fn add_monitor_output(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 128
            || name.contains('\0')
            || self.monitor_outputs.len() >= 16
            || self
                .monitor_outputs
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.monitor_outputs.push(name.to_owned());
        self.monitor_output_gains.push(1.0);
        self.monitor_output_enabled.push(true);
        true
    }
    pub fn rename_monitor_output(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        if index >= self.monitor_outputs.len()
            || name.is_empty()
            || name.len() > 128
            || name.contains('\0')
            || self
                .monitor_outputs
                .iter()
                .enumerate()
                .any(|(i, existing)| i != index && existing.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.monitor_outputs[index] = name.to_owned();
        true
    }
    pub fn remove_monitor_output(&mut self, index: usize) -> bool {
        if index >= self.monitor_outputs.len() || self.monitor_outputs.len() <= 1 {
            return false;
        }
        self.monitor_outputs.remove(index);
        self.monitor_output_gains.remove(index);
        self.monitor_output_enabled.remove(index);
        if self.active_output >= self.monitor_outputs.len() {
            self.active_output = self.monitor_outputs.len() - 1;
        }
        true
    }
    pub fn set_output_gain(&mut self, index: usize, gain: f32) -> bool {
        if !gain.is_finite() || !(0.0..=4.0).contains(&gain) { return false; }
        self.monitor_output_gains.get_mut(index).map(|value| *value = gain).is_some()
    }
    pub fn set_output_enabled(&mut self, index: usize, enabled: bool) -> bool {
        self.monitor_output_enabled.get_mut(index).map(|value| *value = enabled).is_some()
    }
    pub fn upsert_cue(&mut self, id: u32, gain: f32, enabled: bool) -> bool {
        if id == 0 || !gain.is_finite() || !(0.0..=4.0).contains(&gain) { return false; }
        if let Some(cue) = self.cues.iter_mut().find(|cue| cue.id == id) {
            cue.gain = gain; cue.enabled = enabled;
        } else {
            self.cues.push(ControlRoomCueState { id, gain, enabled });
        }
        true
    }
    pub fn set_reference_track(&mut self, path: Option<String>) -> bool {
        if path
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.len() > 4096 || p.contains('\0'))
        {
            return false;
        }
        self.reference_track = path.map(|p| p.trim().to_owned());
        self.reference_enabled = self.reference_track.is_some();
        true
    }
    pub fn set_reference_enabled(&mut self, enabled: bool) -> bool {
        if enabled && self.reference_track.is_none() {
            return false;
        }
        self.reference_enabled = enabled;
        true
    }
    pub fn effective_monitor_gain(&self) -> f32 {
        if self.dim {
            10.0f32.powf(self.dim_db / 20.0)
        } else {
            1.0
        }
    }
    pub fn effective_talkback_gain(&self) -> f32 {
        if self.talkback {
            self.talkback_gain.clamp(0.0, 4.0)
        } else {
            0.0
        }
    }
    pub fn effective_cue_gain(&self) -> f32 {
        10.0f32.powf(self.cue_gain_db.clamp(-120.0, 24.0) / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switches_monitors_and_dims() {
        let mut s = ControlRoomState {
            monitor_outputs: vec!["Main".into(), "Nearfield".into()],
            ..Default::default()
        };
        assert!(s.select_output(1));
        assert!(s.set_reference_track(Some("ref.wav".into())));
        s.dim = true;
        assert!((s.effective_monitor_gain() - 0.1).abs() < 0.001);
        assert!(s.validate());
    }
    #[test]
    fn rejects_duplicate_monitor_outputs() {
        let s = ControlRoomState {
            monitor_outputs: vec!["Main".into(), "Main".into()],
            ..Default::default()
        };
        assert!(!s.validate());
    }
}

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

impl ControlRoomConsole {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() {
            return Err("invalid Control Room state".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.audit() {
            Ok(value)
        } else {
            Err("invalid Control Room state".into())
        }
    }

    pub fn select_source(&mut self, id: u32) -> bool {
        if !self.sources.iter().any(|source| source.id == id) {
            return false;
        }
        self.active_source = id;
        true
    }

    pub fn select_monitor(&mut self, id: u32) -> bool {
        if !self.monitors.iter().any(|monitor| monitor.id == id) {
            return false;
        }
        self.active_monitor = id;
        true
    }

    pub fn upsert_cue(&mut self, mut cue: CueMix) -> bool {
        cue.name = cue.name.trim().to_owned();
        if !cue.validate() {
            return false;
        }
        if let Some(existing) = self.cues.iter_mut().find(|item| item.id == cue.id) {
            *existing = cue;
        } else if self.cues.len() < 4 {
            self.cues.push(cue);
        } else {
            return false;
        }
        self.cues.sort_by_key(|item| item.id);
        true
    }

    pub fn upsert_downmix(&mut self, mut preset: DownmixPreset) -> bool {
        preset.name = preset.name.trim().to_owned();
        if !preset.validate()
            || !self.monitors.iter().any(|monitor| {
                monitor.id == preset.monitor_id
                    && monitor.channels as usize == preset.output.channels()
            })
        {
            return false;
        }
        if let Some(existing) = self
            .downmix_presets
            .iter_mut()
            .find(|item| item.id == preset.id)
        {
            *existing = preset;
        } else if self.downmix_presets.len() < 32 {
            self.downmix_presets.push(preset);
        } else {
            return false;
        }
        self.downmix_presets.sort_by_key(|item| item.id);
        true
    }

    pub fn select_downmix(&mut self, id: Option<u32>) -> bool {
        if let Some(id) = id {
            let Some(preset) = self.downmix_presets.iter().find(|item| item.id == id) else {
                return false;
            };
            if preset.monitor_id != self.active_monitor {
                return false;
            }
        }
        self.active_downmix = id;
        true
    }

    pub fn set_phones(&mut self, phones: Option<PhonesChannel>) -> bool {
        if phones
            .as_ref()
            .is_some_and(|channel| !channel.validate(self))
        {
            return false;
        }
        self.phones = phones;
        true
    }

    pub fn render_downmix(&self, interleaved: &[f32]) -> Result<Vec<f32>, String> {
        let Some(id) = self.active_downmix else {
            return Ok(interleaved.to_vec());
        };
        let preset = self
            .downmix_presets
            .iter()
            .find(|item| item.id == id)
            .ok_or("active downmix preset is missing")?;
        let inputs = preset.source_channels as usize;
        if !interleaved.len().is_multiple_of(inputs)
            || interleaved.iter().any(|sample| !sample.is_finite())
        {
            return Err("invalid interleaved source audio".into());
        }
        let outputs = preset.output.channels();
        let mut rendered = Vec::with_capacity(interleaved.len() / inputs * outputs);
        for frame in interleaved.chunks_exact(inputs) {
            for row in preset.coefficients.chunks_exact(inputs) {
                rendered.push(
                    frame
                        .iter()
                        .zip(row)
                        .map(|(sample, gain)| sample * gain)
                        .sum(),
                );
            }
        }
        Ok(rendered)
    }

    pub fn set_transport(&mut self, activity: TransportActivity) {
        self.transport = activity;
        if self.talkback_forbidden() {
            self.talkback = false;
            self.talkback_momentary = false;
        }
    }

    pub fn set_talkback(&mut self, enabled: bool, momentary: bool) -> bool {
        if enabled && self.talkback_forbidden() {
            return false;
        }
        self.talkback = enabled;
        self.talkback_momentary = enabled && momentary;
        true
    }

    /// Applies Control Room monitor level, DIM and talkback to a stereo block.
    /// Talkback is mixed after DIM, matching a dedicated monitor path rather
    /// than altering the project master bus.
    pub fn process_monitor_stereo(
        &self,
        main_l: &[f32],
        main_r: &[f32],
        talkback_l: Option<&[f32]>,
        talkback_r: Option<&[f32]>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.audit()
            || main_l.len() != main_r.len()
            || main_l.len() > 16_000_000
            || main_l
                .iter()
                .chain(main_r)
                .any(|sample| !sample.is_finite())
        {
            return None;
        }
        let talkback = match (talkback_l, talkback_r) {
            (Some(left), Some(right))
                if left.len() == main_l.len()
                    && right.len() == main_r.len()
                    && left.iter().chain(right).all(|sample| sample.is_finite()) =>
            {
                Some((left, right))
            }
            (None, None) => None,
            _ => return None,
        };
        let main_gain = 10.0f32.powf((self.effective_main_level_db() / 20.0).clamp(-120.0, 24.0));
        let talkback_gain = 10.0f32.powf((self.talkback_gain_db / 20.0).clamp(-120.0, 24.0));
        let mut left = Vec::with_capacity(main_l.len());
        let mut right = Vec::with_capacity(main_r.len());
        for index in 0..main_l.len() {
            let mut l = main_l[index] * main_gain;
            let mut r = main_r[index] * main_gain;
            if let Some((talk_l, talk_r)) =
                talkback.filter(|_| self.talkback || self.talkback_momentary)
            {
                l += talk_l[index] * talkback_gain;
                r += talk_r[index] * talkback_gain;
            }
            left.push(l.clamp(-16.0, 16.0));
            right.push(r.clamp(-16.0, 16.0));
        }
        Some((left, right))
    }

    pub fn release_momentary_talkback(&mut self) -> bool {
        if !self.talkback_momentary {
            return false;
        }
        self.talkback = false;
        self.talkback_momentary = false;
        true
    }

    pub fn set_listen(&mut self, channel_id: u32, enabled: bool) -> bool {
        if channel_id == 0 {
            return false;
        }
        if enabled {
            if !self.listen_channels.contains(&channel_id) {
                self.listen_channels.push(channel_id);
                self.listen_channels.sort_unstable();
            }
        } else {
            let before = self.listen_channels.len();
            self.listen_channels.retain(|id| *id != channel_id);
            if before == self.listen_channels.len() {
                return false;
            }
        }
        true
    }

    pub fn effective_main_level_db(&self) -> f32 {
        if !self.enabled {
            return -120.0;
        }
        let mut level = if self.reference_level_active {
            self.reference_level_db
        } else {
            self.control_level_db
        };
        if self.dim {
            level += self.main_dim_db;
        }
        if self.talkback {
            level += self.talkback_dim_db;
        }
        if !self.listen_channels.is_empty() {
            level += self.listen_dim_db;
        }
        level.clamp(-120.0, 24.0)
    }

    pub fn effective_listen_level_db(&self) -> Option<f32> {
        (!self.listen_channels.is_empty()).then_some(
            (self.effective_main_level_db() - self.listen_dim_db + self.listen_level_db)
                .clamp(-120.0, 24.0),
        )
    }

    pub fn effective_cue_level_db(&self, cue_id: u32) -> Option<f32> {
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled)?;
        let dim = if self.talkback && cue.dim_during_talkback {
            self.talkback_dim_db
        } else {
            0.0
        };
        Some((cue.gain_db + dim).clamp(-120.0, 24.0))
    }

    pub fn effective_talkback_send_db(&self, cue_id: u32) -> Option<f32> {
        if !self.talkback {
            return None;
        }
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled && cue.talkback_enabled)?;
        Some((self.talkback_gain_db + cue.talkback_send_db).clamp(-120.0, 24.0))
    }

    /// Equal-power stereo click gains for a cue channel.
    pub fn cue_click_gains(&self, cue_id: u32) -> Option<[f32; 2]> {
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled && cue.click_enabled)?;
        let level = 10.0f32.powf(cue.click_level_db / 20.0);
        let angle = (cue.click_pan + 1.0) * std::f32::consts::FRAC_PI_4;
        Some([level * angle.cos(), level * angle.sin()])
    }

    fn talkback_forbidden(&self) -> bool {
        matches!(
            (self.auto_disable_talkback, self.transport),
            (AutoDisableTalkback::Recording, TransportActivity::Recording)
                | (
                    AutoDisableTalkback::PlaybackAndRecording,
                    TransportActivity::Playback | TransportActivity::Recording
                )
        )
    }

    pub fn audit(&self) -> bool {
        if self.sources.is_empty()
            || self.sources.len() > 32
            || self.monitors.is_empty()
            || self.monitors.len() > 4
            || self.cues.len() > 4
            || self.downmix_presets.len() > 32
            || !valid_db(self.control_level_db)
            || !valid_db(self.reference_level_db)
            || !valid_reduction(self.main_dim_db)
            || !valid_db(self.talkback_gain_db)
            || !valid_reduction(self.talkback_dim_db)
            || !valid_db(self.listen_level_db)
            || !valid_reduction(self.listen_dim_db)
        {
            return false;
        }
        let valid_named = |id: u32, name: &str| {
            id != 0 && !name.trim().is_empty() && name.len() <= 128 && !name.contains('\0')
        };
        if !self.sources.iter().all(|source| {
            valid_named(source.id, &source.name) && (1..=16).contains(&source.channels)
        }) || !self.monitors.iter().all(|monitor| {
            valid_named(monitor.id, &monitor.name)
                && (1..=16).contains(&monitor.channels)
                && monitor.device_ports.len() == monitor.channels as usize
                && monitor
                    .device_ports
                    .iter()
                    .all(|port| !port.trim().is_empty() && !port.contains('\0'))
        }) || !self.cues.iter().all(|cue| {
            cue.validate()
                && match cue.source {
                    CueSource::Mix | CueSource::CueSends => true,
                    CueSource::External(id) => self.sources.iter().any(|source| source.id == id),
                }
        }) || !self.downmix_presets.iter().all(|preset| {
            preset.validate()
                && self.monitors.iter().any(|monitor| {
                    monitor.id == preset.monitor_id
                        && monitor.channels as usize == preset.output.channels()
                })
        }) || self
            .phones
            .as_ref()
            .is_some_and(|phones| !phones.validate(self))
        {
            return false;
        }
        let unique = |ids: Vec<u32>| {
            let mut copy = ids;
            copy.sort_unstable();
            copy.windows(2).all(|pair| pair[0] < pair[1])
        };
        if !unique(self.sources.iter().map(|item| item.id).collect())
            || !unique(self.monitors.iter().map(|item| item.id).collect())
            || !unique(self.cues.iter().map(|item| item.id).collect())
            || !unique(self.downmix_presets.iter().map(|item| item.id).collect())
            || !self
                .sources
                .iter()
                .any(|item| item.id == self.active_source)
            || !self
                .monitors
                .iter()
                .any(|item| item.id == self.active_monitor)
            || self.talkback_forbidden() && self.talkback
            || self
                .listen_channels
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.active_downmix.is_some_and(|id| {
                !self
                    .downmix_presets
                    .iter()
                    .any(|preset| preset.id == id && preset.monitor_id == self.active_monitor)
            })
        {
            return false;
        }
        if self.exclusive_monitor_ports {
            let mut ports = std::collections::BTreeSet::new();
            if !self
                .monitors
                .iter()
                .flat_map(|monitor| &monitor.device_ports)
                .all(|port| ports.insert(port.to_ascii_lowercase()))
            {
                return false;
            }
        }
        true
    }
}

impl CueMix {
    fn validate(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && valid_db(self.gain_db)
            && valid_db(self.talkback_send_db)
            && valid_db(self.click_level_db)
            && self.click_pan.is_finite()
            && (-1.0..=1.0).contains(&self.click_pan)
    }
}

impl DownmixPreset {
    fn validate(&self) -> bool {
        self.id != 0
            && self.monitor_id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && (1..=16).contains(&self.source_channels)
            && self.coefficients.len() == self.source_channels as usize * self.output.channels()
            && self
                .coefficients
                .iter()
                .all(|gain| gain.is_finite() && (-4.0..=4.0).contains(gain))
    }
}

impl PhonesChannel {
    fn validate(&self, room: &ControlRoomConsole) -> bool {
        self.device_ports
            .iter()
            .all(|port| !port.trim().is_empty() && port.len() <= 128 && !port.contains('\0'))
            && !self.device_ports[0].eq_ignore_ascii_case(&self.device_ports[1])
            && valid_db(self.level_db)
            && valid_db(self.click_level_db)
            && self.click_pan.is_finite()
            && (-1.0..=1.0).contains(&self.click_pan)
            && valid_db(self.listen_level_db)
            && match self.source {
                PhonesSource::Mix => true,
                PhonesSource::External(id) => room.sources.iter().any(|source| source.id == id),
                PhonesSource::Cue(id) => room.cues.iter().any(|cue| cue.id == id),
            }
    }
}

fn valid_db(value: f32) -> bool {
    value.is_finite() && (-120.0..=24.0).contains(&value)
}
fn valid_reduction(value: f32) -> bool {
    value.is_finite() && (-120.0..=0.0).contains(&value)
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod console_tests {
    use super::*;

    #[test]
    fn talkback_dims_cues_and_auto_disables_for_recording() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 1,
            name: "Artist".into(),
            gain_db: -3.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            enabled: true
        }));
        room.auto_disable_talkback = AutoDisableTalkback::Recording;
        assert!(room.set_talkback(true, false));
        assert_eq!(room.effective_cue_level_db(1), Some(-23.0));
        assert_eq!(room.effective_talkback_send_db(1), Some(-6.0));
        room.set_transport(TransportActivity::Recording);
        assert!(!room.talkback);
        assert!(!room.set_talkback(true, false));
        assert!(room.audit());
    }

    #[test]
    fn cue_channels_follow_cubase_four_channel_limit_and_validate_sources() {
        let mut room = ControlRoomConsole::default();
        room.sources.push(MonitorSource {
            id: 9,
            name: "Live Room".into(),
            channels: 2,
        });
        for id in 1..=4 {
            assert!(room.upsert_cue(CueMix {
                id,
                name: format!("Cue {id}"),
                gain_db: 0.0,
                talkback_send_db: -6.0,
                dim_during_talkback: true,
                talkback_enabled: true,
                source: if id == 1 {
                    CueSource::External(9)
                } else {
                    CueSource::CueSends
                },
                click_enabled: id == 1,
                click_level_db: -6.0,
                click_pan: 0.0,
                enabled: true,
            }));
        }
        let mut fifth = room.cues[0].clone();
        fifth.id = 5;
        fifth.name = "Cue 5".into();
        assert!(!room.upsert_cue(fifth));

        room.cues[0].source = CueSource::External(99);
        assert!(!room.audit());
    }

    #[test]
    fn cue_click_uses_equal_power_pan_and_talkback_can_be_disabled_per_cue() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 1,
            name: "Singer".into(),
            gain_db: 0.0,
            talkback_send_db: -3.0,
            dim_during_talkback: true,
            talkback_enabled: false,
            source: CueSource::Mix,
            click_enabled: true,
            click_level_db: 0.0,
            click_pan: 0.0,
            enabled: true,
        }));
        let gains = room.cue_click_gains(1).unwrap();
        assert!((gains[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
        assert!((gains[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
        assert!(room.set_talkback(true, false));
        assert_eq!(room.effective_talkback_send_db(1), None);
        room.cues[0].talkback_enabled = true;
        assert_eq!(room.effective_talkback_send_db(1), Some(-3.0));
    }

    #[test]
    fn monitor_processing_applies_level_and_talkback_on_separate_path() {
        let mut room = ControlRoomConsole::default();
        room.control_level_db = -6.0;
        assert!(room.set_talkback(true, false));
        let (left, right) = room
            .process_monitor_stereo(
                &[1.0, 1.0],
                &[1.0, 1.0],
                Some(&[0.5, 0.5]),
                Some(&[0.5, 0.5]),
            )
            .unwrap();
        assert!(left[0] > 0.0 && left[0] < 1.0);
        assert_eq!(left, right);
        assert!(room
            .process_monitor_stereo(&[1.0], &[1.0, 1.0], None, None)
            .is_none());
    }

    #[test]
    fn reference_dim_and_listen_bus_are_independent() {
        let mut room = ControlRoomConsole::default();
        room.reference_level_active = true;
        room.reference_level_db = -18.0;
        room.dim = true;
        room.main_dim_db = -12.0;
        room.listen_dim_db = -20.0;
        room.listen_level_db = 3.0;
        assert!(room.set_listen(7, true));
        assert_eq!(room.effective_main_level_db(), -50.0);
        assert_eq!(room.effective_listen_level_db(), Some(-27.0));
        assert!(room.audit());
    }

    #[test]
    fn rejects_shared_monitor_ports_when_exclusive() {
        let mut room = ControlRoomConsole::default();
        room.monitors.push(MonitorDestination {
            id: 2,
            name: "Nearfield".into(),
            channels: 2,
            device_ports: vec!["Out 1".into(), "Out 3".into()],
        });
        assert!(!room.audit());
        room.exclusive_monitor_ports = false;
        assert!(room.audit());
    }

    #[test]
    fn applies_monitor_specific_stereo_to_mono_downmix() {
        let mut room = ControlRoomConsole::default();
        room.monitors[0].channels = 1;
        room.monitors[0].device_ports = vec!["Mono Out".into()];
        assert!(room.upsert_downmix(DownmixPreset {
            id: 7,
            name: "Mono Check".into(),
            monitor_id: 1,
            source_channels: 2,
            output: SpeakerConfiguration::Mono,
            coefficients: vec![0.5, 0.5]
        }));
        assert!(room.select_downmix(Some(7)));
        assert_eq!(
            room.render_downmix(&[1.0, -1.0, 0.5, 0.5]).unwrap(),
            vec![0.0, 0.5]
        );
        assert!(room.audit());
    }

    #[test]
    fn phones_selects_cue_without_following_main_dim() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 3,
            name: "Band".into(),
            gain_db: -2.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: false,
            click_level_db: 0.0,
            click_pan: 0.0,
            enabled: true
        }));
        assert!(room.set_phones(Some(PhonesChannel {
            enabled: true,
            device_ports: ["HP L".into(), "HP R".into()],
            source: PhonesSource::Cue(3),
            level_db: -8.0,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            listen_enabled: true,
            listen_level_db: -3.0,
            use_as_preview: true
        })));
        room.dim = true;
        assert_eq!(room.phones.as_ref().unwrap().level_db, -8.0);
        assert!(room.audit());
    }

    #[test]
    fn rejects_invalid_downmix_shape_and_missing_phones_source() {
        let mut room = ControlRoomConsole::default();
        assert!(!room.upsert_downmix(DownmixPreset {
            id: 1,
            name: "Broken".into(),
            monitor_id: 1,
            source_channels: 6,
            output: SpeakerConfiguration::Stereo,
            coefficients: vec![1.0; 3]
        }));
        assert!(!room.set_phones(Some(PhonesChannel {
            enabled: true,
            device_ports: ["HP L".into(), "HP R".into()],
            source: PhonesSource::Cue(99),
            level_db: 0.0,
            click_enabled: false,
            click_level_db: 0.0,
            click_pan: 0.0,
            listen_enabled: false,
            listen_level_db: 0.0,
            use_as_preview: false
        })));
    }

    #[test]
    fn monitor_calibration_applies_gain_and_per_speaker_phase() {
        let room = ControlRoomConsole::default();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_calibration(
            &room,
            MonitorCalibration {
                monitor_id: 1,
                input_gain_db: 6.0206,
                phase_inverted: vec![false, true]
            }
        ));
        let output = controls.process(&room, &[0.25, 0.25]).unwrap();
        assert!((output[0] - 0.5).abs() < 0.001);
        assert!((output[1] + 0.5).abs() < 0.001);
        let json = controls.to_json(&room).unwrap();
        assert_eq!(
            MonitorChannelControls::from_json(&json, &room).unwrap(),
            controls
        );
    }

    #[test]
    fn speaker_solo_can_be_auditioned_on_stereo_center_fallback() {
        let room = ControlRoomConsole::default();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_speaker_solo(&room, 1, true));
        controls.solo_to_center = true;
        assert_eq!(
            controls.process(&room, &[0.2, 0.8]).unwrap(),
            vec![0.4, 0.4]
        );
        controls.clear_speaker_solos();
        assert_eq!(
            controls.process(&room, &[0.2, 0.8]).unwrap(),
            vec![0.2, 0.8]
        );
    }

    #[test]
    fn surround_solo_routes_rears_to_front_for_speaker_checks() {
        let mut room = ControlRoomConsole::default();
        room.monitors[0].channels = 6;
        room.monitors[0].device_ports = (1..=6).map(|index| format!("Out {index}")).collect();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_speaker_solo(&room, 4, true));
        controls.surround_to_front = true;
        assert_eq!(
            controls
                .process(&room, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
                .unwrap(),
            vec![0.0, 5.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert!(!controls.set_speaker_solo(&room, 6, true));
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn control_room_round_trips_and_rejects_invalid_port_conflicts() {
        let mut room = ControlRoomConsole::default();
        room.cues.push(CueMix {
            id: 1,
            name: "Artist".into(),
            gain_db: -3.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            enabled: true,
        });
        let json = room.to_json().unwrap();
        assert_eq!(ControlRoomConsole::from_json(&json).unwrap(), room);
        room.monitors.push(MonitorDestination {
            id: 2,
            name: "Nearfield".into(),
            channels: 2,
            device_ports: vec!["Out 1".into(), "Out 4".into()],
        });
        assert!(room.to_json().is_err());
    }
}
