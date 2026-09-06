use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiMonitorEvent {
    pub timestamp: u64,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}
impl MidiMonitorEvent {
    pub fn validate(&self) -> bool {
        self.status & 0x80 != 0 && self.data1 < 128 && self.data2 < 128
    }
    pub fn channel(&self) -> Option<u8> {
        ((self.status & 0xF0) >= 0x80 && (self.status & 0xF0) <= 0xE0).then_some(self.status & 0x0F)
    }
    pub fn is_realtime(&self) -> bool {
        matches!(self.status, 0xF8..=0xFF)
    }
    pub fn raw_bytes(&self) -> [u8; 3] {
        [self.status, self.data1, self.data2]
    }
    pub fn message_type(&self) -> Option<&'static str> {
        match self.status & 0xF0 {
            0x80 => Some("note_off"),
            0x90 => Some(if self.data2 == 0 {
                "note_off"
            } else {
                "note_on"
            }),
            0xA0 => Some("poly_pressure"),
            0xB0 => Some("control_change"),
            0xC0 => Some("program_change"),
            0xD0 => Some("channel_pressure"),
            0xE0 => Some("pitch_bend"),
            _ => None,
        }
    }
}
pub struct MidiMonitor {
    events: VecDeque<MidiMonitorEvent>,
    capacity: usize,
}
impl Default for MidiMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiMonitor {
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(4096),
            capacity: 4096,
        }
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn set_capacity(&mut self, capacity: usize) -> bool {
        if !(1..=1_000_000).contains(&capacity) {
            return false;
        }
        self.capacity = capacity;
        while self.events.len() > capacity {
            self.events.pop_front();
        }
        true
    }
    pub fn push(&mut self, e: MidiMonitorEvent) {
        if !e.validate()
            || self
                .events
                .back()
                .is_some_and(|last| e.timestamp < last.timestamp)
        {
            return;
        }
        if self.events.len() >= self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }
    pub fn clear(&mut self) {
        self.events.clear();
    }
    pub fn clear_before(&mut self, timestamp: u64) {
        while self
            .events
            .front()
            .is_some_and(|event| event.timestamp < timestamp)
        {
            self.events.pop_front();
        }
    }
    pub fn snapshot(&self, count: usize) -> Vec<MidiMonitorEvent> {
        let skip = self.events.len().saturating_sub(count);
        self.events.iter().skip(skip).cloned().collect()
    }
    pub fn channel_events(&self, channel: u8) -> Vec<MidiMonitorEvent> {
        if channel >= 16 {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|event| event.channel() == Some(channel))
            .cloned()
            .collect()
    }
    pub fn event_count(&self) -> usize {
        self.events.len()
    }
    pub fn export_csv(&self) -> String {
        let mut out = String::from("timestamp,status,data1,data2\n");
        for event in &self.events {
            out.push_str(&format!(
                "{},{},{},{}\n",
                event.timestamp, event.status, event.data1, event.data2
            ));
        }
        out
    }
    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.events.iter().collect::<Vec<_>>())
            .map_err(|error| error.to_string())
    }
    pub fn import_json(&mut self, json: &str) -> Result<usize, String> {
        let events: Vec<MidiMonitorEvent> =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        if events.iter().any(|event| !event.validate())
            || events.windows(2).any(|w| w[0].timestamp > w[1].timestamp)
        {
            return Err("invalid MIDI monitor data".into());
        }
        self.events.clear();
        for event in events {
            self.push(event);
        }
        Ok(self.events.len())
    }
    pub fn import_csv(&mut self, csv: &str) -> Result<usize, String> {
        let mut events = Vec::new();
        for (index, line) in csv.lines().enumerate() {
            if index == 0
                && line
                    .trim()
                    .eq_ignore_ascii_case("timestamp,status,data1,data2")
            {
                continue;
            }
            let fields: Vec<_> = line.split(',').collect();
            if fields.len() != 4 {
                return Err("invalid MIDI CSV".into());
            }
            events.push(MidiMonitorEvent {
                timestamp: fields[0].trim().parse().map_err(|_| "invalid timestamp")?,
                status: fields[1].trim().parse().map_err(|_| "invalid status")?,
                data1: fields[2].trim().parse().map_err(|_| "invalid data1")?,
                data2: fields[3].trim().parse().map_err(|_| "invalid data2")?,
            });
        }
        let json = serde_json::to_string(&events).map_err(|e| e.to_string())?;
        self.import_json(&json)
    }
    pub fn events_between(&self, start: u64, end: u64) -> Vec<MidiMonitorEvent> {
        if end < start {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|e| (start..=end).contains(&e.timestamp))
            .cloned()
            .collect()
    }
    pub fn latest_by_channel(&self, channel: u8) -> Option<MidiMonitorEvent> {
        if channel >= 16 {
            return None;
        }
        self.events
            .iter()
            .rev()
            .find(|e| e.channel() == Some(channel))
            .cloned()
    }
    pub fn filter(
        &self,
        status_mask: u8,
        status_value: u8,
        start: u64,
        end: u64,
    ) -> Vec<MidiMonitorEvent> {
        if end < start {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|e| {
                e.timestamp >= start
                    && e.timestamp <= end
                    && (e.status & status_mask) == status_value
            })
            .cloned()
            .collect()
    }
    pub fn message_events(&self, kind: &str) -> Vec<MidiMonitorEvent> {
        let needle = kind.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|e| e.message_type() == Some(needle.as_str()))
            .cloned()
            .collect()
    }
    pub fn realtime_events(&self) -> Vec<MidiMonitorEvent> {
        self.events
            .iter()
            .filter(|event| event.is_realtime())
            .cloned()
            .collect()
    }
    pub fn audit(&self) -> bool {
        self.capacity > 0
            && self.capacity <= 1_000_000
            && self.events.len() <= self.capacity
            && self.events.iter().all(MidiMonitorEvent::validate)
            && self
                .events
                .iter()
                .zip(self.events.iter().skip(1))
                .all(|(a, b)| a.timestamp <= b.timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_latest_events_in_order() {
        let mut monitor = MidiMonitor::new();
        monitor.push(MidiMonitorEvent {
            timestamp: 1,
            status: 0x90,
            data1: 60,
            data2: 100,
        });
        monitor.push(MidiMonitorEvent {
            timestamp: 2,
            status: 0x80,
            data1: 60,
            data2: 0,
        });
        assert_eq!(monitor.snapshot(1)[0].timestamp, 2);
        monitor.clear();
        assert!(monitor.snapshot(10).is_empty());
        monitor.push(MidiMonitorEvent {
            timestamp: 3,
            status: 0x91,
            data1: 64,
            data2: 80,
        });
        assert_eq!(monitor.channel_events(1).len(), 1);
        assert!(monitor
            .export_csv()
            .starts_with("timestamp,status,data1,data2\n"));
        let json = monitor.export_json().unwrap();
        let mut restored = MidiMonitor::new();
        assert_eq!(restored.import_json(&json).unwrap(), 1);
        assert!(restored
            .import_json("[{\"timestamp\":2,\"status\":0,\"data1\":0,\"data2\":0}]")
            .is_err());
        assert!(restored.import_json("[{\"timestamp\":2,\"status\":144,\"data1\":60,\"data2\":1},{\"timestamp\":1,\"status\":144,\"data1\":60,\"data2\":1}]").is_err());
        assert_eq!(
            restored
                .import_csv("timestamp,status,data1,data2\n1,144,60,100\n")
                .unwrap(),
            1
        );
        monitor.clear_before(3);
        assert_eq!(monitor.event_count(), 1);
    }
}
