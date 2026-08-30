pub enum AuraEventType {
    MeterUpdate,
    AnalysisComplete,
    StructuralChange,
    SystemError,
}

pub struct AuraEvent {
    pub event_type: AuraEventType,
    pub track_id: u32,
    pub value: f32,
    pub message: String,
}

pub struct EventOrchestrator {
    pub high_priority: Vec<AuraEvent>,
    pub low_priority: Vec<AuraEvent>,
}

impl AuraEvent {
    pub fn validate(&self) -> bool { self.value.is_finite() && self.message.len() <= 4096 && !self.message.contains('\0') }
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
    pub fn push_event(&mut self, event: AuraEvent) -> bool {
        if !event.validate() { return false; }
        match event.event_type {
            AuraEventType::SystemError => { if self.high_priority.len() >= 65_536 { return false; } self.high_priority.push(event); }
            _ => { if self.low_priority.len() >= 1_000_000 { return false; } self.low_priority.push(event); }
        }
        true
    }

    pub fn pending_counts(&self) -> (usize, usize) { (self.high_priority.len(), self.low_priority.len()) }

    /// INDUSTRIAL: Polls events, prioritizing high-priority messages.
    pub fn poll_events(&mut self) -> Vec<AuraEvent> {
        let mut out = std::mem::take(&mut self.high_priority);
        out.append(&mut std::mem::take(&mut self.low_priority));
        out
    }
}
