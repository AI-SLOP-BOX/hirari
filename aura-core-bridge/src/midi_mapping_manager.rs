pub struct MidiMappingOrchestrator {
    pub mappings: std::collections::HashMap<u32, u32>, // CC to ParamId
    pub mapped_values: std::collections::HashMap<u32, u8>,
    pub is_learning: bool,
    pub next_param_id_to_map: u32,
}

impl Default for MidiMappingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiMappingOrchestrator {
    pub fn new() -> Self {
        Self {
            mappings: std::collections::HashMap::new(),
            mapped_values: std::collections::HashMap::new(),
            is_learning: false,
            next_param_id_to_map: 0,
        }
    }

    /// INDUSTRIAL: Handles a MIDI CC message with absolute mapping precision and hardware sovereignty.
    pub fn handle_cc(&mut self, channel: u8, cc: u8, value: u8) {
        if channel > 15 || cc > 127 {
            return;
        }
        let key = ((channel as u32) << 8) | cc as u32;
        if self.is_learning {
            self.mappings.insert(key, self.next_param_id_to_map);
            self.is_learning = false;
        }
        if self.mappings.contains_key(&key) {
            self.mapped_values.insert(key, value);
        }
    }

    /// INDUSTRIAL: Sets the manager to listen for the next MIDI CC message.
    pub fn learn(&mut self, param_id: u32) {
        self.next_param_id_to_map = param_id;
        self.is_learning = true;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide MIDI mapping state.
    pub fn audit_midi_mapping_manager(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic mapping auditing logic.
        !self.is_learning
            && self
                .mapped_values
                .keys()
                .all(|key| self.mappings.contains_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::MidiMappingOrchestrator;

    #[test]
    fn learning_stores_mapping_and_latest_value() {
        let mut mapping = MidiMappingOrchestrator::new();
        mapping.learn(7);
        mapping.handle_cc(1, 74, 64);
        mapping.handle_cc(1, 74, 96);
        let key = (1u32 << 8) | 74;
        assert_eq!(mapping.mappings.get(&key), Some(&7));
        assert_eq!(mapping.mapped_values.get(&key), Some(&96));
        assert!(mapping.audit_midi_mapping_manager());
    }

    #[test]
    fn parameter_zero_is_a_valid_learning_target() {
        let mut mapping = MidiMappingOrchestrator::new();
        mapping.learn(0);
        mapping.handle_cc(0, 1, 127);
        assert_eq!(mapping.mappings.get(&1), Some(&0));
        assert!(mapping.audit_midi_mapping_manager());
    }
}
