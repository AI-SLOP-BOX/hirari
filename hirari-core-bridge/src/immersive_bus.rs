pub struct SpatialRoutingEngine {
    pub max_busses: usize,
    pub channels_per_bus: usize,
}

pub struct ImmersiveBusOrchestrator {
    pub routing_engine: SpatialRoutingEngine,
    allocated_busses: Vec<bool>,
    object_positions: std::collections::HashMap<u32, [f32; 3]>,
    object_gains: std::collections::HashMap<u32, f32>,
}

impl Default for ImmersiveBusOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ImmersiveBusOrchestrator {
    pub fn new() -> Self {
        Self {
            routing_engine: SpatialRoutingEngine {
                max_busses: 1024,
                channels_per_bus: 12,
            },
            allocated_busses: vec![false; 1024],
            object_positions: std::collections::HashMap::new(),
            object_gains: std::collections::HashMap::new(),
        }
    }

    /// Allocates one logical immersive bus. Allocation is idempotent for the
    /// same owner-facing bus ID and rejects IDs outside the project contract.
    pub fn resolve_bus_allocation(&mut self, bus_id: u32) -> bool {
        let index = bus_id as usize;
        if index >= self.routing_engine.max_busses || index >= self.allocated_busses.len() {
            return false;
        }
        if self.allocated_busses[index] {
            return false;
        }
        self.allocated_busses[index] = true;
        true
    }

    /// Releases an immersive bus and makes its channel budget available to a
    /// later routing graph rebuild.
    pub fn release_bus(&mut self, bus_id: u32) -> bool {
        let index = bus_id as usize;
        let Some(allocated) = self.allocated_busses.get_mut(index) else {
            return false;
        };
        if !*allocated {
            return false;
        }
        *allocated = false;
        true
    }

    pub fn is_bus_allocated(&self, bus_id: u32) -> bool {
        self.allocated_busses
            .get(bus_id as usize)
            .copied()
            .unwrap_or(false)
    }
    pub fn set_object_position(&mut self, object_id: u32, position: [f32; 3]) -> bool {
        if object_id == 0
            || position
                .iter()
                .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
        {
            return false;
        }
        self.object_positions.insert(object_id, position);
        self.object_gains.entry(object_id).or_insert(1.0);
        true
    }
    pub fn object_position(&self, object_id: u32) -> Option<[f32; 3]> {
        self.object_positions.get(&object_id).copied()
    }
    pub fn set_object_gain(&mut self, object_id: u32, gain: f32) -> bool {
        if !gain.is_finite()
            || !(0.0..=4.0).contains(&gain)
            || !self.object_positions.contains_key(&object_id)
        {
            return false;
        }
        self.object_gains.insert(object_id, gain);
        true
    }
    pub fn object_gain(&self, object_id: u32) -> f32 {
        self.object_gains.get(&object_id).copied().unwrap_or(1.0)
    }
    pub fn remove_object(&mut self, object_id: u32) -> bool {
        let removed = self.object_positions.remove(&object_id).is_some();
        self.object_gains.remove(&object_id);
        removed
    }

    /// Render one positioned object into the canonical 7.1.4 channel order.
    /// The spatial math stays in a reusable processor while this bus owns the
    /// object identity and gain lookup.
    pub fn pan_object_714(&self, object_id: u32, input: f32, output: &mut [f32; 12]) -> bool {
        let Some(position) = self.object_positions.get(&object_id).copied() else {
            output.fill(0.0);
            return false;
        };
        let gain = self.object_gain(object_id);
        SpatialOrchestrator::new().pan_714(
            position[0],
            position[1],
            position[2],
            input * gain,
            output,
        );
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide spatial routing graph.
    pub fn audit_immersive_bus(&self) -> bool {
        self.routing_engine.max_busses == self.allocated_busses.len()
            && self.routing_engine.max_busses > 0
            && (1..=64).contains(&self.routing_engine.channels_per_bus)
            && self.object_positions.len() <= 65_536
            && self
                .object_positions
                .values()
                .all(|p| p.iter().all(|v| v.is_finite() && (-1.0..=1.0).contains(v)))
            && self.object_gains.len() == self.object_positions.len()
            && self.object_gains.iter().all(|(id, gain)| {
                self.object_positions.contains_key(id)
                    && gain.is_finite()
                    && (0.0..=4.0).contains(gain)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::ImmersiveBusOrchestrator;

    #[test]
    fn immersive_bus_allocation_is_bounded_and_releasable() {
        let mut buses = ImmersiveBusOrchestrator::new();
        assert!(buses.audit_immersive_bus());
        assert!(buses.resolve_bus_allocation(7));
        assert!(buses.is_bus_allocated(7));
        assert!(!buses.resolve_bus_allocation(7));
        assert!(buses.release_bus(7));
        assert!(!buses.is_bus_allocated(7));
        assert!(!buses.resolve_bus_allocation(1024));
        assert!(!buses.release_bus(1024));
    }

    #[test]
    fn immersive_object_gain_is_bound_to_positioned_object() {
        let mut buses = ImmersiveBusOrchestrator::new();
        assert!(!buses.set_object_gain(1, 0.5));
        assert!(buses.set_object_position(1, [0.0, 0.0, 0.0]));
        assert!(buses.set_object_gain(1, 0.5));
        assert_eq!(buses.object_gain(1), 0.5);
        assert!(buses.remove_object(1));
        assert!(buses.audit_immersive_bus());
    }

    #[test]
    fn positioned_object_renders_to_714_and_missing_object_silences() {
        let mut buses = ImmersiveBusOrchestrator::new();
        let mut output = [9.0; 12];
        assert!(!buses.pan_object_714(7, 1.0, &mut output));
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert!(buses.set_object_position(7, [1.0, 1.0, 0.0]));
        assert!(buses.set_object_gain(7, 0.5));
        assert!(buses.pan_object_714(7, 1.0, &mut output));
        assert!(output[1] > 0.49);
        assert!(output
            .iter()
            .enumerate()
            .all(|(index, sample)| index == 1 || sample.abs() < 1.0e-6));
    }
}
use crate::spatial_orchestrator::SpatialOrchestrator;
