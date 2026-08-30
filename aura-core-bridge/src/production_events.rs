//! Versioned cross-domain event stream for GUI, VFX, CLI, and automation clients.
//!
//! Events are control-plane data: producers publish after a successful state
//! change, while consumers read through a cursor. Audio callbacks must not
//! allocate or publish directly; adapters should forward events from the
//! control thread.

use crate::production_timeline::MasterClock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const EVENT_API_VERSION: &str = "aura.events.v1";
const DEFAULT_CAPACITY: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ProductionEvent {
    TrackAdded {
        track_id: u32,
        name: String,
    },
    TrackRemoved {
        track_id: u32,
    },
    TrackGainChanged {
        track_id: u32,
        gain: f32,
    },
    RegionMoved {
        track_id: u32,
        region_id: u32,
        start_sample: u64,
    },
    PluginAdded {
        track_id: u32,
        plugin_index: u32,
        plugin: String,
    },
    AutomationChanged {
        target: String,
    },
    TempoMapChanged,
    MarkerChanged {
        marker_id: u32,
    },
    TransportStarted,
    TransportStopped,
    PlayheadMoved {
        clock: MasterClock,
    },
    Custom {
        domain: String,
        name: String,
        payload: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub api_version: &'static str,
    pub sequence: u64,
    pub generation: u64,
    pub event: ProductionEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCursor {
    /// The last sequence number successfully consumed by the client.
    pub after: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventBatch {
    pub events: Vec<EventEnvelope>,
    pub next: EventCursor,
    pub missed: bool,
    pub current_sequence: u64,
}

/// Bounded replayable event stream. A client that falls behind receives
/// `missed = true` and must refresh its snapshot before applying the batch.
#[derive(Debug, Clone)]
pub struct EventHub {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<EventEnvelope>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            next_sequence: 0,
            events: VecDeque::new(),
        }
    }

    pub fn publish(&mut self, generation: u64, event: ProductionEvent) -> EventEnvelope {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let envelope = EventEnvelope {
            api_version: EVENT_API_VERSION,
            sequence: self.next_sequence,
            generation,
            event,
        };
        self.events.push_back(envelope.clone());
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
        envelope
    }

    pub fn current_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn subscribe_from(&self, cursor: EventCursor, limit: usize) -> EventBatch {
        let first_available = self
            .events
            .front()
            .map(|event| event.sequence)
            .unwrap_or(self.next_sequence.saturating_add(1));
        let missed = cursor.after.saturating_add(1) < first_available;
        let events = self
            .events
            .iter()
            .filter(|event| event.sequence > cursor.after)
            .take(limit.max(1).min(4096))
            .cloned()
            .collect::<Vec<_>>();
        let next = EventCursor {
            after: events
                .last()
                .map(|event| event.sequence)
                .unwrap_or(cursor.after),
        };
        EventBatch {
            events,
            next,
            missed,
            current_sequence: self.next_sequence,
        }
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_replay_is_ordered_and_resumable() {
        let mut hub = EventHub::new(8);
        hub.publish(1, ProductionEvent::TransportStarted);
        hub.publish(
            2,
            ProductionEvent::TrackAdded {
                track_id: 7,
                name: "Synth".into(),
            },
        );
        let first = hub.subscribe_from(EventCursor { after: 0 }, 1);
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.next.after, 1);
        let second = hub.subscribe_from(first.next, 8);
        assert_eq!(second.events[0].sequence, 2);
        assert!(!second.missed);
    }

    #[test]
    fn bounded_history_reports_snapshot_recovery_when_client_lags() {
        let mut hub = EventHub::new(2);
        for track_id in 1..=3 {
            hub.publish(
                1,
                ProductionEvent::TrackAdded {
                    track_id,
                    name: track_id.to_string(),
                },
            );
        }
        let batch = hub.subscribe_from(EventCursor { after: 0 }, 8);
        assert!(batch.missed);
        assert_eq!(batch.events[0].sequence, 2);
        assert_eq!(batch.current_sequence, 3);
    }

    #[test]
    fn envelopes_are_stable_json() {
        let mut hub = EventHub::default();
        let event = hub.publish(42, ProductionEvent::MarkerChanged { marker_id: 9 });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("aura.events.v1"));
        assert!(json.contains("marker_changed"));
    }
}
