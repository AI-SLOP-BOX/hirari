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
        // Native plugin parameter indices are zero-based, so parameter 0 is
        // a valid and common automation target.
        !self.name.trim().is_empty()
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
    #[serde(default)]
    pub curve: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginAutomationLane {
    /// Zero-based insert slot on the owning track. Parameter IDs are scoped
    /// to that slot and are not unique across a track's plugin chain.
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub points: Vec<PluginAutomationPoint>,
}

/// Immutable, validated automation data prepared on the control thread. Keep
/// this value alive with the plugin runtime so block rendering never scans the
/// full project lane or allocates on the audio thread.
#[derive(Clone, Debug)]
pub struct PreparedPluginAutomationLane {
    plugin_index: u32,
    parameter_id: u32,
    points: Vec<PluginAutomationPoint>,
}

impl TryFrom<PluginAutomationLane> for PreparedPluginAutomationLane {
    type Error = ();

    fn try_from(lane: PluginAutomationLane) -> Result<Self, Self::Error> {
        if !lane.validate() {
            return Err(());
        }
        Ok(Self {
            plugin_index: lane.plugin_index,
            parameter_id: lane.parameter_id,
            points: lane.points,
        })
    }
}

impl PreparedPluginAutomationLane {
    pub fn plugin_index(&self) -> u32 {
        self.plugin_index
    }

    pub fn render_block_into(
        &self,
        parameter: &PluginParameter,
        start_sample: u64,
        output: &mut [f64],
    ) -> bool {
        render_plugin_automation_block(
            self.parameter_id,
            &self.points,
            parameter,
            start_sample,
            output,
        )
    }
}

fn render_plugin_automation_block(
    parameter_id: u32,
    points: &[PluginAutomationPoint],
    parameter: &PluginParameter,
    start_sample: u64,
    output: &mut [f64],
) -> bool {
    if output.len() > 1_000_000
        || (!output.is_empty() && start_sample > u64::MAX - (output.len() as u64 - 1))
        || parameter.id != parameter_id
        || !parameter.automatable
        || parameter.read_only
        || !parameter.validate()
    {
        return false;
    }
    let default_normalized = (parameter.default - parameter.min) / (parameter.max - parameter.min);
    if points.is_empty() {
        output.fill(parameter.default);
        return true;
    }
    let mut cursor = 0;
    for (offset, value) in output.iter_mut().enumerate() {
        let sample = start_sample + offset as u64;
        let normalized =
            value_at_validated(points, sample, &mut cursor).unwrap_or(default_normalized);
        *value = parameter.min + (parameter.max - parameter.min) * normalized;
    }
    true
}

fn value_at_validated(
    points: &[PluginAutomationPoint],
    sample: u64,
    cursor: &mut usize,
) -> Option<f64> {
    let first = points.first()?;
    if sample <= first.sample {
        return Some(first.normalized);
    }
    while *cursor + 1 < points.len() && points[*cursor + 1].sample <= sample {
        *cursor += 1;
    }
    let left = points.get(*cursor)?;
    let right = match points.get(*cursor + 1) {
        Some(right) => right,
        None => return Some(left.normalized),
    };
    let amount =
        ((sample - left.sample) as f64 / (right.sample - left.sample) as f64).clamp(0.0, 1.0);
    let shaped =
        (amount + left.curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount)).clamp(0.0, 1.0);
    Some(left.normalized + (right.normalized - left.normalized) * shaped)
}

impl PluginAutomationLane {
    pub fn validate(&self) -> bool {
        // Match PluginParameter::id and the native host API: index 0 is valid.
        self.points.len() <= 1_000_000
            && self.points.iter().all(|point| {
                point.sample <= (1u64 << 53)
                    && point.normalized.is_finite()
                    && (0.0..=1.0).contains(&point.normalized)
                    && point.curve.is_finite()
                    && (-1.0..=1.0).contains(&point.curve)
            })
            && self
                .points
                .windows(2)
                .all(|pair| pair[0].sample < pair[1].sample)
    }
    pub fn upsert(&mut self, sample: u64, normalized: f64) -> bool {
        if sample > (1u64 << 53) || !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return false;
        }
        if self.points.len() >= 1_000_000 && self.points.iter().all(|point| point.sample != sample)
        {
            return false;
        }
        let point = PluginAutomationPoint {
            sample,
            normalized,
            curve: 0.0,
        };
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
        let mut cursor = match self
            .points
            .binary_search_by_key(&sample, |point| point.sample)
        {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        self.value_at_validated(sample, &mut cursor)
    }

    fn value_at_validated(&self, sample: u64, cursor: &mut usize) -> Option<f64> {
        value_at_validated(&self.points, sample, cursor)
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
            curve: 0.0,
        });
        points.push(PluginAutomationPoint {
            sample: end,
            normalized: end_value,
            curve: 0.0,
        });
        for point in &mut points {
            point.sample -= start;
        }
        points.sort_by_key(|point| point.sample);
        self.points = points;
        true
    }

    /// Fills caller-owned storage with one denormalized value per sample.
    /// This path allocates nothing and is suitable for a realtime callback.
    pub fn render_block_into(
        &self,
        parameter: &PluginParameter,
        start_sample: u64,
        output: &mut [f64],
    ) -> bool {
        if !self.validate() {
            return false;
        }
        render_plugin_automation_block(
            self.parameter_id,
            &self.points,
            parameter,
            start_sample,
            output,
        )
    }

    /// Allocating convenience wrapper for control and offline callers. The
    /// realtime audio callback should use `render_block_into` with preallocated
    /// storage instead.
    pub fn render_block(
        &self,
        parameter: &PluginParameter,
        start_sample: u64,
        frames: usize,
    ) -> Option<Vec<f64>> {
        let mut values = vec![0.0; frames];
        self.render_block_into(parameter, start_sample, &mut values)
            .then_some(values)
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
            plugin_index: 0,
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
            plugin_index: 0,
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
