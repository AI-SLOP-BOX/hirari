use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessibilityState {
    pub high_contrast: bool,
    pub screen_reader: bool,
    pub focused_id: Option<String>,
    #[serde(default)]
    announcements: Vec<String>,
}
impl AccessibilityState {
    pub fn set_focus(&mut self, id: Option<&str>) -> bool {
        if id
            .map(|v| v.trim().is_empty() || v.len() > 256 || v.contains('\0'))
            .unwrap_or(false)
        {
            return false;
        }
        self.focused_id = id.map(|value| value.trim().to_owned());
        true
    }
    pub fn set_high_contrast(&mut self, enabled: bool) {
        self.high_contrast = enabled;
    }
    pub fn set_screen_reader(&mut self, enabled: bool) {
        self.screen_reader = enabled;
    }
    pub fn announce(&mut self, message: &str) -> bool {
        if message.trim().is_empty()
            || message.len() > 1024
            || message.contains('\0')
            || self.announcements.len() >= 1024
        {
            return false;
        }
        if self.screen_reader {
            self.announcements.push(message.trim().to_owned());
        }
        true
    }
    pub fn drain_announcements(&mut self) -> Vec<String> {
        std::mem::take(&mut self.announcements)
    }
    pub fn validate(&self) -> bool {
        self.focused_id
            .as_ref()
            .map(|v| !v.is_empty() && v.len() <= 256 && !v.contains('\0'))
            .unwrap_or(true)
            && self.announcements.len() <= 1024
            && self.announcements.iter().all(|message| {
                !message.trim().is_empty() && message.len() <= 1024 && !message.contains('\0')
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_is_bounded() {
        let mut s = AccessibilityState::default();
        assert!(s.set_focus(Some("mixer")));
        assert!(s.validate());
    }
    #[test]
    fn screen_reader_announcements_are_queued_only_when_enabled() {
        let mut state = AccessibilityState::default();
        assert!(state.announce("ignored"));
        assert!(state.drain_announcements().is_empty());
        state.set_screen_reader(true);
        assert!(state.announce("Mixer ready"));
        assert_eq!(state.drain_announcements(), vec!["Mixer ready"]);
        assert!(state.validate());
    }
}
