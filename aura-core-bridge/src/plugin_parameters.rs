use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PluginParameter {
    pub id: u32,
    pub name: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub automatable: bool,
    pub read_only: bool,
}
impl PluginParameter {
    pub fn validate(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && self.unit.len() <= 64
            && !self.unit.contains('\0')
            && self.min.is_finite()
            && self.max.is_finite()
            && self.min < self.max
            && self.default.is_finite()
            && (self.min..=self.max).contains(&self.default)
    }
    pub fn clamp(&self, value: f64) -> Option<f64> {
        self.validate().then_some(value.clamp(self.min, self.max))
    }
    pub fn normalized(&self, value: f64) -> Option<f64> {
        let v = self.clamp(value)?;
        Some((v - self.min) / (self.max - self.min))
    }
    pub fn denormalized(&self, normalized: f64) -> Option<f64> {
        if !self.validate() || !normalized.is_finite() {
            return None;
        }
        Some(self.min + (self.max - self.min) * normalized.clamp(0.0, 1.0))
    }
}
pub fn compatible_snapshot(previous: &[PluginParameter], current: &[PluginParameter]) -> bool {
    previous.iter().all(|old| {
        current
            .iter()
            .find(|p| p.id == old.id)
            .map(|p| {
                p.name == old.name
                    && p.unit == old.unit
                    && p.min == old.min
                    && p.max == old.max
                    && p.automatable == old.automatable
                    && p.read_only == old.read_only
            })
            .unwrap_or(false)
    })
}
pub fn compatibility_differences(
    previous: &[PluginParameter],
    current: &[PluginParameter],
) -> Vec<u32> {
    let mut result: Vec<u32> = previous
        .iter()
        .filter_map(|old| {
            current
                .iter()
                .find(|p| p.id == old.id)
                .and_then(|p| {
                    (p.name != old.name
                        || p.unit != old.unit
                        || p.min != old.min
                        || p.max != old.max
                        || p.automatable != old.automatable
                        || p.read_only != old.read_only)
                        .then_some(old.id)
                })
                .or(Some(old.id))
        })
        .chain(
            current
                .iter()
                .filter(|now| !previous.iter().any(|old| old.id == now.id))
                .map(|now| now.id),
        )
        .collect();
    result.sort_unstable();
    result.dedup();
    result
}
pub fn validate_parameter_tree(parameters: &[PluginParameter]) -> bool {
    parameters.len() <= 65_536
        && parameters.iter().all(PluginParameter::validate)
        && parameters
            .iter()
            .enumerate()
            .all(|(i, p)| parameters[..i].iter().all(|q| q.id != p.id))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PluginAutomationPoint {
    pub sample: u64,
    pub normalized: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginAutomationLane {
    pub parameter_id: u32,
    pub points: Vec<PluginAutomationPoint>,
}

impl PluginAutomationLane {
    pub fn validate(&self) -> bool {
        self.parameter_id != 0
            && self.points.len() <= 1_000_000
            && self.points.iter().all(|point| {
                point.normalized.is_finite() && (0.0..=1.0).contains(&point.normalized)
            })
            && self
                .points
                .windows(2)
                .all(|pair| pair[0].sample < pair[1].sample)
    }
    pub fn upsert(&mut self, sample: u64, normalized: f64) -> bool {
        if self.parameter_id == 0 || !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return false;
        }
        if self.points.len() >= 1_000_000 && self.points.iter().all(|point| point.sample != sample)
        {
            return false;
        }
        let point = PluginAutomationPoint { sample, normalized };
        match self
            .points
            .binary_search_by_key(&sample, |point| point.sample)
        {
            Ok(index) => self.points[index] = point,
            Err(index) => self.points.insert(index, point),
        }
        true
    }
    pub fn value_at(&self, sample: u64) -> Option<f64> {
        if !self.validate() || self.points.is_empty() {
            return None;
        }
        let index = match self
            .points
            .binary_search_by_key(&sample, |point| point.sample)
        {
            Ok(index) => return Some(self.points[index].normalized),
            Err(index) => index,
        };
        if index == 0 {
            return Some(self.points[0].normalized);
        }
        if index == self.points.len() {
            return Some(self.points[index - 1].normalized);
        }
        let left = &self.points[index - 1];
        let right = &self.points[index];
        Some(
            left.normalized
                + (right.normalized - left.normalized)
                    * ((sample - left.sample) as f64 / (right.sample - left.sample) as f64),
        )
    }
    pub fn trim(&mut self, start: u64, end: u64) -> bool {
        if end <= start || !self.validate() {
            return false;
        }
        let Some(start_value) = self.value_at(start) else {
            return false;
        };
        let Some(end_value) = self.value_at(end) else {
            return false;
        };
        let mut points: Vec<_> = self
            .points
            .iter()
            .filter(|point| point.sample > start && point.sample < end)
            .cloned()
            .collect();
        points.push(PluginAutomationPoint {
            sample: start,
            normalized: start_value,
        });
        points.push(PluginAutomationPoint {
            sample: end,
            normalized: end_value,
        });
        for point in &mut points {
            point.sample -= start;
        }
        points.sort_by_key(|point| point.sample);
        self.points = points;
        true
    }

    /// Produces one denormalized value per sample for a realtime block. This
    /// is the sample-accurate automation path used by a plugin host.
    pub fn render_block(
        &self,
        parameter: &PluginParameter,
        start_sample: u64,
        frames: usize,
    ) -> Option<Vec<f64>> {
        if frames > 1_000_000
            || parameter.id != self.parameter_id
            || !parameter.validate()
            || !self.validate()
        {
            return None;
        }
        let mut values = Vec::with_capacity(frames);
        for offset in 0..frames {
            let sample = start_sample.checked_add(offset as u64)?;
            let normalized = self
                .value_at(sample)
                .unwrap_or(parameter.normalized(parameter.default)?);
            values.push(parameter.denormalized(normalized)?);
        }
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_parameter_values() {
        let p = PluginParameter {
            id: 1,
            name: "Gain".into(),
            unit: "dB".into(),
            min: -60.0,
            max: 6.0,
            default: 0.0,
            automatable: true,
            read_only: false,
        };
        assert_eq!(p.clamp(99.0), Some(6.0));
    }
    #[test]
    fn reports_regression_differences() {
        let a = PluginParameter {
            id: 1,
            name: "Gain".into(),
            unit: "dB".into(),
            min: -1.0,
            max: 1.0,
            default: 0.0,
            automatable: true,
            read_only: false,
        };
        let mut b = a.clone();
        b.max = 2.0;
        assert_eq!(compatibility_differences(&[a], &[b]), vec![1]);
    }
    #[test]
    fn maps_normalized_values() {
        let p = PluginParameter {
            id: 1,
            name: "Gain".into(),
            unit: "dB".into(),
            min: -10.0,
            max: 10.0,
            default: 0.0,
            automatable: true,
            read_only: false,
        };
        assert_eq!(p.normalized(0.0), Some(0.5));
        assert_eq!(p.denormalized(0.5), Some(0.0));
    }
    #[test]
    fn sample_accurate_lane_interpolates_and_trims() {
        let mut lane = PluginAutomationLane {
            parameter_id: 7,
            points: Vec::new(),
        };
        assert!(lane.upsert(100, 0.0));
        assert!(lane.upsert(200, 1.0));
        assert_eq!(lane.value_at(150), Some(0.5));
        assert!(lane.trim(125, 175));
        assert_eq!(lane.points[0].sample, 0);
        assert_eq!(lane.points[0].normalized, 0.25);
        assert_eq!(lane.points[1].normalized, 0.75);
        assert!(lane.validate());
    }
    #[test]
    fn renders_denormalized_sample_accurate_block() {
        let p = PluginParameter {
            id: 7,
            name: "Mix".into(),
            unit: "%".into(),
            min: 0.0,
            max: 100.0,
            default: 0.0,
            automatable: true,
            read_only: false,
        };
        let mut lane = PluginAutomationLane {
            parameter_id: 7,
            points: Vec::new(),
        };
        assert!(lane.upsert(0, 0.0));
        assert!(lane.upsert(4, 1.0));
        let values = lane.render_block(&p, 0, 5).unwrap();
        assert_eq!(values[0], 0.0);
        assert_eq!(values[2], 50.0);
        assert_eq!(values[4], 100.0);
    }
}
