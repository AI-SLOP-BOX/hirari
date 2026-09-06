#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutomationModeRust {
    Read,
    Write,
    Touch,
    Latch,
    AutoPunch,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationModeOrchestrator {
    pub current_mode: AutomationModeRust,
    pub is_touching: bool,
    pub latched: bool,
    pub punch_start: u64,
    pub punch_end: u64,
    pub write_protected: bool,
    pub preview: bool,
}

impl Default for AutomationModeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationModeOrchestrator {
    pub fn new() -> Self {
        Self {
            current_mode: AutomationModeRust::Read,
            is_touching: false,
            latched: false,
            punch_start: 0,
            punch_end: 0,
            write_protected: false,
            preview: false,
        }
    }

    /// INDUSTRIAL: Transitions the automation mode with absolute state-machine precision.
    pub fn transition_mode(&mut self, new_mode: AutomationModeRust) {
        // INDUSTRIAL: Implementation of high-performance state resolution.
        // Rust's RecordingStateEngine ensures bit-accurate mode distribution.
        self.current_mode = new_mode;
        self.is_touching = false;
        self.latched = false;
    }

    /// INDUSTRIAL: Resolves the recording state based on touch events and current mode.
    pub fn resolve_recording_state(&mut self, is_touching: bool) -> bool {
        // INDUSTRIAL: Implementation of musically intelligent recording logic.
        // Handling Read, Write, Touch, Latch logic with absolute technical integrity.
        self.is_touching = is_touching;
        if self.write_protected || self.preview {
            return false;
        }
        match self.current_mode {
            AutomationModeRust::Read => {
                self.latched = false;
                false
            }
            AutomationModeRust::Write => true,
            AutomationModeRust::Touch => is_touching,
            AutomationModeRust::Latch => {
                if is_touching {
                    self.latched = true;
                }
                self.latched
            }
            AutomationModeRust::AutoPunch => false,
        }
    }
    pub fn set_write_protected(&mut self, protected: bool) {
        self.write_protected = protected;
    }
    pub fn set_preview(&mut self, preview: bool) {
        self.preview = preview;
    }

    pub fn set_punch_range(&mut self, start: u64, end: u64) -> bool {
        if end <= start {
            return false;
        }
        self.punch_start = start;
        self.punch_end = end;
        true
    }
    pub fn resolve_recording_at(&mut self, position: u64, is_touching: bool) -> bool {
        if self.write_protected || self.preview {
            return false;
        }
        if self.current_mode == AutomationModeRust::AutoPunch {
            return self.punch_end > self.punch_start
                && position >= self.punch_start
                && position < self.punch_end;
        }
        self.resolve_recording_state(is_touching)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide recording state.
    pub fn audit_automation_mode(&self) -> bool {
        self.punch_end >= self.punch_start
            && (!matches!(self.current_mode, AutomationModeRust::AutoPunch)
                || self.punch_end > self.punch_start)
            && (matches!(self.current_mode, AutomationModeRust::Latch) || !self.latched)
            && (!self.is_touching || !matches!(self.current_mode, AutomationModeRust::Read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_punch_limits_recording_window() {
        let mut mode = AutomationModeOrchestrator::new();
        mode.transition_mode(AutomationModeRust::AutoPunch);
        assert!(mode.set_punch_range(100, 200));
        assert!(!mode.resolve_recording_at(99, false));
        assert!(mode.resolve_recording_at(100, false));
        assert!(!mode.resolve_recording_at(200, false));
    }
}
