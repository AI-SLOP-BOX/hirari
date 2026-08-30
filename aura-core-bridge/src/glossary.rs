//! Shared beginner-facing terminology used by UI and automation clients.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub term: &'static str,
    pub explanation: &'static str,
    pub related_operations: &'static [&'static str],
}

pub fn entries() -> Vec<GlossaryEntry> {
    vec![
        GlossaryEntry { term: "Track", explanation: "A lane that contains audio or MIDI you want to edit.", related_operations: &["add_track", "set_track_name"] },
        GlossaryEntry { term: "Bus", explanation: "A shared channel that combines several tracks for group processing.", related_operations: &["set_route", "add_plugin"] },
        GlossaryEntry { term: "Aux", explanation: "A return channel used for shared effects such as reverb or delay.", related_operations: &["add_aux_track", "set_route"] },
        GlossaryEntry { term: "Automation", explanation: "A recorded parameter change that moves over time.", related_operations: &["set_automation", "set_volume"] },
        GlossaryEntry { term: "Stem", explanation: "An exported group of related tracks as its own audio file.", related_operations: &["bounce_stems"] },
        GlossaryEntry { term: "Latency", explanation: "The delay between an input action and hearing its result.", related_operations: &["set_low_latency_mode"] },
    ]
}

#[cfg(test)]
mod tests {
    use super::entries;

    #[test]
    fn glossary_is_stable_and_actionable() {
        let values = entries();
        assert!(values.len() >= 6);
        assert!(values.iter().all(|entry| !entry.term.is_empty()
            && !entry.explanation.is_empty()
            && !entry.related_operations.is_empty()));
    }
}
