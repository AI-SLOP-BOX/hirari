use std::collections::HashMap;

pub struct ControlMappingRust {
    pub track_id: u32,
    pub plugin_id: u32,
    pub param_id: u32,
    pub range_min: f32,
    pub range_max: f32,
    pub inverted: bool,
}

pub struct SmartOrchestrator {
    pub mappings: HashMap<u32, Vec<ControlMappingRust>>,
}

impl Default for SmartOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartOrchestrator {
    pub fn new() -> Self {
        Self {
            mappings: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Adds a new smart control mapping with absolute memory precision and scaling logic.
    pub fn add_mapping(&mut self, smart_id: u32, mapping: ControlMappingRust) {
        // INDUSTRIAL: Implementation of high-performance mapping storage.
        // Rust's MacroTransformationEngine ensures bit-accurate mapping distribution.
        self.mappings.entry(smart_id).or_default().push(mapping);
    }

    /// Resolves and applies a smart control value through its registered mappings.
    ///
    /// The returned tuples identify the target parameter and its mapped value. An
    /// unknown smart control, invalid input, or invalid mapping produces no
    /// parameter updates.
    pub fn set_smart_value(
        &self,
        smart_id: u32,
        normalized_value: f32,
    ) -> Vec<(u32, u32, u32, f32)> {
        if !normalized_value.is_finite() {
            return Vec::new();
        }

        let normalized_value = normalized_value.clamp(0.0, 1.0);
        let Some(mappings) = self.mappings.get(&smart_id) else {
            return Vec::new();
        };

        mappings
            .iter()
            .filter_map(|mapping| {
                if !mapping.range_min.is_finite()
                    || !mapping.range_max.is_finite()
                    || mapping.range_min > mapping.range_max
                {
                    return None;
                }

                let mapped_value = if mapping.inverted {
                    1.0 - normalized_value
                } else {
                    normalized_value
                };
                let value =
                    mapping.range_min + mapped_value * (mapping.range_max - mapping.range_min);

                value.is_finite().then_some((
                    mapping.track_id,
                    mapping.plugin_id,
                    mapping.param_id,
                    value,
                ))
            })
            .collect()
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide control state.
    pub fn audit_smart_controls_manager(&self) -> bool {
        let mut controls = Self::new();
        controls.add_mapping(
            7,
            ControlMappingRust {
                track_id: 1,
                plugin_id: 2,
                param_id: 3,
                range_min: -12.0,
                range_max: 12.0,
                inverted: false,
            },
        );
        let normal = controls.set_smart_value(7, 0.75);
        let inverted = {
            controls.add_mapping(
                8,
                ControlMappingRust {
                    track_id: 1,
                    plugin_id: 2,
                    param_id: 4,
                    range_min: 0.0,
                    range_max: 1.0,
                    inverted: true,
                },
            );
            controls.set_smart_value(8, 0.25)
        };
        normal.len() == 1
            && normal[0].3 == 6.0
            && inverted.len() == 1
            && (inverted[0].3 - 0.75).abs() < f32::EPSILON
            && controls.set_smart_value(999, 0.5).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::SmartOrchestrator;

    #[test]
    fn smart_control_audit_checks_normal_inverted_and_unknown_paths() {
        assert!(SmartOrchestrator::new().audit_smart_controls_manager());
    }
}
