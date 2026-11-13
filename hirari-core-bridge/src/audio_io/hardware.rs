#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ControllerProtocol {
    MCU,
    HUI,
    EuCon,
    OSC,
    Ssl,
    Avid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DigitalSync {
    Internal,
    WordClock,
    Adat,
    Spdif,
}
pub fn parse_digital_sync(value: &str) -> Option<DigitalSync> {
    match value.trim().to_ascii_lowercase().as_str() {
        "internal" | "int" => Some(DigitalSync::Internal),
        "wordclock" | "word_clock" | "wc" => Some(DigitalSync::WordClock),
        "adat" => Some(DigitalSync::Adat),
        "spdif" | "s/pdif" => Some(DigitalSync::Spdif),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioDeviceClock {
    pub device_id: u32,
    pub sample_rate: u32,
    pub sync: DigitalSync,
    pub locked: bool,
}

impl AudioDeviceClock {
    pub fn compatible(&self, other: &Self) -> bool {
        self.sample_rate > 0
            && self.sample_rate == other.sample_rate
            && (self.locked || other.locked || self.sync == DigitalSync::Internal)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClockDomain {
    pub devices: Vec<AudioDeviceClock>,
}
impl ClockDomain {
    pub fn locked_devices(&self) -> Vec<u32> {
        let mut out: Vec<_> = self
            .devices
            .iter()
            .filter(|d| d.locked)
            .map(|d| d.device_id)
            .collect();
        out.sort_unstable();
        out
    }
    pub fn is_locked(&self, device_id: u32) -> bool {
        self.devices
            .iter()
            .find(|d| d.device_id == device_id)
            .is_some_and(|d| d.locked)
    }
    pub fn set_sync(&mut self, device_id: u32, sync: DigitalSync) -> bool {
        let Some(index) = self.devices.iter().position(|d| d.device_id == device_id) else {
            return false;
        };
        let old = self.devices[index].sync;
        self.devices[index].sync = sync;
        self.devices[index].locked = matches!(sync, DigitalSync::Internal);
        if self.validate() {
            true
        } else {
            self.devices[index].sync = old;
            self.devices[index].locked = matches!(old, DigitalSync::Internal);
            false
        }
    }
    pub fn external_devices(&self) -> Vec<u32> {
        let mut out: Vec<_> = self
            .devices
            .iter()
            .filter(|d| d.sync != DigitalSync::Internal)
            .map(|d| d.device_id)
            .collect();
        out.sort_unstable();
        out
    }
}
impl ClockDomain {
    pub fn add(&mut self, clock: AudioDeviceClock) -> bool {
        if clock.device_id == 0
            || clock.sample_rate == 0
            || self.devices.iter().any(|d| d.device_id == clock.device_id)
            || self.devices.iter().any(|d| !d.compatible(&clock))
        {
            return false;
        }
        self.devices.push(clock);
        true
    }
    pub fn remove(&mut self, device_id: u32) -> bool {
        let n = self.devices.len();
        self.devices.retain(|d| d.device_id != device_id);
        n != self.devices.len()
    }
    pub fn validate(&self) -> bool {
        self.devices.len() <= 64
            && self
                .devices
                .iter()
                .all(|d| d.device_id != 0 && d.sample_rate > 0)
            && self
                .devices
                .iter()
                .enumerate()
                .all(|(i, d)| self.devices[..i].iter().all(|p| p.device_id != d.device_id))
            && self.devices.windows(2).all(|w| w[0].compatible(&w[1]))
            && self
                .devices
                .iter()
                .filter(|d| d.sync == DigitalSync::Internal)
                .count()
                <= 1
            && self.devices.iter().map(|d| d.sample_rate).all(|rate| {
                self.devices
                    .first()
                    .map(|d| d.sample_rate == rate)
                    .unwrap_or(true)
            })
    }
    pub fn master_device(&self) -> Option<u32> {
        self.devices
            .iter()
            .find(|d| d.sync == DigitalSync::Internal)
            .or_else(|| self.devices.iter().find(|d| d.locked))
            .map(|d| d.device_id)
    }
    pub fn set_master(&mut self, device_id: u32) -> bool {
        let Some(master) = self.devices.iter_mut().find(|d| d.device_id == device_id) else {
            return false;
        };
        let rate = master.sample_rate;
        master.sync = DigitalSync::Internal;
        master.locked = true;
        for device in &mut self.devices {
            if device.device_id != device_id {
                device.sample_rate = rate;
            }
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnedControl {
    pub controller_id: u32,
    pub control_id: u32,
    pub target: String,
}

impl LearnedControl {
    pub fn validate(&self) -> bool {
        self.controller_id != 0
            && self.target.len() <= 128
            && !self.target.trim().is_empty()
            && !self.target.contains('\0')
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LearnedControlMap {
    pub bindings: Vec<LearnedControl>,
}
impl LearnedControlMap {
    pub fn learn(&mut self, binding: LearnedControl) -> bool {
        if !binding.validate() {
            return false;
        }
        let mut binding = binding;
        binding.target = binding.target.trim().to_owned();
        if let Some(existing) = self.bindings.iter_mut().find(|b| {
            b.controller_id == binding.controller_id && b.control_id == binding.control_id
        }) {
            *existing = binding;
        } else {
            self.bindings.push(binding);
        }
        true
    }
    pub fn target_for(&self, controller_id: u32, control_id: u32) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| b.controller_id == controller_id && b.control_id == control_id)
            .map(|b| b.target.as_str())
    }
    pub fn controls_for_target(&self, target: &str) -> Vec<(u32, u32)> {
        let mut out: Vec<_> = self
            .bindings
            .iter()
            .filter(|b| b.target == target)
            .map(|b| (b.controller_id, b.control_id))
            .collect();
        out.sort_unstable();
        out
    }
    pub fn forget_target(&mut self, target: &str) -> usize {
        let before = self.bindings.len();
        self.bindings.retain(|b| b.target != target);
        before - self.bindings.len()
    }
    pub fn feedback_for(&self, controller_id: u32, target: &str) -> Vec<u32> {
        let mut out: Vec<_> = self
            .bindings
            .iter()
            .filter(|b| b.controller_id == controller_id && b.target == target)
            .map(|b| b.control_id)
            .collect();
        out.sort_unstable();
        out
    }
    pub fn validate(&self) -> bool {
        self.bindings.len() <= 4096
            && self.bindings.iter().all(LearnedControl::validate)
            && self.bindings.iter().enumerate().all(|(i, b)| {
                self.bindings[..i]
                    .iter()
                    .all(|p| (p.controller_id, p.control_id) != (b.controller_id, b.control_id))
            })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControllerEvent {
    pub controller_id: u32,
    pub control_id: u32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ControllerFamily {
    Generic,
    Ssl,
    Avid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemoteTransport {
    Play,
    Stop,
    Record,
    FastForward,
    Rewind,
    LocateStart,
}
pub fn parse_remote_transport(command: &str) -> Option<RemoteTransport> {
    match command.trim().to_ascii_lowercase().as_str() {
        "play" => Some(RemoteTransport::Play),
        "stop" => Some(RemoteTransport::Stop),
        "record" | "rec" => Some(RemoteTransport::Record),
        "ff" | "fast_forward" => Some(RemoteTransport::FastForward),
        "rew" | "rewind" => Some(RemoteTransport::Rewind),
        "locate_start" | "home" => Some(RemoteTransport::LocateStart),
        _ => None,
    }
}

pub fn controller_family(name: &str) -> Option<ControllerFamily> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 128 {
        return None;
    }
    if normalized.contains("ssl") {
        Some(ControllerFamily::Ssl)
    } else if normalized.contains("avid")
        || normalized.contains("artist")
        || normalized.contains("s1")
    {
        Some(ControllerFamily::Avid)
    } else {
        Some(ControllerFamily::Generic)
    }
}

pub struct HardwareOrchestrator {
    pub controllers: std::collections::HashMap<u32, ControllerProtocol>,
}
impl HardwareOrchestrator {
    pub fn accepts_transport(&self, controller_id: u32, _command: RemoteTransport) -> bool {
        self.controllers.contains_key(&controller_id)
    }
}

impl Default for HardwareOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareOrchestrator {
    pub fn new() -> Self {
        Self {
            controllers: std::collections::HashMap::new(),
        }
    }

    pub fn register_controller(
        &mut self,
        controller_id: u32,
        protocol: ControllerProtocol,
    ) -> bool {
        if controller_id == 0 || self.controllers.contains_key(&controller_id) {
            return false;
        }
        self.controllers.insert(controller_id, protocol);
        true
    }

    pub fn unregister_controller(&mut self, controller_id: u32) -> bool {
        self.controllers.remove(&controller_id).is_some()
    }

    /// INDUSTRIAL: Dispatches incoming control events with absolute precision and protocol sovereignty.
    pub fn dispatch_event(&self, event: ControllerEvent) -> Option<ControllerEvent> {
        let protocol = self.controllers.get(&event.controller_id)?;
        match protocol {
            ControllerProtocol::MCU => self.decode_mcu(event),
            ControllerProtocol::HUI => self.decode_hui(event),
            ControllerProtocol::EuCon
            | ControllerProtocol::OSC
            | ControllerProtocol::Ssl
            | ControllerProtocol::Avid => self.normalize_event(event),
        }
    }

    /// Decode the provider-neutral OSC text boundary used by scripts and
    /// controllers. Accepted form: `/hirari/<target>/<id> <normalized-value>`.
    /// Parsing is bounded and fail-closed; malformed addresses never become
    /// control events.
    pub fn parse_osc_message(&self, controller_id: u32, message: &str) -> Option<ControllerEvent> {
        let mut fields = message.split_whitespace();
        let address = fields.next()?;
        let raw_value = fields.next()?;
        let value = match raw_value {
            "true" => 1.0,
            "false" => 0.0,
            _ => raw_value.parse::<f32>().ok()?,
        };
        if fields.next().is_some() || !address.starts_with("/hirari/") || address.len() > 128 {
            return None;
        }
        let parts: Vec<&str> = address.trim_matches('/').split('/').collect();
        if parts.len() != 3 || parts[0] != "hirari" {
            return None;
        }
        let control_id = parts[2].parse::<u32>().ok()?;
        self.controllers
            .get(&controller_id)
            .and_then(|protocol| {
                (*protocol == ControllerProtocol::OSC).then_some(ControllerEvent {
                    controller_id,
                    control_id,
                    value,
                })
            })
            .and_then(|event| self.dispatch_event(event))
    }

    /// Encode a normalized event for OSC feedback to the registered OSC
    /// controller. The address is fixed to avoid arbitrary path injection.
    pub fn encode_osc_message(&self, event: &ControllerEvent, target: &str) -> Option<String> {
        if self.controllers.get(&event.controller_id) != Some(&ControllerProtocol::OSC)
            || target.trim().is_empty()
            || target.len() > 64
            || !target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            || !event.value.is_finite()
        {
            return None;
        }
        Some(format!(
            "/hirari/{target}/{} {:.6}",
            event.control_id,
            event.value.clamp(0.0, 1.0)
        ))
    }

    /// Decode the small provider-neutral EuCon control frame used by the
    /// hardware bridge: little-endian u32 control id followed by f32 value.
    /// The real transport remains outside the audio thread and malformed
    /// frames fail closed.
    pub fn parse_eucon_frame(&self, controller_id: u32, frame: &[u8]) -> Option<ControllerEvent> {
        if frame.len() != 8
            || self.controllers.get(&controller_id) != Some(&ControllerProtocol::EuCon)
        {
            return None;
        }
        let control_id = u32::from_le_bytes(frame[0..4].try_into().ok()?);
        let value = f32::from_le_bytes(frame[4..8].try_into().ok()?);
        self.dispatch_event(ControllerEvent {
            controller_id,
            control_id,
            value,
        })
    }

    /// Encode the provider-neutral EuCon feedback frame used by the hardware
    /// bridge: little-endian u32 control id followed by a normalized f32.
    pub fn encode_eucon_frame(&self, event: &ControllerEvent) -> Option<[u8; 8]> {
        if self.controllers.get(&event.controller_id) != Some(&ControllerProtocol::EuCon)
            || !event.value.is_finite()
        {
            return None;
        }
        let mut frame = [0u8; 8];
        frame[0..4].copy_from_slice(&event.control_id.to_le_bytes());
        frame[4..8].copy_from_slice(&event.value.clamp(0.0, 1.0).to_le_bytes());
        Some(frame)
    }

    /// Decode the compact bridge frames emitted by MCU/HUI adapters:
    /// `[control-id:u16-le, value:u16-le]`, normalized from 14-bit MIDI-style
    /// controller values. The physical transport remains platform-specific.
    pub fn parse_surface_frame(&self, controller_id: u32, frame: &[u8]) -> Option<ControllerEvent> {
        if frame.len() != 4 {
            return None;
        }
        let protocol = self.controllers.get(&controller_id)?;
        if !matches!(protocol, ControllerProtocol::MCU | ControllerProtocol::HUI) {
            return None;
        }
        let control_id = u16::from_le_bytes(frame[0..2].try_into().ok()?) as u32;
        let raw = u16::from_le_bytes(frame[2..4].try_into().ok()?) as f32;
        self.dispatch_event(ControllerEvent {
            controller_id,
            control_id,
            value: raw / 16_383.0,
        })
    }

    /// Encode a normalized fader/button value for the compact MCU/HUI bridge
    /// frame `[control-id:u16-le, value:u16-le]`.
    pub fn encode_surface_frame(&self, event: &ControllerEvent) -> Option<[u8; 4]> {
        if !matches!(
            self.controllers.get(&event.controller_id),
            Some(ControllerProtocol::MCU | ControllerProtocol::HUI)
        ) || event.control_id > u16::MAX as u32
            || !event.value.is_finite()
        {
            return None;
        }
        let raw = (event.value.clamp(0.0, 1.0) * 16_383.0).round() as u16;
        let mut frame = [0u8; 4];
        frame[0..2].copy_from_slice(&(event.control_id as u16).to_le_bytes());
        frame[2..4].copy_from_slice(&raw.to_le_bytes());
        Some(frame)
    }

    /// Route SSL/Avid surfaces through the provider-neutral normalized frame.
    /// Vendor-specific wire framing remains outside the audio thread, while
    /// this boundary prevents an event from being emitted to the wrong family.
    pub fn encode_vendor_event(&self, event: &ControllerEvent) -> Option<[u8; 8]> {
        if !matches!(
            self.controllers.get(&event.controller_id),
            Some(ControllerProtocol::Ssl | ControllerProtocol::Avid)
        ) || !event.value.is_finite()
        {
            return None;
        }
        let mut frame = [0u8; 8];
        frame[0..4].copy_from_slice(&event.control_id.to_le_bytes());
        frame[4..8].copy_from_slice(&event.value.clamp(0.0, 1.0).to_le_bytes());
        Some(frame)
    }

    fn decode_mcu(&self, event: ControllerEvent) -> Option<ControllerEvent> {
        self.normalize_event(event)
    }

    fn decode_hui(&self, event: ControllerEvent) -> Option<ControllerEvent> {
        self.normalize_event(event)
    }

    fn normalize_event(&self, mut event: ControllerEvent) -> Option<ControllerEvent> {
        if !event.value.is_finite() {
            return None;
        }
        event.value = event.value.clamp(0.0, 1.0);
        Some(event)
    }

    /// INDUSTRIAL: Performs a forensic audit of the hardware synchronization state.
    pub fn audit_hardware(&self) -> bool {
        self.controllers.len() <= 4096 && self.controllers.keys().all(|id| *id != 0)
    }
}

#[cfg(test)]
#[path = "hardware_tests.rs"]
mod tests;
#[path = "hardware_remote.rs"]
mod remote;
pub use remote::*;

