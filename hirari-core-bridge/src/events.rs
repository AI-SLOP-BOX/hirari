pub enum HirariEventType {
    MeterUpdate,
    AnalysisComplete,
    StructuralChange,
    SystemError,
}

pub struct HirariEvent {
    pub event_type: HirariEventType,
    pub track_id: u32,
    pub value: f32,
    pub message: String,
}

pub struct EventOrchestrator {
    pub high_priority: Vec<HirariEvent>,
    pub low_priority: Vec<HirariEvent>,
}

impl HirariEvent {
    pub fn validate(&self) -> bool {
        self.value.is_finite() && self.message.len() <= 4096 && !self.message.contains('\0')
    }
}

impl Default for EventOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl EventOrchestrator {
    pub fn new() -> Self {
        Self {
            high_priority: Vec::new(),
            low_priority: Vec::new(),
        }
    }

    /// INDUSTRIAL: Pushes an event into the appropriate priority queue.
    pub fn push_event(&mut self, event: HirariEvent) -> bool {
        if !event.validate() {
            return false;
        }
        match event.event_type {
            HirariEventType::SystemError => {
                if self.high_priority.len() >= 65_536 {
                    return false;
                }
                self.high_priority.push(event);
            }
            _ => {
                if self.low_priority.len() >= 1_000_000 {
                    return false;
                }
                self.low_priority.push(event);
            }
        }
        true
    }

    pub fn pending_counts(&self) -> (usize, usize) {
        (self.high_priority.len(), self.low_priority.len())
    }

    /// INDUSTRIAL: Polls events, prioritizing high-priority messages.
    pub fn poll_events(&mut self) -> Vec<HirariEvent> {
        let mut out = std::mem::take(&mut self.high_priority);
        out.append(&mut std::mem::take(&mut self.low_priority));
        out
    }
}
