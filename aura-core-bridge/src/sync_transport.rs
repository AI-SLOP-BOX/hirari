//! Deterministic external MIDI Clock/MTC/LTC transport conversion.

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SyncProtocol {
    MidiClock,
    Mtc,
    Ltc,
}
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SyncSource {
    Internal,
    MidiPort(u32),
    AudioDevice(u32),
    Network(u16),
}
impl SyncSource {
    pub fn validate(self) -> bool {
        match self {
            Self::Internal => true,
            Self::MidiPort(id) | Self::AudioDevice(id) => id != 0,
            Self::Network(port) => port != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmcCommand {
    Stop,
    Play,
    DeferredPlay,
    RecordPunchIn,
    RecordPunchOut,
    FastForward,
    Rewind,
}

/// Decodes a standard MIDI Machine Control universal-real-time packet.
pub fn decode_mmc(bytes: &[u8]) -> Option<MmcCommand> {
    // Universal real-time SysEx: F0 7F <device-id> 06 <command> F7.
    if bytes.len() != 6
        || bytes[0] != 0xf0
        || bytes[1] != 0x7f
        || bytes[2] > 0x7f
        || bytes[3] != 0x06
        || bytes[5] != 0xf7
    {
        return None;
    }
    match bytes[4] {
        0x01 => Some(MmcCommand::Stop),
        0x02 => Some(MmcCommand::Play),
        0x03 => Some(MmcCommand::DeferredPlay),
        0x05 => Some(MmcCommand::RecordPunchIn),
        0x06 => Some(MmcCommand::RecordPunchOut),
        0x04 => Some(MmcCommand::FastForward),
        0x09 => Some(MmcCommand::Rewind),
        _ => None,
    }
}

pub fn encode_mmc(device_id: u8, command: MmcCommand) -> Option<[u8; 6]> {
    if device_id > 0x7f {
        return None;
    }
    let code = match command {
        MmcCommand::Stop => 0x01,
        MmcCommand::Play => 0x02,
        MmcCommand::DeferredPlay => 0x03,
        MmcCommand::FastForward => 0x04,
        MmcCommand::RecordPunchIn => 0x05,
        MmcCommand::RecordPunchOut => 0x06,
        MmcCommand::Rewind => 0x09,
    };
    Some([0xf0, 0x7f, device_id, 0x06, code, 0xf7])
}

/// Encode the MMC Locate "target" command used when the project cursor moves
/// while external synchronization is active.
pub fn encode_mmc_locate(device_id: u8, timecode: Timecode, subframes: u8) -> Option<[u8; 13]> {
    if device_id > 0x7f || subframes > 99 || timecode.validate().is_err() {
        return None;
    }
    Some([
        0xf0,
        0x7f,
        device_id,
        0x06,
        0x44,
        0x06,
        0x01,
        timecode.hours,
        timecode.minutes,
        timecode.seconds,
        timecode.frames,
        subframes,
        0xf7,
    ])
}

pub fn decode_mmc_locate(bytes: &[u8]) -> Option<(u8, Timecode, u8)> {
    if bytes.len() != 13
        || bytes[0] != 0xf0
        || bytes[1] != 0x7f
        || bytes[2] > 0x7f
        || bytes[3..7] != [0x06, 0x44, 0x06, 0x01]
        || bytes[12] != 0xf7
        || bytes[11] > 99
    {
        return None;
    }
    // MMC Locate does not carry a frame-rate code. Validate the position
    // against the project/default rate supplied by this boundary (30 fps).
    let timecode = Timecode {
        hours: bytes[7],
        minutes: bytes[8],
        seconds: bytes[9],
        frames: bytes[10],
        fps: 30,
    };
    timecode.validate().ok()?;
    Some((bytes[2], timecode, bytes[11]))
}

/// Host-independent external transport state machine. Device adapters feed
/// MIDI clock ticks or MTC frames here; the UI can consume one coherent state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExternalSyncController {
    pub source: SyncSource,
    pub protocol: SyncProtocol,
    pub enabled: bool,
    pub running: bool,
    pub beat: f64,
    pub timecode: Option<Timecode>,
}
impl Default for ExternalSyncController {
    fn default() -> Self {
        Self {
            source: SyncSource::Internal,
            protocol: SyncProtocol::MidiClock,
            enabled: false,
            running: false,
            beat: 0.0,
            timecode: None,
        }
    }
}
impl ExternalSyncController {
    pub fn select_source(&mut self, source: SyncSource) {
        if source.validate() {
            self.source = source;
            self.running = false;
        }
    }
    pub fn configure(&mut self, protocol: SyncProtocol, enabled: bool) {
        self.protocol = protocol;
        self.enabled = enabled;
        if !enabled {
            self.running = false;
        }
    }
    pub fn start(&mut self) {
        if self.enabled {
            self.running = true;
        }
    }
    pub fn stop(&mut self) {
        self.running = false;
    }
    pub fn set_beat(&mut self, beat: f64) -> Result<(), &'static str> {
        if !beat.is_finite() || beat < 0.0 {
            return Err("invalid sync beat");
        }
        self.beat = beat;
        Ok(())
    }
    pub fn apply_mmc(&mut self, command: MmcCommand) -> bool {
        if !self.enabled {
            return false;
        }
        match command {
            MmcCommand::Stop | MmcCommand::RecordPunchOut => self.stop(),
            MmcCommand::Play | MmcCommand::DeferredPlay | MmcCommand::RecordPunchIn => self.start(),
            MmcCommand::FastForward => {
                self.running = true;
                self.beat = (self.beat + 4.0).min(f64::MAX);
            }
            MmcCommand::Rewind => {
                self.running = true;
                self.beat = (self.beat - 4.0).max(0.0);
            }
        }
        true
    }
    pub fn clock_tick(&mut self) {
        if self.enabled && self.protocol == SyncProtocol::MidiClock {
            self.running = true;
            self.beat = (self.beat + 1.0 / 24.0).min(f64::MAX);
        }
    }
    pub fn set_timecode(&mut self, tc: Timecode) -> Result<(), &'static str> {
        tc.validate()?;
        self.timecode = Some(tc);
        if self.enabled && self.protocol == SyncProtocol::Mtc {
            self.running = true;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fps: u8,
}

impl Timecode {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.hours > 23
            || self.minutes > 59
            || self.seconds > 59
            || self.fps == 0
            || self.frames >= self.fps
        {
            return Err("invalid timecode");
        }
        Ok(())
    }
    pub fn frame_index(&self) -> u64 {
        (((self.hours as u64 * 60 + self.minutes as u64) * 60 + self.seconds as u64)
            * self.fps as u64)
            + self.frames as u64
    }

    /// Returns the nominal 30-fps frame count for SMPTE 29.97 drop-frame
    /// timecode. The two skipped frame numbers at most minute boundaries are
    /// excluded; non-29.97 timecodes use the ordinary frame index.
    pub fn drop_frame_index(&self) -> Result<u64, &'static str> {
        self.validate()?;
        if self.fps != 30 {
            return Ok(self.frame_index());
        }
        let total_minutes = self.hours as u64 * 60 + self.minutes as u64;
        let dropped = 2 * (total_minutes - total_minutes / 10);
        Ok(self.frame_index().saturating_sub(dropped))
    }

    pub fn from_drop_frame_index(mut index: u64) -> Result<Self, &'static str> {
        let fps = 30u8;
        let frames_per_ten_minutes = 17_982u64;
        let ten_minute_blocks = index / frames_per_ten_minutes;
        index %= frames_per_ten_minutes;
        let mut minute = 0u64;
        for candidate in 0..10u64 {
            let capacity = if candidate == 0 || candidate == 9 {
                1_800
            } else {
                1_798
            };
            if index < capacity {
                minute = candidate;
                break;
            }
            index -= capacity;
        }
        let total_minutes = ten_minute_blocks * 10 + minute;
        let nominal_frame =
            ten_minute_blocks * 18_000 + minute * 1_800 + index + if minute > 0 { 2 } else { 0 };
        let hours = ((total_minutes / 60) % 24) as u8;
        let minutes = (total_minutes % 60) as u8;
        let seconds = ((nominal_frame / 30) % 60) as u8;
        let frames = (nominal_frame % 30) as u8;
        Self {
            hours,
            minutes,
            seconds,
            frames,
            fps,
        }
        .validate()?;
        Ok(Self {
            hours,
            minutes,
            seconds,
            frames,
            fps,
        })
    }

    pub fn from_frame_index(mut index: u64, fps: u8) -> Result<Self, &'static str> {
        if fps == 0 || fps > 120 {
            return Err("invalid timecode frame rate");
        }
        let frames = (index % fps as u64) as u8;
        index /= fps as u64;
        let seconds = (index % 60) as u8;
        index /= 60;
        let minutes = (index % 60) as u8;
        let hours = ((index / 60) % 24) as u8;
        Ok(Self {
            hours,
            minutes,
            seconds,
            frames,
            fps,
        })
    }

    /// Encodes the data byte of one MIDI Time Code quarter-frame message.
    /// Send it after status byte 0xF1; `part` is 0..=7.
    pub fn mtc_quarter_frame(&self, part: u8) -> Result<u8, &'static str> {
        self.validate()?;
        if part > 7 {
            return Err("invalid MTC quarter-frame part");
        }
        let nibble = match part {
            0 => self.frames & 0x0f,
            1 => self.frames >> 4,
            2 => self.seconds & 0x0f,
            3 => self.seconds >> 4,
            4 => self.minutes & 0x0f,
            5 => self.minutes >> 4,
            6 => self.hours & 0x0f,
            _ => (self.hours >> 4) | fps_code(self.fps) << 1,
        };
        Ok((part << 4) | nibble)
    }

    /// Universal real-time MTC Full Frame message for locate/chase setup.
    pub fn mtc_full_frame(&self, device_id: u8) -> Result<[u8; 10], &'static str> {
        self.validate()?;
        if device_id > 0x7f {
            return Err("invalid MTC device id");
        }
        let hours_and_rate = self.hours | (fps_code(self.fps) << 5);
        Ok([
            0xf0,
            0x7f,
            device_id,
            0x01,
            0x01,
            hours_and_rate,
            self.minutes,
            self.seconds,
            self.frames,
            0xf7,
        ])
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MtcDestination {
    pub port_id: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MtcOutputRouter {
    pub destinations: Vec<MtcDestination>,
    pub device_id: u8,
    pub follows_project_time: bool,
}

impl MtcOutputRouter {
    pub fn active_ports(&self) -> Vec<u32> {
        let mut ports = self
            .destinations
            .iter()
            .filter(|destination| destination.enabled)
            .map(|destination| destination.port_id)
            .collect::<Vec<_>>();
        ports.sort_unstable();
        ports
    }

    pub fn locate_messages(&self, timecode: Timecode) -> Result<Vec<(u32, Vec<u8>)>, &'static str> {
        if !self.validate() || !self.follows_project_time {
            return Err("MTC output is not configured to follow project time");
        }
        let message = timecode.mtc_full_frame(self.device_id)?.to_vec();
        Ok(self
            .active_ports()
            .into_iter()
            .map(|port| (port, message.clone()))
            .collect())
    }

    pub fn quarter_frame_messages(
        &self,
        timecode: Timecode,
        part: u8,
    ) -> Result<Vec<(u32, [u8; 2])>, &'static str> {
        if !self.validate() {
            return Err("invalid MTC output configuration");
        }
        let data = timecode.mtc_quarter_frame(part)?;
        Ok(self
            .active_ports()
            .into_iter()
            .map(|port| (port, [0xf1, data]))
            .collect())
    }

    pub fn validate(&self) -> bool {
        self.device_id <= 0x7f
            && self.destinations.len() <= 256
            && self
                .destinations
                .iter()
                .all(|destination| destination.port_id != 0)
            && self
                .destinations
                .iter()
                .enumerate()
                .all(|(index, destination)| {
                    self.destinations[..index]
                        .iter()
                        .all(|previous| previous.port_id != destination.port_id)
                })
    }
}

fn fps_code(fps: u8) -> u8 {
    match fps {
        24 => 0,
        25 => 1,
        29 | 30 => 2,
        _ => 3,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MtcDecoder {
    parts: [u8; 8],
    received: u8,
}

impl MtcDecoder {
    pub fn reset(&mut self) {
        self.parts = [0; 8];
        self.received = 0;
    }
    pub fn received_parts(&self) -> u8 {
        self.received.count_ones() as u8
    }

    pub fn push(&mut self, data_byte: u8) -> Result<Option<Timecode>, &'static str> {
        let part = data_byte >> 4;
        if part > 7 {
            return Err("invalid MTC quarter-frame part");
        }
        self.parts[part as usize] = data_byte & 0x0f;
        self.received |= 1 << part;
        if self.received != 0xff {
            return Ok(None);
        }
        let fps = match self.parts[7] >> 1 {
            0 => 24,
            1 => 25,
            2 => 30,
            _ => 30,
        };
        let timecode = Timecode {
            frames: self.parts[0] | ((self.parts[1] & 1) << 4),
            seconds: self.parts[2] | ((self.parts[3] & 3) << 4),
            minutes: self.parts[4] | ((self.parts[5] & 3) << 4),
            hours: self.parts[6] | ((self.parts[7] & 1) << 4),
            fps,
        };
        self.reset();
        timecode.validate()?;
        Ok(Some(timecode))
    }
}

pub fn midi_clock_ticks_to_beats(ticks: u32) -> f64 {
    ticks as f64 / 24.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_clock_and_validates_timecode() {
        assert_eq!(midi_clock_ticks_to_beats(48), 2.0);
        let tc = Timecode {
            hours: 1,
            minutes: 2,
            seconds: 3,
            frames: 12,
            fps: 30,
        };
        assert!(tc.validate().is_ok());
        assert_eq!(tc.frame_index(), 111702);
        assert_eq!(tc.drop_frame_index().unwrap(), 111590);
        assert!(Timecode { frames: 30, ..tc }.validate().is_err());
        let restored = Timecode::from_frame_index(tc.frame_index(), 30).unwrap();
        assert_eq!(restored, tc);
        assert_eq!(
            Timecode::from_drop_frame_index(tc.drop_frame_index().unwrap()).unwrap(),
            tc
        );
        assert_eq!(tc.mtc_quarter_frame(0).unwrap(), 12);
        assert!(tc.mtc_quarter_frame(8).is_err());
        let mut decoder = MtcDecoder::default();
        let mut decoded = None;
        for part in 0..8 {
            decoded = decoder.push(tc.mtc_quarter_frame(part).unwrap()).unwrap();
        }
        assert_eq!(decoded, Some(tc));
    }
    #[test]
    fn decodes_mmc_transport_commands() {
        assert_eq!(
            decode_mmc(&[0xf0, 0x7f, 0x7f, 0x06, 0x02, 0xf7]),
            Some(MmcCommand::Play)
        );
        assert_eq!(
            decode_mmc(&[0xf0, 0x7f, 0x7f, 0x06, 0x01, 0xf7]),
            Some(MmcCommand::Stop)
        );
        assert_eq!(decode_mmc(&[0xf0, 0x7f, 0x7f, 0x01, 0x02, 0xf7]), None);
        assert_eq!(decode_mmc(&[0xf0, 0x7f, 0x80, 0x06, 0x02, 0xf7]), None);
        assert_eq!(
            decode_mmc(&[0xf0, 0x7f, 0x7f, 0x06, 0x02, 0xf7, 0x00]),
            None
        );
    }

    #[test]
    fn mmc_locate_round_trips_project_cursor_position() {
        let timecode = Timecode {
            hours: 1,
            minutes: 2,
            seconds: 3,
            frames: 12,
            fps: 30,
        };
        let packet = encode_mmc_locate(7, timecode, 25).unwrap();
        assert_eq!(decode_mmc_locate(&packet), Some((7, timecode, 25)));
        assert!(encode_mmc_locate(128, timecode, 0).is_none());
        assert!(encode_mmc_locate(7, timecode, 100).is_none());
    }

    #[test]
    fn routes_mtc_full_frame_and_quarter_frames_to_enabled_ports() {
        let router = MtcOutputRouter {
            destinations: vec![
                MtcDestination {
                    port_id: 9,
                    enabled: true,
                },
                MtcDestination {
                    port_id: 3,
                    enabled: false,
                },
                MtcDestination {
                    port_id: 7,
                    enabled: true,
                },
            ],
            device_id: 0x7f,
            follows_project_time: true,
        };
        let timecode = Timecode {
            hours: 1,
            minutes: 2,
            seconds: 3,
            frames: 12,
            fps: 25,
        };
        let locate = router.locate_messages(timecode).unwrap();
        assert_eq!(
            locate.iter().map(|(port, _)| *port).collect::<Vec<_>>(),
            vec![7, 9]
        );
        assert!(locate
            .iter()
            .all(|(_, bytes)| bytes == &vec![0xf0, 0x7f, 0x7f, 0x01, 0x01, 0x21, 2, 3, 12, 0xf7]));
        assert_eq!(
            router.quarter_frame_messages(timecode, 0).unwrap(),
            vec![(7, [0xf1, 12]), (9, [0xf1, 12])]
        );
        assert!(router.quarter_frame_messages(timecode, 8).is_err());
        assert!(router.validate());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLockState {
    Idle,
    Acquiring,
    Locked,
    DroppedOut,
    Inhibited,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TimecodeLockPreferences {
    pub lock_frames: u16,
    pub drop_out_frames: u16,
    pub inhibit_restart_ms: u32,
    pub auto_detect_frame_rate: bool,
    pub project_fps: u8,
}

impl Default for TimecodeLockPreferences {
    fn default() -> Self {
        Self {
            lock_frames: 16,
            drop_out_frames: 10,
            inhibit_restart_ms: 500,
            auto_detect_frame_rate: false,
            project_fps: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimecodeLockEngine {
    pub preferences: TimecodeLockPreferences,
    pub state: SyncLockState,
    pub detected_fps: Option<u8>,
    pub current: Option<Timecode>,
    consecutive_frames: u16,
    missing_frames: u16,
    last_frame_index: Option<u64>,
    inhibit_until_ms: u64,
}

impl TimecodeLockEngine {
    pub fn new(preferences: TimecodeLockPreferences) -> Result<Self, &'static str> {
        if !preferences.validate() {
            return Err("invalid timecode lock preferences");
        }
        Ok(Self {
            preferences,
            state: SyncLockState::Idle,
            detected_fps: None,
            current: None,
            consecutive_frames: 0,
            missing_frames: 0,
            last_frame_index: None,
            inhibit_until_ms: 0,
        })
    }

    /// Feed one complete MTC frame. Lock is acquired only after the configured
    /// number of consecutive frames; discontinuities restart acquisition.
    pub fn ingest(
        &mut self,
        timecode: Timecode,
        now_ms: u64,
    ) -> Result<SyncLockState, &'static str> {
        timecode.validate()?;
        if now_ms < self.inhibit_until_ms {
            self.state = SyncLockState::Inhibited;
            return Ok(self.state);
        }
        if timecode.fps != self.preferences.project_fps {
            if self.preferences.auto_detect_frame_rate {
                self.detected_fps = Some(timecode.fps);
            } else {
                self.state = SyncLockState::Idle;
                return Err("incoming timecode frame rate mismatch");
            }
        } else {
            self.detected_fps = Some(timecode.fps);
        }
        let index = timecode.frame_index();
        let consecutive = self
            .last_frame_index
            .is_none_or(|previous| index == previous.saturating_add(1));
        if consecutive {
            self.consecutive_frames = self.consecutive_frames.saturating_add(1);
        } else {
            self.consecutive_frames = 1;
        }
        self.last_frame_index = Some(index);
        self.current = Some(timecode);
        self.missing_frames = 0;
        self.state = if self.consecutive_frames >= self.preferences.lock_frames {
            SyncLockState::Locked
        } else {
            SyncLockState::Acquiring
        };
        Ok(self.state)
    }

    /// Called by the host when expected frames did not arrive. Crossing the
    /// dropout threshold stops transport and inhibits immediate relocking.
    pub fn report_missing_frames(&mut self, frames: u16, now_ms: u64) -> SyncLockState {
        if self.state == SyncLockState::Idle {
            return self.state;
        }
        self.missing_frames = self.missing_frames.saturating_add(frames);
        if self.missing_frames > self.preferences.drop_out_frames {
            self.state = SyncLockState::DroppedOut;
            self.consecutive_frames = 0;
            self.last_frame_index = None;
            self.inhibit_until_ms =
                now_ms.saturating_add(u64::from(self.preferences.inhibit_restart_ms));
        }
        self.state
    }

    pub fn position_samples(&self, sample_rate: f64) -> Option<u64> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return None;
        }
        let timecode = self.current?;
        let samples = timecode.frame_index() as f64 * sample_rate / f64::from(timecode.fps);
        (samples.is_finite() && samples <= u64::MAX as f64).then_some(samples.round() as u64)
    }

    pub fn audit(&self) -> bool {
        self.preferences.validate()
            && self.detected_fps.is_none_or(valid_sync_fps)
            && self
                .current
                .is_none_or(|timecode| timecode.validate().is_ok())
    }
}

impl TimecodeLockPreferences {
    fn validate(self) -> bool {
        self.lock_frames > 0
            && self.lock_frames <= 10_000
            && self.drop_out_frames > 0
            && self.drop_out_frames <= 10_000
            && self.inhibit_restart_ms <= 60_000
            && valid_sync_fps(self.project_fps)
    }
}

fn valid_sync_fps(fps: u8) -> bool {
    matches!(fps, 24 | 25 | 29 | 30)
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiClockPreferences {
    pub follows_project_position: bool,
    pub always_send_start: bool,
    pub send_clock_in_stop: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiClockDestination {
    pub port_id: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SyncProjectSettings {
    pub source: SyncSource,
    pub protocol: SyncProtocol,
    pub enabled: bool,
    pub timecode: TimecodeLockPreferences,
    pub midi_clock: MidiClockPreferences,
    pub midi_destinations: Vec<MidiClockDestination>,
    pub mmc_device_id: u8,
}

impl SyncProjectSettings {
    pub fn validate(&self) -> bool {
        self.source.validate()
            && self.timecode.validate()
            && self.mmc_device_id <= 0x7f
            && self.midi_destinations.len() <= 256
            && self
                .midi_destinations
                .iter()
                .all(|destination| destination.port_id != 0)
            && self
                .midi_destinations
                .iter()
                .enumerate()
                .all(|(index, destination)| {
                    self.midi_destinations[..index]
                        .iter()
                        .all(|previous| previous.port_id != destination.port_id)
                })
            && (!self.enabled
                || self.source != SyncSource::Internal
                || self.protocol == SyncProtocol::MidiClock)
    }

    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid synchronization settings".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.validate() {
            Ok(value)
        } else {
            Err("invalid synchronization settings".into())
        }
    }

    pub fn create_midi_clock_master(&self) -> Option<MidiClockMaster> {
        (self.validate() && self.protocol == SyncProtocol::MidiClock).then(|| MidiClockMaster {
            preferences: self.midi_clock,
            destinations: self.midi_destinations.clone(),
            running: false,
            beat: 0.0,
        })
    }

    pub fn create_timecode_lock(&self) -> Result<TimecodeLockEngine, &'static str> {
        if !self.validate() || !matches!(self.protocol, SyncProtocol::Mtc | SyncProtocol::Ltc) {
            return Err("settings do not select timecode synchronization");
        }
        TimecodeLockEngine::new(self.timecode)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MidiClockMaster {
    pub preferences: MidiClockPreferences,
    pub destinations: Vec<MidiClockDestination>,
    pub running: bool,
    pub beat: f64,
}

impl MidiClockMaster {
    pub fn transport_start(&mut self, beat: f64) -> Result<Vec<Vec<u8>>, &'static str> {
        if !beat.is_finite() || beat < 0.0 {
            return Err("invalid MIDI clock position");
        }
        self.beat = beat;
        self.running = true;
        let mut messages = Vec::new();
        if self.preferences.follows_project_position {
            let spp = (beat * 4.0).round().clamp(0.0, 16_383.0) as u16;
            messages.push(vec![0xf2, (spp & 0x7f) as u8, ((spp >> 7) & 0x7f) as u8]);
        }
        messages.push(vec![if self.preferences.always_send_start || beat == 0.0 {
            0xfa
        } else {
            0xfb
        }]);
        Ok(messages)
    }

    pub fn transport_stop(&mut self) -> Vec<Vec<u8>> {
        self.running = false;
        vec![vec![0xfc]]
    }

    pub fn clock_pulse(&mut self) -> Option<Vec<u8>> {
        if !self.running && !self.preferences.send_clock_in_stop {
            return None;
        }
        if self.running {
            self.beat = (self.beat + 1.0 / 24.0).min(f64::MAX);
        }
        Some(vec![0xf8])
    }

    pub fn active_ports(&self) -> Vec<u32> {
        let mut ports: Vec<_> = self
            .destinations
            .iter()
            .filter(|destination| destination.enabled)
            .map(|destination| destination.port_id)
            .collect();
        ports.sort_unstable();
        ports
    }

    pub fn audit(&self) -> bool {
        self.beat.is_finite()
            && self.beat >= 0.0
            && self.destinations.len() <= 256
            && self
                .destinations
                .iter()
                .all(|destination| destination.port_id != 0)
            && self
                .destinations
                .iter()
                .enumerate()
                .all(|(index, destination)| {
                    self.destinations[..index]
                        .iter()
                        .all(|previous| previous.port_id != destination.port_id)
                })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiClockFollower {
    pub running: bool,
    pub beat: f64,
    pub bpm: Option<f64>,
    pub locked: bool,
    pulse_count: u32,
    last_pulse_us: Option<u64>,
    smoothed_interval_us: Option<f64>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SystemLinkTransferBits {
    Bits16,
    Bits24,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VstSystemLinkSettings {
    pub device_id: u32,
    pub device_name: String,
    pub active: bool,
    pub online: bool,
    pub asio_input: String,
    pub asio_output: String,
    pub data_only: bool,
    pub offset_samples: i32,
    pub transfer_bits: SystemLinkTransferBits,
    pub midi_inputs: u8,
    pub midi_outputs: u8,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SystemLinkTransport {
    Stop,
    Play,
    Record,
    Locate,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SystemLinkTransportPacket {
    pub origin_id: u32,
    pub sequence: u64,
    pub command: SystemLinkTransport,
    pub sample_position: u64,
    pub sample_rate: u32,
    pub tempo_micros_bpm: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SystemLinkMidiPacket {
    pub origin_id: u32,
    pub port: u8,
    pub sample_position: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SystemLinkPeer {
    pub id: u32,
    pub name: String,
    pub online: bool,
    pub sample_rate: u32,
    pub tempo_micros_bpm: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VstSystemLinkRuntime {
    pub settings: VstSystemLinkSettings,
    pub peers: std::collections::BTreeMap<u32, SystemLinkPeer>,
    pub clock_locked: bool,
    pub receiving: bool,
    pub sending: bool,
    pub transport: SystemLinkTransport,
    pub sample_position: u64,
    next_sequence: u64,
    received_sequences: std::collections::BTreeMap<u32, u64>,
}

impl VstSystemLinkSettings {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid VST System Link settings".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let settings: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if settings.validate() {
            Ok(settings)
        } else {
            Err("invalid VST System Link settings".into())
        }
    }

    pub fn validate(&self) -> bool {
        let valid_text =
            |value: &str| !value.trim().is_empty() && value.len() <= 256 && !value.contains('\0');
        self.device_id != 0
            && valid_text(&self.device_name)
            && valid_text(&self.asio_input)
            && valid_text(&self.asio_output)
            && self.offset_samples.unsigned_abs() <= 1_000_000
            && self.midi_inputs <= 16
            && self.midi_outputs <= 16
            && (!self.online || self.active)
    }

    pub fn runtime(self) -> Result<VstSystemLinkRuntime, String> {
        if !self.validate() {
            return Err("invalid VST System Link settings".into());
        }
        Ok(VstSystemLinkRuntime {
            settings: self,
            peers: std::collections::BTreeMap::new(),
            clock_locked: false,
            receiving: false,
            sending: false,
            transport: SystemLinkTransport::Stop,
            sample_position: 0,
            next_sequence: 1,
            received_sequences: std::collections::BTreeMap::new(),
        })
    }
}

impl VstSystemLinkRuntime {
    pub fn set_clock_locked(&mut self, locked: bool) {
        self.clock_locked = locked;
        if !locked {
            self.settings.online = false;
            self.receiving = false;
            self.sending = false;
        }
    }

    pub fn set_online(&mut self, online: bool) -> bool {
        if online && (!self.settings.active || !self.clock_locked) {
            return false;
        }
        self.settings.online = online;
        if !online {
            self.receiving = false;
            self.sending = false;
        }
        true
    }

    pub fn upsert_peer(&mut self, peer: SystemLinkPeer) -> bool {
        if peer.id == 0
            || peer.id == self.settings.device_id
            || peer.name.trim().is_empty()
            || peer.name.len() > 256
            || peer.name.contains('\0')
            || !(8_000..=384_000).contains(&peer.sample_rate)
            || !(1_000_000..=999_000_000).contains(&peer.tempo_micros_bpm)
        {
            return false;
        }
        if self.peers.values().any(|existing| {
            existing.id != peer.id && existing.name.eq_ignore_ascii_case(&peer.name)
        }) {
            return false;
        }
        if self.peers.len() >= 256 && !self.peers.contains_key(&peer.id) {
            return false;
        }
        self.peers.insert(peer.id, peer);
        self.receiving = true;
        true
    }

    pub fn send_transport(
        &mut self,
        command: SystemLinkTransport,
        sample_position: u64,
        sample_rate: u32,
        tempo_bpm: f64,
    ) -> Result<SystemLinkTransportPacket, String> {
        if !self.ready()
            || !(8_000..=384_000).contains(&sample_rate)
            || !tempo_bpm.is_finite()
            || !(1.0..=999.0).contains(&tempo_bpm)
        {
            return Err("VST System Link is not ready".into());
        }
        let packet = SystemLinkTransportPacket {
            origin_id: self.settings.device_id,
            sequence: self.next_sequence,
            command,
            sample_position,
            sample_rate,
            tempo_micros_bpm: (tempo_bpm * 1_000_000.0).round() as u64,
        };
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| "System Link sequence overflow".to_owned())?;
        self.transport = command;
        self.sample_position = apply_sample_offset(sample_position, self.settings.offset_samples);
        self.sending = true;
        Ok(packet)
    }

    pub fn receive_transport(&mut self, packet: SystemLinkTransportPacket) -> bool {
        if !self.ready()
            || packet.origin_id == self.settings.device_id
            || self.peers.get(&packet.origin_id).is_none_or(|peer| {
                !peer.online
                    || peer.sample_rate != packet.sample_rate
                    || peer.tempo_micros_bpm != packet.tempo_micros_bpm
            })
            || self
                .received_sequences
                .get(&packet.origin_id)
                .is_some_and(|sequence| packet.sequence <= *sequence)
        {
            return false;
        }
        self.received_sequences
            .insert(packet.origin_id, packet.sequence);
        self.transport = packet.command;
        self.sample_position =
            apply_sample_offset(packet.sample_position, self.settings.offset_samples);
        self.receiving = true;
        true
    }

    pub fn send_midi(
        &mut self,
        port: u8,
        sample_position: u64,
        bytes: &[u8],
    ) -> Option<SystemLinkMidiPacket> {
        if !self.ready() || port >= self.settings.midi_outputs || !valid_midi_message(bytes) {
            return None;
        }
        self.sending = true;
        Some(SystemLinkMidiPacket {
            origin_id: self.settings.device_id,
            port,
            sample_position: apply_sample_offset(sample_position, self.settings.offset_samples),
            bytes: bytes.to_vec(),
        })
    }

    pub fn receive_midi(&mut self, packet: &SystemLinkMidiPacket) -> bool {
        if !self.ready()
            || !self.peers.contains_key(&packet.origin_id)
            || packet.port >= self.settings.midi_inputs
            || !valid_midi_message(&packet.bytes)
        {
            return false;
        }
        self.receiving = true;
        true
    }

    pub fn self_test(&self) -> bool {
        self.ready()
            && self.receiving
            && self.sending
            && self.peers.values().any(|peer| peer.online)
    }

    fn ready(&self) -> bool {
        self.settings.active && self.settings.online && self.clock_locked
    }
}

fn apply_sample_offset(position: u64, offset: i32) -> u64 {
    if offset < 0 {
        position.saturating_sub(offset.unsigned_abs() as u64)
    } else {
        position.saturating_add(offset as u64)
    }
}

fn valid_midi_message(bytes: &[u8]) -> bool {
    (1..=1024).contains(&bytes.len())
        && bytes[0] & 0x80 != 0
        && bytes
            .iter()
            .skip(1)
            .all(|byte| *byte < 0x80 || bytes[0] == 0xf0)
        && (bytes[0] != 0xf0 || bytes.last() == Some(&0xf7))
}

#[cfg(test)]
mod system_link_tests {
    use super::*;

    fn settings(id: u32, name: &str, offset_samples: i32) -> VstSystemLinkSettings {
        VstSystemLinkSettings {
            device_id: id,
            device_name: name.into(),
            active: true,
            online: false,
            asio_input: "ADAT In 8".into(),
            asio_output: "ADAT Out 8".into(),
            data_only: false,
            offset_samples,
            transfer_bits: SystemLinkTransferBits::Bits24,
            midi_inputs: 2,
            midi_outputs: 2,
        }
    }

    fn peer(id: u32, name: &str) -> SystemLinkPeer {
        SystemLinkPeer {
            id,
            name: name.into(),
            online: true,
            sample_rate: 48_000,
            tempo_micros_bpm: 120_000_000,
        }
    }

    fn online_runtime(id: u32, name: &str, offset_samples: i32) -> VstSystemLinkRuntime {
        let mut runtime = settings(id, name, offset_samples).runtime().unwrap();
        runtime.set_clock_locked(true);
        assert!(runtime.set_online(true));
        runtime
    }

    #[test]
    fn settings_round_trip_and_reject_invalid_limits() {
        let original = settings(1, "Studio A", -32);
        let restored = VstSystemLinkSettings::from_json(&original.to_json().unwrap()).unwrap();
        assert_eq!(restored, original);

        let mut invalid = original.clone();
        invalid.active = false;
        invalid.online = true;
        assert!(!invalid.validate());
        invalid.online = false;
        invalid.midi_outputs = 17;
        assert!(!invalid.validate());
        invalid.midi_outputs = 2;
        invalid.offset_samples = i32::MIN;
        assert!(!invalid.validate());
    }

    #[test]
    fn transport_is_peer_to_peer_sample_accurate_and_rejects_stale_packets() {
        let mut sender = online_runtime(1, "Studio A", -32);
        let mut receiver = online_runtime(2, "Studio B", 16);
        assert!(sender.upsert_peer(peer(2, "Studio B")));
        assert!(receiver.upsert_peer(peer(1, "Studio A")));

        let packet = sender
            .send_transport(SystemLinkTransport::Play, 48_000, 48_000, 120.0)
            .unwrap();
        assert_eq!(sender.sample_position, 47_968);
        assert!(receiver.receive_transport(packet));
        assert_eq!(receiver.transport, SystemLinkTransport::Play);
        assert_eq!(receiver.sample_position, 48_016);
        assert!(!receiver.receive_transport(packet));

        let mut mismatch = packet;
        mismatch.sequence += 1;
        mismatch.tempo_micros_bpm = 121_000_000;
        assert!(!receiver.receive_transport(mismatch));
    }

    #[test]
    fn midi_ports_validate_messages_and_complete_self_test() {
        let mut sender = online_runtime(1, "Studio A", -32);
        let mut receiver = online_runtime(2, "Studio B", 16);
        assert!(sender.upsert_peer(peer(2, "Studio B")));
        assert!(receiver.upsert_peer(peer(1, "Studio A")));

        let packet = sender.send_midi(1, 1_000, &[0x90, 60, 100]).unwrap();
        assert_eq!(packet.sample_position, 968);
        assert!(sender.send_midi(2, 1_000, &[0x90, 60, 100]).is_none());
        assert!(sender.send_midi(0, 1_000, &[0x90, 0x80, 100]).is_none());
        assert!(receiver.receive_midi(&packet));
        assert!(sender.self_test());
        assert!(!receiver.self_test());
        assert!(receiver.send_midi(0, 1_000, &[0x80, 60, 0]).is_some());
        assert!(receiver.self_test());

        let mut invalid_port = packet.clone();
        invalid_port.port = 2;
        assert!(!receiver.receive_midi(&invalid_port));
        let mut invalid_message = packet;
        invalid_message.bytes = vec![60, 100];
        assert!(!receiver.receive_midi(&invalid_message));
    }

    #[test]
    fn clock_loss_forces_offline_and_clears_activity() {
        let mut runtime = online_runtime(1, "Studio A", 0);
        assert!(runtime.upsert_peer(peer(2, "Studio B")));
        assert!(runtime.send_midi(0, 0, &[0x90, 60, 100]).is_some());
        assert!(runtime.self_test());

        runtime.set_clock_locked(false);
        assert!(!runtime.settings.online);
        assert!(!runtime.receiving);
        assert!(!runtime.sending);
        assert!(!runtime.self_test());
        assert!(!runtime.set_online(true));
    }
}

impl Default for MidiClockFollower {
    fn default() -> Self {
        Self {
            running: false,
            beat: 0.0,
            bpm: None,
            locked: false,
            pulse_count: 0,
            last_pulse_us: None,
            smoothed_interval_us: None,
        }
    }
}

impl MidiClockFollower {
    pub fn start(&mut self) {
        self.running = true;
        self.beat = 0.0;
        self.reset_timing();
    }
    pub fn continue_playback(&mut self) {
        self.running = true;
        self.reset_timing();
    }
    pub fn stop(&mut self) {
        self.running = false;
        self.locked = false;
    }
    pub fn set_song_position_pointer(&mut self, lsb: u8, msb: u8) -> bool {
        if lsb > 0x7f || msb > 0x7f {
            return false;
        }
        let sixteenth_notes = u16::from(lsb) | (u16::from(msb) << 7);
        self.beat = f64::from(sixteenth_notes) / 4.0;
        true
    }

    pub fn pulse(&mut self, timestamp_us: u64) -> bool {
        if !self.running {
            return false;
        }
        if let Some(previous) = self.last_pulse_us {
            let interval = timestamp_us.saturating_sub(previous);
            if interval == 0 || interval > 2_000_000 {
                self.reset_timing();
                self.last_pulse_us = Some(timestamp_us);
                return false;
            }
            let smoothed = self
                .smoothed_interval_us
                .map(|old| old * 0.85 + interval as f64 * 0.15)
                .unwrap_or(interval as f64);
            self.smoothed_interval_us = Some(smoothed);
            self.bpm = Some(60_000_000.0 / (smoothed * 24.0));
        }
        self.last_pulse_us = Some(timestamp_us);
        self.pulse_count = self.pulse_count.saturating_add(1);
        self.beat = (self.beat + 1.0 / 24.0).min(f64::MAX);
        self.locked = self.pulse_count >= 24 && self.bpm.is_some();
        true
    }

    pub fn poll_timeout(&mut self, now_us: u64) -> bool {
        let timed_out = self
            .last_pulse_us
            .is_some_and(|last| now_us.saturating_sub(last) > 2_000_000);
        if timed_out {
            self.locked = false;
            self.running = false;
            self.reset_timing();
        }
        timed_out
    }

    fn reset_timing(&mut self) {
        self.pulse_count = 0;
        self.last_pulse_us = None;
        self.smoothed_interval_us = None;
        self.bpm = None;
        self.locked = false;
    }
}

#[cfg(test)]
mod pro_sync_tests {
    use super::*;

    #[test]
    fn mtc_requires_stable_frames_and_inhibits_after_dropout() {
        let mut engine = TimecodeLockEngine::new(TimecodeLockPreferences {
            lock_frames: 3,
            drop_out_frames: 2,
            inhibit_restart_ms: 500,
            auto_detect_frame_rate: false,
            project_fps: 30,
        })
        .unwrap();
        for frame in 0..3 {
            let state = engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 0,
                        frames: frame,
                        fps: 30,
                    },
                    frame as u64 * 33,
                )
                .unwrap();
            assert_eq!(
                state,
                if frame < 2 {
                    SyncLockState::Acquiring
                } else {
                    SyncLockState::Locked
                }
            );
        }
        assert_eq!(
            engine.report_missing_frames(3, 100),
            SyncLockState::DroppedOut
        );
        assert_eq!(
            engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 1,
                        frames: 0,
                        fps: 30
                    },
                    200
                )
                .unwrap(),
            SyncLockState::Inhibited
        );
        assert_eq!(
            engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 1,
                        frames: 1,
                        fps: 30
                    },
                    601
                )
                .unwrap(),
            SyncLockState::Acquiring
        );
        assert!(engine.audit());
    }

    #[test]
    fn frame_rate_mismatch_fails_or_auto_detects() {
        let mut strict = TimecodeLockEngine::new(TimecodeLockPreferences::default()).unwrap();
        assert!(strict
            .ingest(
                Timecode {
                    hours: 0,
                    minutes: 0,
                    seconds: 0,
                    frames: 0,
                    fps: 25
                },
                0
            )
            .is_err());
        let mut automatic = TimecodeLockEngine::new(TimecodeLockPreferences {
            auto_detect_frame_rate: true,
            ..TimecodeLockPreferences::default()
        })
        .unwrap();
        assert_eq!(
            automatic
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 0,
                        frames: 0,
                        fps: 25
                    },
                    0
                )
                .unwrap(),
            SyncLockState::Acquiring
        );
        assert_eq!(automatic.detected_fps, Some(25));
    }

    #[test]
    fn midi_clock_master_emits_spp_continue_and_stop_mode_clock() {
        let mut master = MidiClockMaster {
            preferences: MidiClockPreferences {
                follows_project_position: true,
                always_send_start: false,
                send_clock_in_stop: true,
            },
            destinations: vec![MidiClockDestination {
                port_id: 7,
                enabled: true,
            }],
            running: false,
            beat: 0.0,
        };
        assert_eq!(
            master.transport_start(4.0).unwrap(),
            vec![vec![0xf2, 16, 0], vec![0xfb]]
        );
        assert_eq!(master.transport_stop(), vec![vec![0xfc]]);
        assert_eq!(master.clock_pulse(), Some(vec![0xf8]));
        assert_eq!(master.active_ports(), vec![7]);
        assert!(master.audit());
    }

    #[test]
    fn midi_clock_follower_smooths_tempo_and_times_out() {
        let mut follower = MidiClockFollower::default();
        follower.start();
        for pulse in 0..24 {
            assert!(follower.pulse(pulse * 20_833));
        }
        assert!(follower.locked);
        assert!((follower.bpm.unwrap() - 120.0).abs() < 0.1);
        assert!(follower.set_song_position_pointer(8, 0));
        assert_eq!(follower.beat, 2.0);
        assert!(follower.poll_timeout(3_000_000));
        assert!(!follower.running);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn sync_settings_round_trip_recreates_clean_runtime_engines() {
        let settings = SyncProjectSettings {
            source: SyncSource::MidiPort(7),
            protocol: SyncProtocol::MidiClock,
            enabled: true,
            timecode: TimecodeLockPreferences::default(),
            midi_clock: MidiClockPreferences {
                follows_project_position: true,
                always_send_start: false,
                send_clock_in_stop: true,
            },
            midi_destinations: vec![MidiClockDestination {
                port_id: 7,
                enabled: true,
            }],
            mmc_device_id: 0x7f,
        };
        let json = settings.to_json().unwrap();
        let restored = SyncProjectSettings::from_json(&json).unwrap();
        assert_eq!(restored, settings);
        let master = restored.create_midi_clock_master().unwrap();
        assert!(!master.running);
        assert_eq!(master.beat, 0.0);
        assert_eq!(master.active_ports(), vec![7]);
    }

    #[test]
    fn sync_settings_reject_duplicate_ports_and_protocol_mismatch() {
        let mut settings = SyncProjectSettings {
            source: SyncSource::MidiPort(1),
            protocol: SyncProtocol::Mtc,
            enabled: true,
            timecode: TimecodeLockPreferences::default(),
            midi_clock: MidiClockPreferences {
                follows_project_position: false,
                always_send_start: false,
                send_clock_in_stop: false,
            },
            midi_destinations: vec![],
            mmc_device_id: 0,
        };
        assert!(settings.create_timecode_lock().is_ok());
        settings.midi_destinations = vec![
            MidiClockDestination {
                port_id: 1,
                enabled: true,
            },
            MidiClockDestination {
                port_id: 1,
                enabled: false,
            },
        ];
        assert!(settings.to_json().is_err());
    }
}
