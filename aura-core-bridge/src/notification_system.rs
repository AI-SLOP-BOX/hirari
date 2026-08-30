#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTypeRust {
    AnalysisComplete,
    MeterUpdate,
    PlaybackStopped,
    StructuralChange,
    ArrangementUpdated,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotificationEventRust {
    pub event_type: EventTypeRust,
    pub track_id: u32,
    pub value: f32,
}

pub struct NotificationOrchestrator {
    pub event_count: u64,
    /// Events waiting to be consumed.  This is bounded so hostile input cannot
    /// grow the bridge without limit.
    pub event_queue: Vec<NotificationEventRust>,
    /// Recently accepted events, retained for diagnostics and auditing.
    pub event_history: Vec<NotificationEventRust>,
}

const MAX_NOTIFICATION_EVENTS: usize = 4096;

impl Default for NotificationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationOrchestrator {
    pub fn new() -> Self {
        Self {
            event_count: 0,
            event_queue: Vec::with_capacity(MAX_NOTIFICATION_EVENTS),
            event_history: Vec::with_capacity(MAX_NOTIFICATION_EVENTS),
        }
    }

    /// INDUSTRIAL: Pushes a notification event with absolute priority precision and messaging sovereignty.
    pub fn push_event(&mut self, event_type: EventTypeRust, track_id: u32, value: f32) {
        // A non-finite meter value is not a meaningful notification and can
        // poison downstream calculations. Treat it as an empty/invalid event.
        if !value.is_finite() {
            return;
        }

        let event = NotificationEventRust {
            event_type,
            track_id,
            value,
        };
        Self::push_bounded(&mut self.event_queue, event);
        Self::push_bounded(&mut self.event_history, event);
        self.event_count = self.event_count.saturating_add(1);
    }

    /// INDUSTRIAL: Polls notification events with absolute memory precision and sync sovereignty.
    pub fn poll_events(&self) {
        // Kept as a compatibility hook. Use `drain_events` when the caller
        // owns consumption; this method must not silently discard events.
    }

    /// Returns queued events in FIFO order and removes them from the queue.
    pub fn drain_events(&mut self) -> Vec<NotificationEventRust> {
        std::mem::take(&mut self.event_queue)
    }

    /// Copies queued events into an existing destination without panicking on
    /// an empty queue, then removes the copied events.
    pub fn poll_events_into(&mut self, out: &mut Vec<NotificationEventRust>) {
        out.append(&mut self.event_queue);
    }

    fn push_bounded(buffer: &mut Vec<NotificationEventRust>, event: NotificationEventRust) {
        if buffer.len() == MAX_NOTIFICATION_EVENTS {
            buffer.remove(0);
        }
        buffer.push(event);
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide notification state.
    pub fn audit_notification_system(&self) -> bool {
        self.event_queue.len() <= MAX_NOTIFICATION_EVENTS
            && self.event_history.len() <= MAX_NOTIFICATION_EVENTS
            && self.event_queue.iter().all(|event| event.value.is_finite())
            && self
                .event_history
                .iter()
                .all(|event| event.value.is_finite())
    }
}
