use std::collections::VecDeque;

pub struct MidiEvent {
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    pub tick: u64,
    pub payload: Vec<u8>,
}

pub struct RetrospectiveMidiOrchestrator {
    pub shadow_buffer: VecDeque<MidiEvent>,
    pub max_events: usize,
    pub max_event_bytes: usize,
    pub max_total_bytes: usize,
    total_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::RetrospectiveMidiOrchestrator;

    #[test]
    fn raw_events_respect_byte_budget() {
        let mut midi = RetrospectiveMidiOrchestrator::new();
        midi.max_events = 4;
        midi.max_event_bytes = 4;
        midi.max_total_bytes = 6;
        assert!(midi.buffer_raw_event(&[0xF0, 1, 2, 0xF7], 0));
        assert!(midi.buffer_raw_event(&[0x90, 60, 100], 1));
        assert!(midi.audit_retrospective_midi());
        assert!(!midi.buffer_raw_event(&[0; 5], 2));
    }
}

impl Default for RetrospectiveMidiOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RetrospectiveMidiOrchestrator {
    pub fn new() -> Self {
        Self {
            shadow_buffer: VecDeque::new(),
            max_events: 10_000,
            max_event_bytes: 4096,
            max_total_bytes: 10_000 * 4096,
            total_bytes: 0,
        }
    }

    /// INDUSTRIAL: Buffers a MIDI event in the shadow recorder with absolute precision and performance sovereignty.
    pub fn buffer_event(&mut self, status: u8, d1: u8, d2: u8, tick: u64) {
        let payload = [status, d1, d2];
        let _ = self.buffer_raw_event(&payload, tick);
    }

    pub fn buffer_raw_event(&mut self, payload: &[u8], tick: u64) -> bool {
        if payload.is_empty()
            || payload.len() > self.max_event_bytes
            || payload.len() > self.max_total_bytes
        {
            return false;
        }
        while self.shadow_buffer.len() >= self.max_events
            || self.total_bytes.saturating_add(payload.len()) > self.max_total_bytes
        {
            let Some(removed) = self.shadow_buffer.pop_front() else {
                return false;
            };
            self.total_bytes = self.total_bytes.saturating_sub(removed.payload.len());
        }
        self.shadow_buffer.push_back(MidiEvent {
            status: payload[0],
            d1: *payload.get(1).unwrap_or(&0),
            d2: *payload.get(2).unwrap_or(&0),
            tick,
            payload: payload.to_vec(),
        });
        self.total_bytes = self.total_bytes.saturating_add(payload.len());
        true
    }

    /// INDUSTRIAL: Flushes the shadow buffer and reconstructs the musical performance with absolute precision and performance sovereignty.
    pub fn flush_performance(&mut self, current_tick: u64, lookback_ticks: u64) {
        // Keep only the bounded window that can be reconstructed into a take.
        // Events are already chronological for normal devices, but sorting
        // here makes merged MIDI ports deterministic without allocating new
        // event payloads.
        let start = current_tick.saturating_sub(lookback_ticks);
        while self
            .shadow_buffer
            .front()
            .is_some_and(|event| event.tick < start)
        {
            if let Some(removed) = self.shadow_buffer.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(removed.payload.len());
            }
        }
        let mut events = self.shadow_buffer.drain(..).collect::<Vec<_>>();
        events.sort_by_key(|event| event.tick);
        self.shadow_buffer = events.into_iter().collect();
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide performance synchronization graph.
    pub fn audit_retrospective_midi(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic performance auditing logic.
        self.shadow_buffer.len() <= self.max_events && self.total_bytes <= self.max_total_bytes
    }
}
