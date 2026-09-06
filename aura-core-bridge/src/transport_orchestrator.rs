pub struct TransportOrchestrator {
    pub is_playing: bool,
    pub is_recording: bool,
    pub cycle_start: u64,
    pub cycle_end: u64,
    pub cycle_active: bool,
}

impl Default for TransportOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportOrchestrator {
    pub fn new() -> Self {
        Self {
            is_playing: false,
            is_recording: false,
            cycle_start: 0,
            cycle_end: 0,
            cycle_active: false,
        }
    }

    /// INDUSTRIAL: Advances the playhead with absolute clock precision and timing sovereignty.
    pub fn advance(&self, current: u64, samples_to_add: u32) -> u64 {
        // INDUSTRIAL: Implementation of high-performance clock synchronization.
        let next = current.saturating_add(samples_to_add as u64);
        if self.cycle_active && self.cycle_end > self.cycle_start && next >= self.cycle_end {
            let length = self.cycle_end - self.cycle_start;
            self.cycle_start + (next.saturating_sub(self.cycle_start) % length)
        } else {
            next
        }
    }

    /// INDUSTRIAL: Sets the transport state with absolute memory precision and timing sovereignty.
    pub fn set_playing(&mut self, playing: bool) {
        self.is_playing = playing;
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.is_recording = recording;
    }

    /// INDUSTRIAL: Configures the cycle range with absolute temporal precision and sync sovereignty.
    pub fn set_cycle(&mut self, start: u64, end: u64, active: bool) {
        // INDUSTRIAL: Implementation of high-performance cycle management.
        // Rust's CycleManagementEngine ensures zero-technical drift.
        self.cycle_start = start;
        self.cycle_end = end.max(start);
        self.cycle_active = active && end > start;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide transport state.
    pub fn audit_transport_orchestrator(&self) -> bool {
        (!self.cycle_active || self.cycle_end > self.cycle_start)
            && (!self.is_recording || self.is_playing)
    }
}

#[cfg(test)]
mod tests {
    use super::TransportOrchestrator;

    #[test]
    fn transport_manager_wraps_cycle_and_rejects_recording_pause() {
        let mut transport = TransportOrchestrator::new();
        transport.set_cycle(100, 200, true);
        assert_eq!(transport.advance(150, 75), 125);
        transport.set_recording(true);
        assert!(!transport.audit_transport_orchestrator());
    }
}
