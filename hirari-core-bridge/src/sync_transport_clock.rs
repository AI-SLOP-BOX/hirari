include!("sync_transport_mtc.rs");
include!("sync_transport_lock.rs");
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
