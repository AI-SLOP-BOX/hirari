//! Hardware remote-control mapping engine: value modes, focus policies,
//! mapping pages and the soft-takeover runtime, split out of hardware.rs.
use super::*;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteValueMode {
    Jump,
    Pickup,
    Scaled,
    Toggle,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteInputMode {
    Absolute,
    RelativeSignedBit,
    RelativeBinaryOffset,
    RelativeTwosComplement,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RemoteFocusMode {
    Fixed,
    TrackSelection,
    FocusQuickControl,
}

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
pub enum QuickControlTarget {
    Track(u32),
    Plugin(u32),
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum MappingScope {
    Global,
    Project,
}

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
pub struct RemoteParameterChange {
    pub target: String,
    pub value: f32,
}

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
        if !self.audit() {
            return Err("invalid MIDI Remote state".into());
        }
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
        if value.audit() {
            Ok(value)
        } else {
            Err("invalid MIDI Remote state".into())
        }
    }

    pub fn new(controller_id: u32, bank_size: u16) -> Option<Self> {
        if controller_id == 0 || bank_size == 0 || bank_size > 128 {
            return None;
        }
        Some(Self {
            controller_id,
            pages: Vec::new(),
            active_page: 0,
            selected_track: None,
            active_plugin: None,
            quick_control_policy: QuickControlFocusPolicy::default(),
            focused_window: FocusedWindow::default(),
            locked_quick_control: None,
            focus_locked: false,
            bank_offset: 0,
            bank_size,
            target_values: std::collections::BTreeMap::new(),
            takeover: std::collections::BTreeMap::new(),
        })
    }

    pub fn upsert_page(&mut self, mut page: MappingPage) -> bool {
        page.name = page.name.trim().to_owned();
        if !page.validate() {
            return false;
        }
        if let Some(existing) = self.pages.iter_mut().find(|item| item.id == page.id) {
            if existing.factory {
                return false;
            }
            *existing = page;
        } else if self.pages.len() < 256 {
            self.pages.push(page);
        } else {
            return false;
        }
        self.pages.sort_by_key(|item| item.id);
        if self.active_page == 0 {
            self.active_page = self.pages[0].id;
        }
        self.takeover.clear();
        true
    }

    pub fn remove_page(&mut self, page_id: u32) -> bool {
        if self
            .pages
            .iter()
            .find(|page| page.id == page_id)
            .is_none_or(|page| page.factory)
        {
            return false;
        }
        self.pages.retain(|page| page.id != page_id);
        if self.active_page == page_id {
            self.active_page = self.pages.first().map(|page| page.id).unwrap_or(0);
        }
        self.takeover.clear();
        true
    }

    pub fn activate_page(&mut self, page_id: u32) -> bool {
        if !self.pages.iter().any(|page| page.id == page_id) {
            return false;
        }
        self.active_page = page_id;
        self.takeover.clear();
        true
    }

    pub fn next_page(&mut self) -> bool {
        self.shift_page(1)
    }
    pub fn previous_page(&mut self) -> bool {
        self.shift_page(-1)
    }

    fn shift_page(&mut self, delta: i32) -> bool {
        if self.pages.is_empty() {
            return false;
        }
        let current = self
            .pages
            .iter()
            .position(|page| page.id == self.active_page)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.pages.len() as i32) as usize;
        self.active_page = self.pages[next].id;
        self.takeover.clear();
        true
    }

    pub fn set_selected_track(&mut self, track_id: Option<u32>) -> bool {
        if track_id == Some(0) {
            return false;
        }
        if self.selected_track != track_id {
            self.selected_track = track_id;
            self.takeover.clear();
        }
        true
    }

    pub fn set_active_plugin(&mut self, plugin_id: Option<u32>) -> bool {
        if plugin_id == Some(0) {
            return false;
        }
        if self.active_plugin != plugin_id {
            self.active_plugin = plugin_id;
            self.takeover.clear();
        }
        true
    }

    pub fn set_focused_window(&mut self, window: FocusedWindow) {
        if self.focused_window != window {
            self.focused_window = window;
            self.takeover.clear();
        }
    }

    pub fn set_quick_control_policy(&mut self, policy: QuickControlFocusPolicy) {
        if self.quick_control_policy != policy {
            self.quick_control_policy = policy;
            self.takeover.clear();
        }
    }

    pub fn set_quick_control_lock(&mut self, locked: bool) -> bool {
        if locked {
            let Some(target) = self.current_quick_control_target() else {
                return false;
            };
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
        self.bank_offset = if delta < 0 {
            self.bank_offset.saturating_sub(delta.unsigned_abs() as u32)
        } else {
            self.bank_offset.saturating_add(delta as u32)
        };
        self.takeover.clear();
    }

    pub fn set_target_value(&mut self, target: &str, value: f32) -> bool {
        let target = target.trim();
        if target.is_empty() || target.len() > 256 || target.contains('\0') || !value.is_finite() {
            return false;
        }
        self.target_values
            .insert(target.to_owned(), value.clamp(0.0, 1.0));
        true
    }

    pub fn process(&mut self, event: ControllerEvent) -> Option<RemoteParameterChange> {
        if event.controller_id != self.controller_id || !event.value.is_finite() {
            return None;
        }
        let page = self.pages.iter().find(|page| page.id == self.active_page)?;
        let mapping = page
            .mappings
            .iter()
            .find(|mapping| mapping.control_id == event.control_id)?
            .clone();
        let target = self.resolve_target(&mapping)?;
        let current = *self.target_values.get(&target).unwrap_or(&0.0);
        let state = self
            .takeover
            .entry((self.active_page, mapping.control_id))
            .or_default();
        let hardware = event.value.clamp(0.0, 1.0);
        let normalized = match mapping.input_mode {
            RemoteInputMode::Absolute => hardware,
            mode => (current + relative_delta(mode, hardware)).clamp(0.0, 1.0),
        };
        let next = match mapping.value_mode {
            RemoteValueMode::Jump => normalized,
            RemoteValueMode::Pickup if mapping.input_mode == RemoteInputMode::Absolute => {
                let crossed = state
                    .last_hardware
                    .is_some_and(|previous| (previous - current) * (hardware - current) <= 0.0);
                if !state.picked_up && ((hardware - current).abs() <= 1.0 / 127.0 || crossed) {
                    state.picked_up = true;
                }
                if !state.picked_up {
                    state.last_hardware = Some(hardware);
                    return None;
                }
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
                if !pressed || state.last_button {
                    state.last_button = pressed;
                    state.last_hardware = Some(hardware);
                    return None;
                }
                state.last_button = true;
                if current >= 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
        };
        state.last_hardware = Some(hardware);
        let ranged = if mapping.inverted {
            mapping.maximum - next * (mapping.maximum - mapping.minimum)
        } else {
            mapping.minimum + next * (mapping.maximum - mapping.minimum)
        };
        self.target_values.insert(target.clone(), ranged);
        Some(RemoteParameterChange {
            target,
            value: ranged,
        })
    }

    /// Produce motor-fader/LED feedback for every active-page mapping assigned
    /// to a changed host parameter.
    pub fn feedback_for(&self, target: &str, value: f32) -> Vec<ControllerEvent> {
        if !value.is_finite() {
            return Vec::new();
        }
        let Some(page) = self.pages.iter().find(|page| page.id == self.active_page) else {
            return Vec::new();
        };
        page.mappings
            .iter()
            .filter(|mapping| {
                mapping.transmit_feedback && self.resolve_target(mapping).as_deref() == Some(target)
            })
            .map(|mapping| {
                let span = mapping.maximum - mapping.minimum;
                let mut normalized = if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    (value - mapping.minimum) / span
                };
                if mapping.inverted {
                    normalized = 1.0 - normalized;
                }
                ControllerEvent {
                    controller_id: self.controller_id,
                    control_id: mapping.control_id,
                    value: normalized.clamp(0.0, 1.0),
                }
            })
            .collect()
    }

    fn resolve_target(&self, mapping: &RemoteMapping) -> Option<String> {
        let mut target = mapping.target.clone();
        if mapping.focus_mode == RemoteFocusMode::TrackSelection {
            let track = self.selected_track?;
            target = target.replace("{track}", &track.to_string());
        }
        if mapping.focus_mode == RemoteFocusMode::FocusQuickControl {
            let focus = self
                .locked_quick_control
                .or_else(|| self.current_quick_control_target())?;
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
            QuickControlFocusPolicy::TrackOnly => {
                self.selected_track.map(QuickControlTarget::Track)
            }
            QuickControlFocusPolicy::PluginWindowOnly => {
                self.active_plugin.map(QuickControlTarget::Plugin)
            }
            QuickControlFocusPolicy::TrackAndPluginWindow => match self.focused_window {
                FocusedWindow::Project => self.selected_track.map(QuickControlTarget::Track),
                FocusedWindow::Plugin => self.active_plugin.map(QuickControlTarget::Plugin),
            },
        }
    }

    pub fn audit(&self) -> bool {
        self.controller_id != 0
            && self.bank_size > 0
            && self.bank_size <= 128
            && self.pages.len() <= 256
            && self.pages.iter().all(MappingPage::validate)
            && self.pages.iter().enumerate().all(|(index, page)| {
                self.pages[..index]
                    .iter()
                    .all(|previous| previous.id != page.id)
            })
            && (self.pages.is_empty() && self.active_page == 0
                || self.pages.iter().any(|page| page.id == self.active_page))
            && self.selected_track != Some(0)
            && self.active_plugin != Some(0)
            && self.focus_locked == self.locked_quick_control.is_some()
            && self.locked_quick_control.is_none_or(|target| match target {
                QuickControlTarget::Track(id) | QuickControlTarget::Plugin(id) => id != 0,
            })
            && self.target_values.iter().all(|(target, value)| {
                !target.trim().is_empty()
                    && target.len() <= 256
                    && !target.contains('\0')
                    && value.is_finite()
                    && (0.0..=1.0).contains(value)
            })
    }
}

impl MappingPage {
    fn validate(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.mappings.len() <= 4096
            && self.mappings.iter().all(RemoteMapping::validate)
            && self.mappings.iter().enumerate().all(|(index, mapping)| {
                self.mappings[..index]
                    .iter()
                    .all(|previous| previous.control_id != mapping.control_id)
            })
    }
}

impl RemoteMapping {
    fn validate(&self) -> bool {
        self.control_id != 0
            && !self.target.trim().is_empty()
            && self.target.len() <= 256
            && !self.target.contains('\0')
            && self.minimum.is_finite()
            && self.maximum.is_finite()
            && self.minimum < self.maximum
            && (0.0..=1.0).contains(&self.minimum)
            && (0.0..=1.0).contains(&self.maximum)
            && (self.focus_mode != RemoteFocusMode::TrackSelection
                || self.target.contains("{track}"))
            && (self.focus_mode != RemoteFocusMode::FocusQuickControl
                || self.target.contains("{focus}"))
            && (self.bank_slot.is_none() || self.target.contains("{bank}"))
    }
}

fn relative_delta(mode: RemoteInputMode, normalized: f32) -> f32 {
    let raw = (normalized.clamp(0.0, 1.0) * 127.0).round() as u8;
    let steps = match mode {
        RemoteInputMode::Absolute => 0,
        RemoteInputMode::RelativeSignedBit => {
            if raw >= 65 {
                i16::from(raw - 64)
            } else {
                -i16::from(raw)
            }
        }
        RemoteInputMode::RelativeBinaryOffset => i16::from(raw) - 64,
        RemoteInputMode::RelativeTwosComplement => {
            if raw <= 64 {
                i16::from(raw)
            } else {
                i16::from(raw) - 128
            }
        }
    };
    f32::from(steps) / 127.0
}

#[cfg(test)]
mod remote_tests {
    include!("hardware_remote_tests.rs");
}

#[cfg(test)]
mod remote_persistence_tests {
    include!("hardware_remote_persistence_tests.rs");
}
