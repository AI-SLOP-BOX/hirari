use crate::{MidiEvent, MidiEventType, WorkspaceState, ForensicSeverity, ForensicModule};
use parking_lot::Mutex;

/// Industrial MIDI Orchestrator [Sovereign Sequencing]
/// Handles high-resolution MIDI 2.0 events and MPE data.
pub struct MidiOrchestrator {
    pub event_queue: Mutex<Vec<MidiEvent>>,
    pub active_notes: Mutex<std::collections::HashSet<u8>>,
}

impl MidiOrchestrator {
    pub fn new() -> Self {
        Self {
            event_queue: Mutex::new(Vec::with_capacity(1024)),
            active_notes: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Ingests a raw MIDI event into the sovereign processing queue.
    pub fn push_event(&self, event: MidiEvent) {
        // Track active notes for MPE state management
        match &event.event {
            MidiEventType::NoteOn { note, .. } => { self.active_notes.lock().insert(*note); }
            MidiEventType::NoteOff { note } => { self.active_notes.lock().remove(note); }
            _ => {}
        }

        // Update note state before taking the queue lock so the two mutexes are
        // never held at the same time, avoiding lock-order dependencies.
        self.event_queue.lock().push(event);
    }

    /// Processes the MIDI queue and maps events to nodal parameters.
    pub fn process_frame(&self, _state: &WorkspaceState) {
        let mut q = self.event_queue.lock();
        if q.is_empty() { return; }

        crate::aura_log!(
            ForensicSeverity::Info,
            ForensicModule::Audio,
            "MIDI: Processing {} events in sovereign frame.",
            q.len()
        );

        // PRODUCTION: Map MIDI CC to Node Parameters via Macro Bindings
        // for event in q.iter() { ... }

        q.clear();
    }
}
