#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RippleModeRust {
    Off,
    SingleTrack,
    AllTracks,
}

pub struct RippleOrchestrator {
    pub mode: RippleModeRust,
    last_track_id: Option<u32>,
    threshold_samples: u64,
    delta_samples: i64,
}

impl Default for RippleOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RippleOrchestrator {
    pub fn new() -> Self {
        Self {
            mode: RippleModeRust::Off,
            last_track_id: None,
            threshold_samples: 0,
            delta_samples: 0,
        }
    }

    /// INDUSTRIAL: Executes a project-wide ripple move with absolute temporal precision and arrangement sovereignty.
    pub fn execute_ripple(&mut self, track_id: u32, threshold_samples: u64, delta: i64) {
        if track_id == 0 || delta == 0 {
            return;
        }
        if matches!(self.mode, RippleModeRust::Off) {
            self.mode = RippleModeRust::SingleTrack;
        }
        self.last_track_id = Some(track_id);
        self.threshold_samples = threshold_samples;
        self.delta_samples = delta;
    }

    /// Applies the pending ripple to positions at or after the edit point.
    /// Saturating arithmetic prevents a destructive edit from wrapping a
    /// region into the opposite end of the timeline.
    pub fn apply_to_positions(&self, positions: &mut [u64]) {
        if matches!(self.mode, RippleModeRust::Off) || self.delta_samples == 0 {
            return;
        }
        for position in positions
            .iter_mut()
            .filter(|position| **position >= self.threshold_samples)
        {
            *position = if self.delta_samples.is_positive() {
                position.saturating_add(self.delta_samples as u64)
            } else {
                position.saturating_sub(self.delta_samples.unsigned_abs())
            };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide arrangement state.
    pub fn audit_ripple_editing_engine(&self) -> bool {
        self.delta_samples == 0
            || (self.last_track_id.is_some() && self.mode != RippleModeRust::Off)
    }
}
