/// Deterministic external MIDI Clock/MTC/LTC transport conversion.

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

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
