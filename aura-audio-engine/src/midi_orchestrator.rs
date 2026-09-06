use crate::{WorkspaceState, MidiEvent, MidiEventType};
use crossbeam_queue::SegQueue;

pub struct MidiOrchestrator {
    pub event_queue: SegQueue<MidiEvent>,
    pub active_notes: std::collections::HashSet<u8>,
}

impl MidiOrchestrator {
    pub fn new() -> Self {
        Self {
            event_queue: SegQueue::new(),
            active_notes: std::collections::HashSet::new(),
        }
    }
    pub fn push_event(&self, event: MidiEvent) {
        self.event_queue.push(event);
    }
    pub fn process_frame(&mut self, _state: &WorkspaceState) {
        while let Some(event) = self.event_queue.pop() {
            match event.event {
                MidiEventType::NoteOn { note, velocity: _ } => {
                    self.active_notes.insert(note);
                }
                MidiEventType::NoteOff { note } => {
                    self.active_notes.remove(&note);
                }
                _ => {}
            }
        }
    }
}
