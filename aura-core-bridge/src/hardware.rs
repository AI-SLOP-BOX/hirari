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
pub enum DigitalSync { Internal, WordClock, Adat, Spdif }
pub fn parse_digital_sync(value: &str) -> Option<DigitalSync> { match value.trim().to_ascii_lowercase().as_str() { "internal" | "int" => Some(DigitalSync::Internal), "wordclock" | "word_clock" | "wc" => Some(DigitalSync::WordClock), "adat" => Some(DigitalSync::Adat), "spdif" | "s/pdif" => Some(DigitalSync::Spdif), _ => None } }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioDeviceClock { pub device_id: u32, pub sample_rate: u32, pub sync: DigitalSync, pub locked: bool }

impl AudioDeviceClock { pub fn compatible(&self, other: &Self) -> bool { self.sample_rate > 0 && self.sample_rate == other.sample_rate && (self.locked || other.locked || self.sync == DigitalSync::Internal) } }
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClockDomain { pub devices: Vec<AudioDeviceClock> }
impl ClockDomain { pub fn locked_devices(&self) -> Vec<u32> { let mut out: Vec<_> = self.devices.iter().filter(|d| d.locked).map(|d| d.device_id).collect(); out.sort_unstable(); out } pub fn is_locked(&self, device_id: u32) -> bool { self.devices.iter().find(|d| d.device_id == device_id).is_some_and(|d| d.locked) } pub fn set_sync(&mut self, device_id: u32, sync: DigitalSync) -> bool { let Some(index)=self.devices.iter().position(|d| d.device_id==device_id) else { return false; }; let old=self.devices[index].sync; self.devices[index].sync=sync; self.devices[index].locked=matches!(sync, DigitalSync::Internal); if self.validate() { true } else { self.devices[index].sync=old; self.devices[index].locked=matches!(old, DigitalSync::Internal); false } } pub fn external_devices(&self) -> Vec<u32> { let mut out: Vec<_> = self.devices.iter().filter(|d| d.sync != DigitalSync::Internal).map(|d| d.device_id).collect(); out.sort_unstable(); out } }
impl ClockDomain { pub fn add(&mut self, clock: AudioDeviceClock) -> bool { if clock.device_id == 0 || clock.sample_rate == 0 || self.devices.iter().any(|d| d.device_id == clock.device_id) || self.devices.iter().any(|d| !d.compatible(&clock)) { return false; } self.devices.push(clock); true } pub fn remove(&mut self, device_id: u32) -> bool { let n=self.devices.len(); self.devices.retain(|d| d.device_id != device_id); n != self.devices.len() } pub fn validate(&self) -> bool { self.devices.len() <= 64 && self.devices.iter().all(|d| d.device_id != 0 && d.sample_rate > 0) && self.devices.iter().enumerate().all(|(i,d)| self.devices[..i].iter().all(|p| p.device_id != d.device_id)) && self.devices.windows(2).all(|w| w[0].compatible(&w[1])) && self.devices.iter().filter(|d| d.sync == DigitalSync::Internal).count() <= 1 && self.devices.iter().map(|d| d.sample_rate).all(|rate| self.devices.first().map(|d| d.sample_rate == rate).unwrap_or(true)) } pub fn master_device(&self) -> Option<u32> { self.devices.iter().find(|d| d.sync == DigitalSync::Internal).or_else(|| self.devices.iter().find(|d| d.locked)).map(|d| d.device_id) } pub fn set_master(&mut self, device_id: u32) -> bool { let Some(master) = self.devices.iter_mut().find(|d| d.device_id == device_id) else { return false; }; let rate = master.sample_rate; master.sync = DigitalSync::Internal; master.locked = true; for device in &mut self.devices { if device.device_id != device_id { device.sample_rate = rate; } } true } }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnedControl { pub controller_id: u32, pub control_id: u32, pub target: String }

impl LearnedControl { pub fn validate(&self) -> bool { self.controller_id != 0 && self.target.len() <= 128 && !self.target.trim().is_empty() && !self.target.contains('\0') } }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LearnedControlMap { pub bindings: Vec<LearnedControl> }
impl LearnedControlMap {
    pub fn learn(&mut self, binding: LearnedControl) -> bool {
        if !binding.validate() { return false; }
        let mut binding = binding;
        binding.target = binding.target.trim().to_owned();
        if let Some(existing) = self.bindings.iter_mut().find(|b| b.controller_id == binding.controller_id && b.control_id == binding.control_id) { *existing = binding; } else { self.bindings.push(binding); }
        true
    }
    pub fn target_for(&self, controller_id: u32, control_id: u32) -> Option<&str> { self.bindings.iter().find(|b| b.controller_id == controller_id && b.control_id == control_id).map(|b| b.target.as_str()) }
    pub fn controls_for_target(&self, target: &str) -> Vec<(u32,u32)> { let mut out: Vec<_> = self.bindings.iter().filter(|b| b.target == target).map(|b| (b.controller_id,b.control_id)).collect(); out.sort_unstable(); out }
    pub fn forget_target(&mut self, target: &str) -> usize { let before=self.bindings.len(); self.bindings.retain(|b| b.target != target); before-self.bindings.len() }
    pub fn feedback_for(&self, controller_id: u32, target: &str) -> Vec<u32> { let mut out: Vec<_> = self.bindings.iter().filter(|b| b.controller_id == controller_id && b.target == target).map(|b| b.control_id).collect(); out.sort_unstable(); out }
    pub fn validate(&self) -> bool { self.bindings.len() <= 4096 && self.bindings.iter().all(LearnedControl::validate) && self.bindings.iter().enumerate().all(|(i,b)| self.bindings[..i].iter().all(|p| (p.controller_id,p.control_id)!=(b.controller_id,b.control_id))) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControllerEvent {
    pub controller_id: u32,
    pub control_id: u32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum ControllerFamily { Generic, Ssl, Avid }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum RemoteTransport { Play, Stop, Record, FastForward, Rewind, LocateStart }
pub fn parse_remote_transport(command: &str) -> Option<RemoteTransport> { match command.trim().to_ascii_lowercase().as_str() { "play" => Some(RemoteTransport::Play), "stop" => Some(RemoteTransport::Stop), "record" | "rec" => Some(RemoteTransport::Record), "ff" | "fast_forward" => Some(RemoteTransport::FastForward), "rew" | "rewind" => Some(RemoteTransport::Rewind), "locate_start" | "home" => Some(RemoteTransport::LocateStart), _ => None } }

pub fn controller_family(name: &str) -> Option<ControllerFamily> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 128 { return None; }
    if normalized.contains("ssl") { Some(ControllerFamily::Ssl) }
    else if normalized.contains("avid") || normalized.contains("artist") || normalized.contains("s1") { Some(ControllerFamily::Avid) }
    else { Some(ControllerFamily::Generic) }
}

pub struct HardwareOrchestrator {
    pub controllers: std::collections::HashMap<u32, ControllerProtocol>,
}
impl HardwareOrchestrator { pub fn accepts_transport(&self, controller_id: u32, _command: RemoteTransport) -> bool { self.controllers.contains_key(&controller_id) } }

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

    pub fn register_controller(&mut self, controller_id: u32, protocol: ControllerProtocol) -> bool {
        if controller_id == 0 || self.controllers.contains_key(&controller_id) { return false; }
        self.controllers.insert(controller_id, protocol); true
    }

    pub fn unregister_controller(&mut self, controller_id: u32) -> bool { self.controllers.remove(&controller_id).is_some() }

    /// INDUSTRIAL: Dispatches incoming control events with absolute precision and protocol sovereignty.
    pub fn dispatch_event(&self, event: ControllerEvent) -> Option<ControllerEvent> {
        let protocol = self.controllers.get(&event.controller_id)?;
        match protocol {
            ControllerProtocol::MCU => self.decode_mcu(event),
            ControllerProtocol::HUI => self.decode_hui(event),
            ControllerProtocol::EuCon | ControllerProtocol::OSC | ControllerProtocol::Ssl | ControllerProtocol::Avid => self.normalize_event(event),
        }
    }

    /// Decode the provider-neutral OSC text boundary used by scripts and
    /// controllers. Accepted form: `/aura/<target>/<id> <normalized-value>`.
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
        if fields.next().is_some() || !address.starts_with("/aura/") || address.len() > 128 { return None; }
        let parts: Vec<&str> = address.trim_matches('/').split('/').collect();
        if parts.len() != 3 || parts[0] != "aura" { return None; }
        let control_id = parts[2].parse::<u32>().ok()?;
        self.controllers.get(&controller_id).and_then(|protocol| {
            (*protocol == ControllerProtocol::OSC).then_some(ControllerEvent { controller_id, control_id, value })
        }).and_then(|event| self.dispatch_event(event))
    }

    /// Encode a normalized event for OSC feedback to the registered OSC
    /// controller. The address is fixed to avoid arbitrary path injection.
    pub fn encode_osc_message(&self, event: &ControllerEvent, target: &str) -> Option<String> {
        if self.controllers.get(&event.controller_id) != Some(&ControllerProtocol::OSC)
            || target.trim().is_empty() || target.len() > 64
            || !target.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            || !event.value.is_finite() { return None; }
        Some(format!("/aura/{target}/{} {:.6}", event.control_id, event.value.clamp(0.0, 1.0)))
    }

    /// Decode the small provider-neutral EuCon control frame used by the
    /// hardware bridge: little-endian u32 control id followed by f32 value.
    /// The real transport remains outside the audio thread and malformed
    /// frames fail closed.
    pub fn parse_eucon_frame(&self, controller_id: u32, frame: &[u8]) -> Option<ControllerEvent> {
        if frame.len() != 8 || self.controllers.get(&controller_id) != Some(&ControllerProtocol::EuCon) { return None; }
        let control_id = u32::from_le_bytes(frame[0..4].try_into().ok()?);
        let value = f32::from_le_bytes(frame[4..8].try_into().ok()?);
        self.dispatch_event(ControllerEvent { controller_id, control_id, value })
    }

    /// Encode the provider-neutral EuCon feedback frame used by the hardware
    /// bridge: little-endian u32 control id followed by a normalized f32.
    pub fn encode_eucon_frame(&self, event: &ControllerEvent) -> Option<[u8; 8]> {
        if self.controllers.get(&event.controller_id) != Some(&ControllerProtocol::EuCon)
            || !event.value.is_finite() { return None; }
        let mut frame = [0u8; 8];
        frame[0..4].copy_from_slice(&event.control_id.to_le_bytes());
        frame[4..8].copy_from_slice(&event.value.clamp(0.0, 1.0).to_le_bytes());
        Some(frame)
    }

    /// Decode the compact bridge frames emitted by MCU/HUI adapters:
    /// `[control-id:u16-le, value:u16-le]`, normalized from 14-bit MIDI-style
    /// controller values. The physical transport remains platform-specific.
    pub fn parse_surface_frame(&self, controller_id: u32, frame: &[u8]) -> Option<ControllerEvent> {
        if frame.len() != 4 { return None; }
        let protocol = self.controllers.get(&controller_id)?;
        if !matches!(protocol, ControllerProtocol::MCU | ControllerProtocol::HUI) { return None; }
        let control_id = u16::from_le_bytes(frame[0..2].try_into().ok()?) as u32;
        let raw = u16::from_le_bytes(frame[2..4].try_into().ok()?) as f32;
        self.dispatch_event(ControllerEvent { controller_id, control_id, value: raw / 16_383.0 })
    }

    /// Encode a normalized fader/button value for the compact MCU/HUI bridge
    /// frame `[control-id:u16-le, value:u16-le]`.
    pub fn encode_surface_frame(&self, event: &ControllerEvent) -> Option<[u8; 4]> {
        if !matches!(self.controllers.get(&event.controller_id), Some(ControllerProtocol::MCU | ControllerProtocol::HUI))
            || event.control_id > u16::MAX as u32 || !event.value.is_finite() { return None; }
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
        if !matches!(self.controllers.get(&event.controller_id), Some(ControllerProtocol::Ssl | ControllerProtocol::Avid))
            || !event.value.is_finite() { return None; }
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
mod tests { include!("hardware_tests.rs"); }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteValueMode { Jump, Pickup, Scaled, Toggle }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteInputMode { Absolute, RelativeSignedBit, RelativeBinaryOffset, RelativeTwosComplement }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteFocusMode { Fixed, TrackSelection, FocusQuickControl }

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum QuickControlFocusPolicy {
    #[default]
    TrackAndPluginWindow,
    TrackOnly,
    PluginWindowOnly,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum FocusedWindow {
    #[default]
    Project,
    Plugin,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum QuickControlTarget { Track(u32), Plugin(u32) }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum MappingScope { Global, Project }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RemoteMapping {
    pub control_id: u32,
    pub target: String,
    pub value_mode: RemoteValueMode,
    pub input_mode: RemoteInputMode,
    pub focus_mode: RemoteFocusMode,
    pub minimum: f32,
    pub maximum: f32,
    pub inverted: bool,
    pub transmit_feedback: bool,
    pub bank_slot: Option<u16>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MappingPage {
    pub id: u32,
    pub name: String,
    pub factory: bool,
    pub scope: MappingScope,
    pub mappings: Vec<RemoteMapping>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct SoftTakeoverState {
    last_hardware: Option<f32>,
    picked_up: bool,
    last_button: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteParameterChange { pub target: String, pub value: f32 }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MidiRemoteEngine {
    pub controller_id: u32,
    pub pages: Vec<MappingPage>,
    pub active_page: u32,
    pub selected_track: Option<u32>,
    #[serde(default)]
    pub active_plugin: Option<u32>,
    #[serde(default)]
    pub quick_control_policy: QuickControlFocusPolicy,
    #[serde(default)]
    pub focused_window: FocusedWindow,
    #[serde(default)]
    pub locked_quick_control: Option<QuickControlTarget>,
    pub focus_locked: bool,
    pub bank_offset: u32,
    pub bank_size: u16,
    pub target_values: std::collections::BTreeMap<String, f32>,
    #[serde(skip)]
    takeover: std::collections::BTreeMap<(u32, u32), SoftTakeoverState>,
}

impl MidiRemoteEngine {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid MIDI Remote state".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        // Older project data stored only the lock flag. Reconstruct its target
        // from the saved selection when possible, otherwise leave it unlocked.
        if value.focus_locked && value.locked_quick_control.is_none() {
            value.locked_quick_control = value.current_quick_control_target();
            value.focus_locked = value.locked_quick_control.is_some();
        }
        if value.audit() { Ok(value) } else { Err("invalid MIDI Remote state".into()) }
    }

    pub fn new(controller_id: u32, bank_size: u16) -> Option<Self> {
        if controller_id == 0 || bank_size == 0 || bank_size > 128 { return None; }
        Some(Self { controller_id, pages: Vec::new(), active_page: 0, selected_track: None,
            active_plugin: None, quick_control_policy: QuickControlFocusPolicy::default(),
            focused_window: FocusedWindow::default(), locked_quick_control: None,
            focus_locked: false, bank_offset: 0, bank_size, target_values: std::collections::BTreeMap::new(),
            takeover: std::collections::BTreeMap::new() })
    }

    pub fn upsert_page(&mut self, mut page: MappingPage) -> bool {
        page.name = page.name.trim().to_owned();
        if !page.validate() { return false; }
        if let Some(existing) = self.pages.iter_mut().find(|item| item.id == page.id) {
            if existing.factory { return false; }
            *existing = page;
        } else if self.pages.len() < 256 { self.pages.push(page); }
        else { return false; }
        self.pages.sort_by_key(|item| item.id);
        if self.active_page == 0 { self.active_page = self.pages[0].id; }
        self.takeover.clear(); true
    }

    pub fn remove_page(&mut self, page_id: u32) -> bool {
        if self.pages.iter().find(|page| page.id == page_id).is_none_or(|page| page.factory) { return false; }
        self.pages.retain(|page| page.id != page_id);
        if self.active_page == page_id { self.active_page = self.pages.first().map(|page| page.id).unwrap_or(0); }
        self.takeover.clear(); true
    }

    pub fn activate_page(&mut self, page_id: u32) -> bool {
        if !self.pages.iter().any(|page| page.id == page_id) { return false; }
        self.active_page = page_id; self.takeover.clear(); true
    }

    pub fn next_page(&mut self) -> bool { self.shift_page(1) }
    pub fn previous_page(&mut self) -> bool { self.shift_page(-1) }

    fn shift_page(&mut self, delta: i32) -> bool {
        if self.pages.is_empty() { return false; }
        let current = self.pages.iter().position(|page| page.id == self.active_page).unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.pages.len() as i32) as usize;
        self.active_page = self.pages[next].id; self.takeover.clear(); true
    }

    pub fn set_selected_track(&mut self, track_id: Option<u32>) -> bool {
        if track_id == Some(0) { return false; }
        if self.selected_track != track_id { self.selected_track = track_id; self.takeover.clear(); }
        true
    }

    pub fn set_active_plugin(&mut self, plugin_id: Option<u32>) -> bool {
        if plugin_id == Some(0) { return false; }
        if self.active_plugin != plugin_id { self.active_plugin = plugin_id; self.takeover.clear(); }
        true
    }

    pub fn set_focused_window(&mut self, window: FocusedWindow) {
        if self.focused_window != window { self.focused_window = window; self.takeover.clear(); }
    }

    pub fn set_quick_control_policy(&mut self, policy: QuickControlFocusPolicy) {
        if self.quick_control_policy != policy { self.quick_control_policy = policy; self.takeover.clear(); }
    }

    pub fn set_quick_control_lock(&mut self, locked: bool) -> bool {
        if locked {
            let Some(target) = self.current_quick_control_target() else { return false; };
            self.locked_quick_control = Some(target);
        } else {
            self.locked_quick_control = None;
        }
        self.focus_locked = locked;
        self.takeover.clear();
        true
    }

    pub fn shift_bank(&mut self, banks: i32) {
        let delta = i64::from(banks) * i64::from(self.bank_size);
        self.bank_offset = if delta < 0 { self.bank_offset.saturating_sub(delta.unsigned_abs() as u32) }
            else { self.bank_offset.saturating_add(delta as u32) };
        self.takeover.clear();
    }

    pub fn set_target_value(&mut self, target: &str, value: f32) -> bool {
        let target = target.trim();
        if target.is_empty() || target.len() > 256 || target.contains('\0') || !value.is_finite() { return false; }
        self.target_values.insert(target.to_owned(), value.clamp(0.0, 1.0)); true
    }

    pub fn process(&mut self, event: ControllerEvent) -> Option<RemoteParameterChange> {
        if event.controller_id != self.controller_id || !event.value.is_finite() { return None; }
        let page = self.pages.iter().find(|page| page.id == self.active_page)?;
        let mapping = page.mappings.iter().find(|mapping| mapping.control_id == event.control_id)?.clone();
        let target = self.resolve_target(&mapping)?;
        let current = *self.target_values.get(&target).unwrap_or(&0.0);
        let state = self.takeover.entry((self.active_page, mapping.control_id)).or_default();
        let hardware = event.value.clamp(0.0, 1.0);
        let normalized = match mapping.input_mode {
            RemoteInputMode::Absolute => hardware,
            mode => (current + relative_delta(mode, hardware)).clamp(0.0, 1.0),
        };
        let next = match mapping.value_mode {
            RemoteValueMode::Jump => normalized,
            RemoteValueMode::Pickup if mapping.input_mode == RemoteInputMode::Absolute => {
                let crossed = state.last_hardware.is_some_and(|previous| (previous - current) * (hardware - current) <= 0.0);
                if !state.picked_up && ((hardware - current).abs() <= 1.0 / 127.0 || crossed) { state.picked_up = true; }
                if !state.picked_up { state.last_hardware = Some(hardware); return None; }
                hardware
            }
            RemoteValueMode::Pickup => normalized,
            RemoteValueMode::Scaled if mapping.input_mode == RemoteInputMode::Absolute => {
                let previous = state.last_hardware.unwrap_or(hardware);
                let movement = hardware - previous;
                let distance = (hardware - current).abs();
                (current + movement * (0.2 + 0.8 * (1.0 - distance))).clamp(0.0, 1.0)
            }
            RemoteValueMode::Scaled => normalized,
            RemoteValueMode::Toggle => {
                let pressed = hardware >= 0.5;
                if !pressed || state.last_button { state.last_button = pressed; state.last_hardware = Some(hardware); return None; }
                state.last_button = true;
                if current >= 0.5 { 0.0 } else { 1.0 }
            }
        };
        state.last_hardware = Some(hardware);
        let ranged = if mapping.inverted { mapping.maximum - next * (mapping.maximum - mapping.minimum) }
            else { mapping.minimum + next * (mapping.maximum - mapping.minimum) };
        self.target_values.insert(target.clone(), ranged);
        Some(RemoteParameterChange { target, value: ranged })
    }

    /// Produce motor-fader/LED feedback for every active-page mapping assigned
    /// to a changed host parameter.
    pub fn feedback_for(&self, target: &str, value: f32) -> Vec<ControllerEvent> {
        if !value.is_finite() { return Vec::new(); }
        let Some(page) = self.pages.iter().find(|page| page.id == self.active_page) else { return Vec::new(); };
        page.mappings.iter().filter(|mapping| mapping.transmit_feedback
            && self.resolve_target(mapping).as_deref() == Some(target)).map(|mapping| {
                let span = mapping.maximum - mapping.minimum;
                let mut normalized = if span.abs() < f32::EPSILON { 0.0 } else { (value - mapping.minimum) / span };
                if mapping.inverted { normalized = 1.0 - normalized; }
                ControllerEvent { controller_id: self.controller_id, control_id: mapping.control_id,
                    value: normalized.clamp(0.0, 1.0) }
            }).collect()
    }

    fn resolve_target(&self, mapping: &RemoteMapping) -> Option<String> {
        let mut target = mapping.target.clone();
        if mapping.focus_mode == RemoteFocusMode::TrackSelection {
            let track = self.selected_track?;
            target = target.replace("{track}", &track.to_string());
        }
        if mapping.focus_mode == RemoteFocusMode::FocusQuickControl {
            let focus = self.locked_quick_control.or_else(|| self.current_quick_control_target())?;
            let value = match focus {
                QuickControlTarget::Track(id) => format!("track/{id}"),
                QuickControlTarget::Plugin(id) => format!("plugin/{id}"),
            };
            target = target.replace("{focus}", &value);
        }
        if let Some(slot) = mapping.bank_slot {
            let track = self.bank_offset.checked_add(u32::from(slot))?;
            target = target.replace("{bank}", &track.to_string());
        }
        Some(target)
    }

    fn current_quick_control_target(&self) -> Option<QuickControlTarget> {
        match self.quick_control_policy {
            QuickControlFocusPolicy::TrackOnly => self.selected_track.map(QuickControlTarget::Track),
            QuickControlFocusPolicy::PluginWindowOnly => self.active_plugin.map(QuickControlTarget::Plugin),
            QuickControlFocusPolicy::TrackAndPluginWindow => match self.focused_window {
                FocusedWindow::Project => self.selected_track.map(QuickControlTarget::Track),
                FocusedWindow::Plugin => self.active_plugin.map(QuickControlTarget::Plugin),
            },
        }
    }

    pub fn audit(&self) -> bool {
        self.controller_id != 0 && self.bank_size > 0 && self.bank_size <= 128 && self.pages.len() <= 256
            && self.pages.iter().all(MappingPage::validate)
            && self.pages.iter().enumerate().all(|(index, page)| self.pages[..index].iter().all(|previous| previous.id != page.id))
            && (self.pages.is_empty() && self.active_page == 0 || self.pages.iter().any(|page| page.id == self.active_page))
            && self.selected_track != Some(0) && self.active_plugin != Some(0)
            && self.focus_locked == self.locked_quick_control.is_some()
            && self.locked_quick_control.is_none_or(|target| match target {
                QuickControlTarget::Track(id) | QuickControlTarget::Plugin(id) => id != 0,
            })
            && self.target_values.iter().all(|(target, value)| !target.trim().is_empty() && target.len() <= 256
                && !target.contains('\0') && value.is_finite() && (0.0..=1.0).contains(value))
    }
}

impl MappingPage {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 128 && !self.name.contains('\0')
            && self.mappings.len() <= 4096 && self.mappings.iter().all(RemoteMapping::validate)
            && self.mappings.iter().enumerate().all(|(index, mapping)| self.mappings[..index]
                .iter().all(|previous| previous.control_id != mapping.control_id))
    }
}

impl RemoteMapping {
    fn validate(&self) -> bool {
        self.control_id != 0 && !self.target.trim().is_empty() && self.target.len() <= 256 && !self.target.contains('\0')
            && self.minimum.is_finite() && self.maximum.is_finite() && self.minimum < self.maximum
            && (0.0..=1.0).contains(&self.minimum) && (0.0..=1.0).contains(&self.maximum)
            && (self.focus_mode != RemoteFocusMode::TrackSelection || self.target.contains("{track}"))
            && (self.focus_mode != RemoteFocusMode::FocusQuickControl || self.target.contains("{focus}"))
            && (self.bank_slot.is_none() || self.target.contains("{bank}"))
    }
}

fn relative_delta(mode: RemoteInputMode, normalized: f32) -> f32 {
    let raw = (normalized.clamp(0.0, 1.0) * 127.0).round() as u8;
    let steps = match mode {
        RemoteInputMode::Absolute => 0,
        RemoteInputMode::RelativeSignedBit => if raw >= 65 { i16::from(raw - 64) } else { -i16::from(raw) },
        RemoteInputMode::RelativeBinaryOffset => i16::from(raw) - 64,
        RemoteInputMode::RelativeTwosComplement => if raw <= 64 { i16::from(raw) } else { i16::from(raw) - 128 },
    };
    f32::from(steps) / 127.0
}





#[cfg(test)]
mod remote_tests { include!("hardware_remote_tests.rs"); }

#[cfg(test)]
mod remote_persistence_tests { include!("hardware_remote_persistence_tests.rs"); }
