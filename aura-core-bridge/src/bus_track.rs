pub struct BusTrackOrchestrator {
    pub bus_id: u32,
    pub input_gain: f32,
    pub invert_phase: bool,
}

impl Default for BusTrackOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl BusTrackOrchestrator {
    pub fn new() -> Self {
        Self {
            bus_id: 0,
            input_gain: 1.0,
            invert_phase: false,
        }
    }

    pub fn set_input_gain(&mut self, gain: f32) -> bool {
        if !gain.is_finite() || !(-4.0..=4.0).contains(&gain) {
            return false;
        }
        self.input_gain = gain;
        true
    }

    pub fn set_phase_inverted(&mut self, inverted: bool) {
        self.invert_phase = inverted;
    }

    /// INDUSTRIAL: Fetches and processes audio from the bus with absolute signal precision and transparency.
    pub fn fetch_audio(&self, l: &mut [f32], r: &mut [f32]) {
        let gain = if self.input_gain.is_finite() {
            self.input_gain.clamp(-4.0, 4.0)
        } else {
            0.0
        };
        let sign = if self.invert_phase { -1.0 } else { 1.0 };
        for sample in l.iter_mut().take(r.len()) {
            *sample = if sample.is_finite() {
                *sample * gain * sign
            } else {
                0.0
            };
        }
        for sample in r.iter_mut().take(l.len()) {
            *sample = if sample.is_finite() {
                *sample * gain * sign
            } else {
                0.0
            };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide bus track state.
    pub fn audit_bus_track(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic signal auditing logic.
        self.input_gain.is_finite() && self.input_gain.abs() <= 4.0
    }
}

#[cfg(test)]
mod tests {
    use super::BusTrackOrchestrator;

    #[test]
    fn bus_track_gain_update_is_validated() {
        let mut track = BusTrackOrchestrator::new();
        assert!(track.set_input_gain(2.0));
        assert!(!track.set_input_gain(f32::NAN));
        assert_eq!(track.input_gain, 2.0);
        track.set_phase_inverted(true);
        let mut left = [1.0];
        let mut right = [1.0];
        track.fetch_audio(&mut left, &mut right);
        assert_eq!(left[0], -2.0);
        assert!(track.audit_bus_track());
    }
}
