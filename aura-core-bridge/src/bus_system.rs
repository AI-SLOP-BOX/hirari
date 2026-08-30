pub struct BusRust {
    pub id: u32,
    pub data_l: Vec<f32>,
    pub data_r: Vec<f32>,
    pub gain_db: f32,
    pub pan: f32,
}

pub struct BusSystemOrchestrator {
    pub buses: Vec<BusRust>,
    pub active_order: Vec<u32>,
}

#[cfg(test)]
mod tests {
    use super::BusSystemOrchestrator;

    #[test]
    fn bus_order_and_removal_are_validated() {
        let mut buses = BusSystemOrchestrator::new();
        assert!(!buses.add_bus(0));
        assert!(buses.add_bus(1));
        assert!(buses.add_bus(2));
        assert!(!buses.set_active_order(vec![1, 1]));
        assert!(buses.set_active_order(vec![2, 1]));
        assert!(buses.remove_bus(2));
        assert_eq!(buses.active_order, vec![1]);
        assert!(buses.audit_bus_system());
    }

    #[test]
    fn active_bus_mixdown_follows_console_order_without_mutating_sources() {
        let mut buses = BusSystemOrchestrator::new();
        assert!(buses.add_bus(1));
        assert!(buses.add_bus(2));
        buses.add_samples(1, &[0.25, 0.5], &[0.1, 0.2]);
        buses.add_samples(2, &[0.5, 0.25], &[0.2, 0.1]);
        let before = buses.buses[0].data_l.clone();
        let (left, right) = buses.mix_active_busses(2).unwrap();
        assert_eq!(left, vec![0.75, 0.75]);
        assert_eq!(right, vec![0.3, 0.3]);
        assert_eq!(buses.buses[0].data_l, before);
    }

    #[test]
    fn bus_fader_and_balance_are_applied_at_mixdown() {
        let mut buses = BusSystemOrchestrator::new();
        assert!(buses.add_bus(1));
        buses.add_samples(1, &[1.0], &[1.0]);
        assert!(buses.set_bus_gain_db(1, -6.0));
        assert!(buses.set_bus_pan(1, 1.0));
        let (left, right) = buses.mix_active_busses(1).unwrap();
        assert_eq!(left[0], 0.0);
        assert!((right[0] - 10.0f32.powf(-6.0 / 20.0)).abs() < 1e-5);
        assert!(!buses.set_bus_pan(1, f32::NAN));
    }
}

impl Default for BusSystemOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl BusSystemOrchestrator {
    pub fn new() -> Self {
        Self {
            buses: Vec::new(),
            active_order: Vec::new(),
        }
    }

    pub fn add_bus(&mut self, bus_id: u32) -> bool {
        if bus_id == 0 || self.buses.len() >= 65_536 || self.buses.iter().any(|bus| bus.id == bus_id) {
            return false;
        }
        self.buses.push(BusRust {
            id: bus_id,
            data_l: Vec::new(),
            data_r: Vec::new(),
            gain_db: 0.0,
            pan: 0.0,
        });
        self.active_order.push(bus_id);
        true
    }

    pub fn remove_bus(&mut self, bus_id: u32) -> bool {
        let Some(index) = self.buses.iter().position(|bus| bus.id == bus_id) else { return false; };
        self.buses.remove(index);
        self.active_order.retain(|id| *id != bus_id);
        true
    }

    pub fn set_active_order(&mut self, order: Vec<u32>) -> bool {
        let ids: std::collections::HashSet<u32> = self.buses.iter().map(|bus| bus.id).collect();
        if order.len() != ids.len() || order.iter().any(|id| !ids.contains(id)) || order.windows(2).any(|pair| pair[0] == pair[1]) { return false; }
        self.active_order = order;
        true
    }

    pub fn set_bus_gain_db(&mut self, bus_id: u32, gain_db: f32) -> bool {
        if !gain_db.is_finite() || !(-120.0..=24.0).contains(&gain_db) { return false; }
        let Some(bus) = self.buses.iter_mut().find(|bus| bus.id == bus_id) else { return false; };
        bus.gain_db = gain_db;
        true
    }

    pub fn set_bus_pan(&mut self, bus_id: u32, pan: f32) -> bool {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) { return false; }
        let Some(bus) = self.buses.iter_mut().find(|bus| bus.id == bus_id) else { return false; };
        bus.pan = pan;
        true
    }

    pub fn clear(&mut self) {
        for bus in &mut self.buses {
            bus.data_l.fill(0.0);
            bus.data_r.fill(0.0);
        }
    }

    /// INDUSTRIAL: Adds samples to a bus with absolute summing precision and mixing sovereignty.
    pub fn add_samples(&mut self, bus_id: u32, l: &[f32], r: &[f32]) {
        let Some(bus) = self.buses.iter_mut().find(|bus| bus.id == bus_id) else {
            return;
        };
        if l.len() != r.len() {
            return;
        }
        if bus.data_l.len() < l.len() {
            bus.data_l.resize(l.len(), 0.0);
            bus.data_r.resize(l.len(), 0.0);
        }
        for (index, (&left, &right)) in l.iter().zip(r).enumerate() {
            let left = if left.is_finite() { left } else { 0.0 };
            let right = if right.is_finite() { right } else { 0.0 };
            bus.data_l[index] = (bus.data_l[index] + left).clamp(-16.0, 16.0);
            bus.data_r[index] = (bus.data_r[index] + right).clamp(-16.0, 16.0);
        }
    }

    /// INDUSTRIAL: Processes the signal graph with absolute graph precision and mixing sovereignty.
    pub fn process(&mut self, len: u32) {
        let len = (len as usize).min(4_194_304);
        for bus in &mut self.buses {
            if bus.data_l.len() < len {
                bus.data_l.resize(len, 0.0);
            }
            if bus.data_r.len() < len {
                bus.data_r.resize(len, 0.0);
            }
            bus.data_l.truncate(len);
            bus.data_r.truncate(len);
        }
    }

    /// Mixes active buses in MixConsole order into a bounded stereo master
    /// buffer without mutating the individual bus buffers.
    pub fn mix_active_busses(&self, len: usize) -> Option<(Vec<f32>, Vec<f32>)> {
        if len > 4_194_304 || !self.audit_bus_system() { return None; }
        let mut left = vec![0.0f32; len];
        let mut right = vec![0.0f32; len];
        for bus_id in &self.active_order {
            let bus = self.buses.iter().find(|bus| bus.id == *bus_id)?;
            let gain = 10.0f32.powf(bus.gain_db.clamp(-120.0, 24.0) / 20.0);
            // Balance law: center preserves the original stereo level while
            // hard-left/right safely attenuate the opposite side.
            let pan = bus.pan.clamp(-1.0, 1.0);
            let left_gain = gain * (1.0 - pan).min(1.0);
            let right_gain = gain * (1.0 + pan).min(1.0);
            for index in 0..len.min(bus.data_l.len()) {
                left[index] = (left[index] + bus.data_l[index] * left_gain).clamp(-16.0, 16.0);
                right[index] = (right[index] + bus.data_r[index] * right_gain).clamp(-16.0, 16.0);
            }
        }
        Some((left, right))
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal summing state.
    pub fn audit_bus_system(&self) -> bool {
        let unique_ids = self
            .buses
            .iter()
            .map(|bus| bus.id)
            .collect::<std::collections::HashSet<_>>();
        unique_ids.len() == self.buses.len()
            && self.active_order.iter().all(|id| unique_ids.contains(id))
            && self.active_order.len() == unique_ids.len()
            && self.buses.iter().all(|bus| {
                    bus.data_l.len() == bus.data_r.len()
                    && bus.gain_db.is_finite() && (-120.0..=24.0).contains(&bus.gain_db)
                    && bus.pan.is_finite() && (-1.0..=1.0).contains(&bus.pan)
                    && bus.data_l.iter().chain(bus.data_r.iter()).all(|sample| sample.is_finite())
            })
    }
}
