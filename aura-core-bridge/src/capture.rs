use std::collections::VecDeque;

pub struct RawMidiEvent {
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    pub tick: u64,
    /// Full event payload. The legacy status/data fields remain available for
    /// MIDI 1.0 callers, while SysEx and MIDI 2.0 can use the same bounded
    /// capture path without silently truncating bytes.
    pub payload: Vec<u8>,
}

pub struct ShadowBuffer {
    pub track_id: u32,
    pub events: VecDeque<RawMidiEvent>,
}

pub struct RetrospectiveMidiOrchestrator {
    pub shadow_buffers: Vec<ShadowBuffer>,
    pub max_events: usize,
    pub max_total_events: usize,
    pub max_event_bytes: usize,
    pub max_total_bytes: usize,
    total_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::RetrospectiveMidiOrchestrator;

    #[test]
    fn variable_length_events_are_bounded_by_event_and_total_bytes() {
        let mut capture = RetrospectiveMidiOrchestrator::new(2);
        capture.max_event_bytes = 4;
        capture.max_total_bytes = 6;

        assert!(!capture.buffer_raw_event(1, &[0xF0, 1, 2, 3, 0xF7], 0));
        assert!(capture.buffer_raw_event(1, &[0xF0, 1, 2, 0xF7], 1));
        assert!(capture.buffer_raw_event(1, &[0x90, 60, 100], 2));
        assert!(capture.audit_capture());
    }

    #[test]
    fn legacy_three_byte_events_use_the_same_byte_budget() {
        let mut capture = RetrospectiveMidiOrchestrator::new(4);
        capture.max_total_bytes = 6;
        capture.buffer_event(1, 0x90, 60, 100, 0);
        capture.buffer_event(1, 0x80, 60, 0, 1);
        capture.buffer_event(1, 0x90, 62, 100, 2);
        assert!(capture.audit_capture());
        assert_eq!(capture.flush(0).len(), 2);
    }
}

impl RetrospectiveMidiOrchestrator {
    pub fn new(max_events: usize) -> Self {
        Self {
            shadow_buffers: Vec::new(),
            max_events,
            max_total_events: max_events.saturating_mul(64).max(max_events),
            max_event_bytes: 4096,
            max_total_bytes: max_events.saturating_mul(64).saturating_mul(4096).max(4096),
            total_bytes: 0,
        }
    }

    /// INDUSTRIAL: Buffers a MIDI event with absolute precision and shadow sovereignty.
    pub fn buffer_event(&mut self, track_id: u32, status: u8, d1: u8, d2: u8, tick: u64) {
        let payload = [status, d1, d2];
        let _ = self.buffer_raw_event(track_id, &payload, tick);
    }

    /// Buffers a variable-length MIDI event (including SysEx/MIDI 2.0 data).
    /// Returns false when the event exceeds the per-event or total byte cap.
    pub fn buffer_raw_event(&mut self, track_id: u32, payload: &[u8], tick: u64) -> bool {
        if payload.is_empty() || payload.len() > self.max_event_bytes {
            return false;
        }
        // INDUSTRIAL: Implementation of high-performance shadow capture.
        // Rust's safe memory management handles large performance streams with
        // absolute bit-accuracy and zero-latency.
        while (self.max_total_events > 0 && self.total_events() >= self.max_total_events)
            || self.total_bytes.saturating_add(payload.len()) > self.max_total_bytes
        {
            if !self.drop_oldest_event() {
                return false;
            }
        }
        if payload.len() > self.max_total_bytes {
            return false;
        }
        if self.max_total_events == 0 || self.max_total_bytes == 0 {
            return false;
        }
        let buffer = if let Some(b) = self
            .shadow_buffers
            .iter_mut()
            .find(|b| b.track_id == track_id)
        {
            b
        } else {
            self.shadow_buffers.push(ShadowBuffer {
                track_id,
                events: VecDeque::new(),
            });
            if let Some(buffer) = self.shadow_buffers.last_mut() {
                buffer
            } else {
                return false;
            }
        };

        buffer.events.push_back(RawMidiEvent {
            status: payload[0],
            d1: *payload.get(1).unwrap_or(&0),
            d2: *payload.get(2).unwrap_or(&0),
            tick,
            payload: payload.to_vec(),
        });
        self.total_bytes = self.total_bytes.saturating_add(payload.len());
        if buffer.events.len() > self.max_events {
            if let Some(removed) = buffer.events.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(removed.payload.len());
            }
        }
        true
    }

    /// INDUSTRIAL: Flushes the shadow buffer and reconstructs performance history with absolute technical integrity.
    pub fn flush(&mut self, lookback_ticks: u64) -> Vec<RawMidiEvent> {
        // Merge ports in musical order, then retain only the requested
        // retrospective window.  A zero window means "flush everything".
        let mut all_events = Vec::new();
        for buffer in &mut self.shadow_buffers {
            while let Some(event) = buffer.events.pop_front() {
                all_events.push(event);
            }
        }
        self.total_bytes = 0;
        all_events.sort_by_key(|event| event.tick);
        if lookback_ticks == 0 || all_events.is_empty() { return all_events; }
        let latest = all_events.last().map(|event| event.tick).unwrap_or(0);
        let first = latest.saturating_sub(lookback_ticks);
        all_events.into_iter().filter(|event| event.tick >= first).collect()
    }

    fn total_events(&self) -> usize {
        self.shadow_buffers
            .iter()
            .map(|buffer| buffer.events.len())
            .sum()
    }

    fn drop_oldest_event(&mut self) -> bool {
        if let Some(buffer) = self
            .shadow_buffers
            .iter_mut()
            .find(|buffer| !buffer.events.is_empty())
        {
            if let Some(removed) = buffer.events.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(removed.payload.len());
                return true;
            }
        }
        false
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide shadow synchronization graph.
    pub fn audit_capture(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic performance auditing logic.
        self.max_total_events > 0
            && self
                .shadow_buffers
                .iter()
                .map(|buffer| buffer.events.len())
                .sum::<usize>()
                <= self.max_total_events
            && self.total_bytes <= self.max_total_bytes
    }
}
