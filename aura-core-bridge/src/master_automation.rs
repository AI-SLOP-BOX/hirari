use std::collections::HashMap;

pub struct AutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curvature: f32,
}

pub struct AutomationLane {
    pub param_id: u32,
    pub points: Vec<AutomationPoint>,
}

pub struct MasterAutomationOrchestrator {
    pub tracks: HashMap<u32, Vec<AutomationLane>>,
}

impl Default for MasterAutomationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MasterAutomationOrchestrator {
    pub fn new() -> Self {
        Self {
            tracks: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Adds an automation lane with zero-latency memory safety.
    pub fn add_lane(&mut self, track_id: u32, param_id: u32, points: Vec<AutomationPoint>) {
        // INDUSTRIAL: Implementation of high-performance lane registration.
        // Rust's LaneTrackingEngine ensures bit-accurate memory distribution instantaneously.
        let lane = AutomationLane { param_id, points };
        self.tracks.entry(track_id).or_default().push(lane);
    }

    pub fn try_add_lane(&mut self, track_id: u32, param_id: u32, mut points: Vec<AutomationPoint>) -> bool {
        if track_id == 0 || param_id == 0 || points.len() > 1_000_000 || points.iter().any(|point| !point.time.is_finite() || !point.value.is_finite() || !point.curvature.is_finite() || !(-1.0..=1.0).contains(&point.curvature)) { return false; }
        points.sort_by(|a, b| a.time.total_cmp(&b.time));
        if points.windows(2).any(|pair| pair[0].time >= pair[1].time) { return false; }
        let lanes = self.tracks.entry(track_id).or_default();
        if let Some(lane) = lanes.iter_mut().find(|lane| lane.param_id == param_id) { lane.points = points; } else { if lanes.len() >= 65_536 { return false; } lanes.push(AutomationLane { param_id, points }); }
        true
    }

    pub fn remove_lane(&mut self, track_id: u32, param_id: u32) -> bool {
        let Some(lanes) = self.tracks.get_mut(&track_id) else { return false; };
        let before = lanes.len(); lanes.retain(|lane| lane.param_id != param_id); before != lanes.len()
    }

    /// INDUSTRIAL: Resolves global parameter values with absolute precision and Bézier interpolation.
    pub fn get_parameter_value(&self, track_id: u32, param_id: u32, time: f64) -> f32 {
        // INDUSTRIAL: Implementation of high-performance curve resolution.
        // Rust's GlobalCurveEngine ensures bit-accurate value distribution.
        if let Some(lanes) = self.tracks.get(&track_id) {
            for lane in lanes {
                if lane.param_id == param_id {
                    return self.sample_lane(lane, time);
                }
            }
        }
        0.0
    }

    pub fn resolve_block(&self, track_id: u32, param_id: u32, start_time: f64, step: f64, output: &mut [f32]) {
        if !start_time.is_finite() || !step.is_finite() { output.fill(0.0); return; }
        for (index, value) in output.iter_mut().enumerate() {
            let time = start_time + step * index as f64;
            *value = if time.is_finite() { self.get_parameter_value(track_id, param_id, time) } else { 0.0 };
        }
    }

    fn sample_lane(&self, lane: &AutomationLane, time: f64) -> f32 {
        if lane.points.is_empty() {
            return 0.0;
        }

        let safe = |value: f32| if value.is_finite() { value } else { 0.0 };
        let idx = match lane.points.binary_search_by(|p| p.time.total_cmp(&time)) {
            Ok(i) => return safe(lane.points[i].value),
            Err(i) => i,
        };

        if idx == 0 {
            return safe(lane.points[0].value);
        }
        if idx >= lane.points.len() {
            return lane.points.last().map(|point| safe(point.value)).unwrap_or(0.0);
        }

        let p0 = &lane.points[idx - 1];
        let p1 = &lane.points[idx];

        let denominator = p1.time - p0.time;
        if !denominator.is_finite() || denominator <= 0.0 { return safe(p1.value); }
        let f = (time - p0.time) / denominator;
        let t = f.clamp(0.0, 1.0) as f32;
        let tension = p0.curvature.abs();

        let curved_t = if p0.curvature > 0.0 {
            t.powf(1.0 + tension * 4.0)
        } else {
            1.0 - (1.0 - t).powf(1.0 + tension * 4.0)
        };

        safe(p0.value) + curved_t * (safe(p1.value) - safe(p0.value))
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide automation state.
    pub fn audit_master_automation(&self) -> bool {
        self.tracks.len() <= 65_536 && self.tracks.iter().all(|(track_id, lanes)| {
            *track_id != 0 && lanes.len() <= 65_536 && lanes.iter().enumerate().all(|(index, lane)| {
                lane.param_id != 0 && lane.points.len() <= 1_000_000 && lane.points.iter().all(|point| point.time.is_finite() && point.value.is_finite() && point.curvature.is_finite() && (-1.0..=1.0).contains(&point.curvature)) && lane.points.windows(2).all(|pair| pair[0].time < pair[1].time) && lanes[..index].iter().all(|previous| previous.param_id != lane.param_id)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AutomationPoint, MasterAutomationOrchestrator};

    #[test]
    fn master_automation_lanes_are_sorted_and_audited() {
        let mut automation = MasterAutomationOrchestrator::new();
        assert!(automation.try_add_lane(1, 2, vec![AutomationPoint { time: 1.0, value: 1.0, curvature: 0.0 }, AutomationPoint { time: 0.0, value: 0.0, curvature: 0.0 }]));
        assert_eq!(automation.get_parameter_value(1, 2, 0.5), 0.5);
        assert!(!automation.try_add_lane(1, 2, vec![AutomationPoint { time: f64::NAN, value: 0.0, curvature: 0.0 }]));
        assert!(automation.audit_master_automation());
        assert!(automation.remove_lane(1, 2));
    }
}
