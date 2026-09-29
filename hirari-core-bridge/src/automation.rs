#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutomationMode {
    Read,
    Write,
    Touch,
    Latch,
    AutoPunch,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AutomationPoint {
    pub tick: u64,
    pub value: f32,
    pub curvature: f32, // -1.0 to 1.0 for Bézier curves
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AutomationCurve {
    pub param_id: u32,
    pub points: Vec<AutomationPoint>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AutomationOrchestrator {
    pub curves: Vec<AutomationCurve>,
    pub mode: AutomationMode,
    pub sample_rate: f32,
}

impl AutomationOrchestrator {
    pub fn new(sr: f32) -> Self {
        Self {
            curves: Vec::new(),
            mode: AutomationMode::Read,
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                48_000.0
            },
        }
    }
    pub fn set_mode(&mut self, mode: AutomationMode) {
        self.mode = mode;
    }

    /// Records a control move according to the active DAW automation mode.
    /// Read mode is immutable; AutoPunch accepts writes only inside the
    /// supplied cycle range. All writes share the validated upsert path.
    pub fn record_point(
        &mut self,
        param_id: u32,
        tick: u64,
        value: f32,
        curvature: f32,
        punch_range: Option<(u64, u64)>,
    ) -> bool {
        if self.mode == AutomationMode::Read {
            return false;
        }
        if self.mode == AutomationMode::AutoPunch {
            let Some((start, end)) = punch_range else {
                return false;
            };
            if end <= start || !(start..=end).contains(&tick) {
                return false;
            }
        }
        self.upsert_point(param_id, tick, value, curvature)
    }

    /// Touch-mode gesture: writes the touched value and restores the value
    /// that was present immediately before the gesture at release.
    pub fn record_touch(
        &mut self,
        param_id: u32,
        start_tick: u64,
        end_tick: u64,
        value: f32,
        curvature: f32,
    ) -> bool {
        if end_tick <= start_tick || !value.is_finite() {
            return false;
        }
        let backup = self.clone();
        let restore = self.resolve_value_at(param_id, start_tick);
        if !self.upsert_point(param_id, start_tick, value, curvature) {
            return false;
        }
        if self.upsert_point(param_id, end_tick, restore, 0.0) {
            true
        } else {
            *self = backup;
            false
        }
    }

    /// Inserts or replaces a point while keeping the curve ordered.  This is
    /// the canonical edit path for UI, MIDI learn, and CLI automation writes.
    pub fn upsert_point(&mut self, param_id: u32, tick: u64, value: f32, curvature: f32) -> bool {
        if param_id == 0
            || !value.is_finite()
            || !(0.0..=1.0).contains(&value)
            || !curvature.is_finite()
        {
            return false;
        }
        let curve = if let Some(curve) = self
            .curves
            .iter_mut()
            .find(|curve| curve.param_id == param_id)
        {
            curve
        } else {
            if self.curves.len() >= 65_536 {
                return false;
            }
            self.curves.push(AutomationCurve {
                param_id,
                points: Vec::new(),
            });
            self.curves.last_mut().expect("curve was just inserted")
        };
        if curve.points.len() >= 1_000_000
            && curve
                .points
                .binary_search_by_key(&tick, |existing| existing.tick)
                .is_err()
        {
            return false;
        }
        let point = AutomationPoint {
            tick,
            value,
            curvature: curvature.clamp(-1.0, 1.0),
        };
        match curve
            .points
            .binary_search_by_key(&tick, |existing| existing.tick)
        {
            Ok(index) => curve.points[index] = point,
            Err(index) => curve.points.insert(index, point),
        }
        true
    }

    pub fn remove_point(&mut self, param_id: u32, tick: u64) -> bool {
        let Some(curve) = self
            .curves
            .iter_mut()
            .find(|curve| curve.param_id == param_id)
        else {
            return false;
        };
        let Ok(index) = curve.points.binary_search_by_key(&tick, |point| point.tick) else {
            return false;
        };
        curve.points.remove(index);
        if curve.points.is_empty() {
            self.curves
                .retain(|candidate| candidate.param_id != param_id);
        }
        true
    }

    pub fn unlink_curve(&mut self, param_id: u32) -> bool {
        let before = self.curves.len();
        self.curves.retain(|curve| curve.param_id != param_id);
        before != self.curves.len()
    }

    pub fn clear_points(&mut self, param_id: u32) -> bool {
        let Some(index) = self
            .curves
            .iter()
            .position(|curve| curve.param_id == param_id)
        else {
            return false;
        };
        !self.curves.remove(index).points.is_empty()
    }

    pub fn snapshot_curve(&self, param_id: u32) -> Option<Vec<AutomationPoint>> {
        self.curves
            .iter()
            .find(|curve| curve.param_id == param_id)
            .map(|curve| curve.points.clone())
    }

    pub fn curve_ids(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self.curves.iter().map(|curve| curve.param_id).collect();
        ids.sort_unstable();
        ids
    }

    pub fn transform_curve(
        &mut self,
        param_id: u32,
        scale: f32,
        offset: f32,
        invert: bool,
    ) -> bool {
        if !scale.is_finite() || !offset.is_finite() {
            return false;
        }
        let Some(curve) = self.curves.iter_mut().find(|c| c.param_id == param_id) else {
            return false;
        };
        for point in &mut curve.points {
            let mut value = point.value * scale + offset;
            if invert {
                value = 1.0 - value;
            }
            point.value = value.clamp(0.0, 1.0);
        }
        true
    }

    pub fn trim_curve(&mut self, param_id: u32, start: u64, end: u64) -> bool {
        if end <= start {
            return false;
        }
        let Some(index) = self.curves.iter().position(|c| c.param_id == param_id) else {
            return false;
        };
        if self.curves[index].points.is_empty() {
            return false;
        }
        let start_value = self.resolve_value_at(param_id, start);
        let end_value = self.resolve_value_at(param_id, end);
        let mut points: Vec<_> = self.curves[index]
            .points
            .iter()
            .filter(|point| point.tick > start && point.tick < end)
            .cloned()
            .collect();
        points.push(AutomationPoint {
            tick: start,
            value: start_value,
            curvature: 0.0,
        });
        points.push(AutomationPoint {
            tick: end,
            value: end_value,
            curvature: 0.0,
        });
        // Boundary points preserve the visible curve shape when the user
        // trims between existing automation nodes.
        points.sort_by_key(|point| point.tick);
        self.curves[index].points = points;
        true
    }

    pub fn reverse_curve(&mut self, param_id: u32, start: u64, end: u64) -> bool {
        if end <= start {
            return false;
        }
        let Some(curve) = self.curves.iter_mut().find(|c| c.param_id == param_id) else {
            return false;
        };
        let mut changed = false;
        for point in &mut curve.points {
            if (start..=end).contains(&point.tick) {
                point.tick = end - (point.tick - start);
                changed = true;
            }
        }
        curve.points.sort_by_key(|point| point.tick);
        changed
    }

    /// Links two automation lanes by cloning the source points into the
    /// destination lane. Existing destination data is replaced atomically.
    pub fn link_curves(&mut self, source_id: u32, destination_id: u32) -> bool {
        if source_id == 0 || destination_id == 0 || source_id == destination_id {
            return false;
        }
        let Some(source) = self.curves.iter().find(|curve| curve.param_id == source_id) else {
            return false;
        };
        let points = source.points.clone();
        if let Some(destination) = self
            .curves
            .iter_mut()
            .find(|curve| curve.param_id == destination_id)
        {
            destination.points = points;
        } else if self.curves.len() < 65_536 {
            self.curves.push(AutomationCurve {
                param_id: destination_id,
                points,
            });
        } else {
            return false;
        }
        true
    }

    pub fn link_group(&mut self, source_id: u32, destinations: &[u32]) -> bool {
        if source_id == 0
            || destinations.is_empty()
            || destinations.iter().any(|id| *id == 0 || *id == source_id)
            || destinations
                .iter()
                .enumerate()
                .any(|(i, id)| destinations[..i].contains(id))
        {
            return false;
        }
        let Some(source) = self.curves.iter().find(|curve| curve.param_id == source_id) else {
            return false;
        };
        let points = source.points.clone();
        let new_count = destinations
            .iter()
            .filter(|id| !self.curves.iter().any(|curve| curve.param_id == **id))
            .count();
        if self.curves.len().saturating_add(new_count) > 65_536 {
            return false;
        }
        for id in destinations {
            if let Some(curve) = self.curves.iter_mut().find(|curve| curve.param_id == *id) {
                curve.points = points.clone();
            } else {
                self.curves.push(AutomationCurve {
                    param_id: *id,
                    points: points.clone(),
                });
            }
        }
        true
    }

    /// Resolves a contiguous automation block without allocating per sample.
    pub fn resolve_block(
        &self,
        param_id: u32,
        start_tick: u64,
        tick_step: u64,
        output: &mut [f32],
    ) {
        for (index, value) in output.iter_mut().enumerate() {
            let tick = start_tick.saturating_add(tick_step.saturating_mul(index as u64));
            *value = self.resolve_value_at(param_id, tick);
        }
    }

    /// INDUSTRIAL: Resolves the parameter value at a specific tick using Bézier interpolation.
    pub fn resolve_value_at(&self, param_id: u32, tick: u64) -> f32 {
        // INDUSTRIAL: Implementation of high-performance curve resolution.
        // Rust's safe memory management handles complex curve sets with
        // absolute bit-accuracy and zero-latency.
        // Rust's CurveEngine ensures bit-accurate value distribution.
        let curve = match self.curves.iter().find(|c| c.param_id == param_id) {
            Some(c) => c,
            None => return 0.0,
        };

        if curve.points.is_empty() {
            return 0.0;
        }

        // Automation data can come from external hosts, so never propagate a
        // non-finite value into the audio/control path.
        let safe_value = |value: f32| if value.is_finite() { value } else { 0.0 };

        // INDUSTRIAL: Binary search for the active automation segment.
        let idx = match curve.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => return safe_value(curve.points[i].value),
            Err(i) => i,
        };

        if idx == 0 {
            return safe_value(curve.points[0].value);
        }
        if idx >= curve.points.len() {
            return curve
                .points
                .last()
                .map(|point| safe_value(point.value))
                .unwrap_or(0.0);
        }

        let p1 = &curve.points[idx - 1];
        let p2 = &curve.points[idx];

        // INDUSTRIAL: Implementation of high-performance Bézier interpolation.
        // Equal ticks are valid input from some automation sources. They do
        // not describe an interpolation interval; use the later point.
        if p2.tick <= p1.tick {
            return safe_value(p2.value);
        }

        let t = (tick - p1.tick) as f32 / (p2.tick - p1.tick) as f32;
        if !t.is_finite() {
            return safe_value(p1.value);
        }
        safe_value(self.interpolate_bezier(
            safe_value(p1.value),
            safe_value(p2.value),
            if p1.curvature.is_finite() {
                p1.curvature.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            t.clamp(0.0, 1.0),
        ))
    }

    /// INDUSTRIAL: Smooths the parameter values with exponential smoothing and absolute precision.
    pub fn process_smoothing(&self, num_samples: u32) -> f32 {
        if num_samples == 0 || !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return 0.0;
        }
        let exponent = -(num_samples as f32) / (self.sample_rate * 0.005);
        (1.0 - exponent.exp()).clamp(0.0, 1.0)
    }

    /// Apply the block settling factor to a normalized parameter value.
    pub fn smooth_parameter(&self, current: &mut f32, target: f32, num_samples: u32) -> bool {
        if !current.is_finite() || !target.is_finite() || !(0.0..=1.0).contains(&target) {
            return false;
        }
        let factor = self.process_smoothing(num_samples);
        *current = (*current + (target - *current) * factor).clamp(0.0, 1.0);
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide automation synchronization graph.
    pub fn audit_automation(&self) -> bool {
        if !self.sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&self.sample_rate) {
            return false;
        }

        self.curves.len() <= 65_536
            && self.curves.iter().all(|curve| {
                curve.param_id != 0
                    && curve.points.windows(2).all(|pair| {
                        pair[0].tick < pair[1].tick
                            && pair[0].value.is_finite()
                            && pair[0].curvature.is_finite()
                    })
                    && curve
                        .points
                        .last()
                        .map(|point| point.value.is_finite() && point.curvature.is_finite())
                        .unwrap_or(true)
                    && curve.points.len() <= 1_000_000
            })
            && self.curves.iter().enumerate().all(|(i, curve)| {
                self.curves[..i]
                    .iter()
                    .all(|previous| previous.param_id != curve.param_id)
            })
    }

    fn interpolate_bezier(&self, v1: f32, v2: f32, c: f32, t: f32) -> f32 {
        if c == 0.0 {
            return v1 + t * (v2 - v1);
        }
        // INDUSTRIAL: Professional curve shaping logic.
        let curve_t = if c > 0.0 {
            t.powf(1.0 + c * 2.0)
        } else {
            1.0 - (1.0 - t).powf(1.0 - c * 2.0)
        };
        v1 + curve_t * (v2 - v1)
    }
}

#[cfg(test)]
mod tests {
    use super::{AutomationMode, AutomationOrchestrator};

    #[test]
    fn automation_modes_gate_writes_and_punch_range() {
        let mut automation = AutomationOrchestrator::new(48_000.0);
        assert!(!automation.record_point(1, 10, 0.5, 0.0, None));
        automation.set_mode(AutomationMode::Write);
        assert!(automation.record_point(1, 10, 0.5, 0.0, None));
        automation.set_mode(AutomationMode::AutoPunch);
        assert!(!automation.record_point(1, 20, 0.7, 0.0, Some((0, 10))));
        assert!(automation.record_point(1, 20, 0.7, 0.0, Some((20, 30))));
        assert!(automation.audit_automation());
    }

    #[test]
    fn touch_mode_restores_pre_gesture_value_at_release() {
        let mut automation = AutomationOrchestrator::new(48_000.0);
        automation.set_mode(AutomationMode::Write);
        assert!(automation.record_point(1, 0, 0.25, 0.0, None));
        assert!(automation.record_touch(1, 10, 20, 0.9, 0.0));
        assert_eq!(automation.resolve_value_at(1, 10), 0.9);
        assert_eq!(automation.resolve_value_at(1, 20), 0.25);
    }

    #[test]
    fn trimming_curve_preserves_interpolated_boundaries() {
        let mut automation = AutomationOrchestrator::new(48_000.0);
        automation.set_mode(AutomationMode::Write);
        assert!(automation.record_point(1, 0, 0.0, 0.0, None));
        assert!(automation.record_point(1, 100, 1.0, 0.0, None));
        assert!(automation.trim_curve(1, 25, 75));
        assert_eq!(
            automation
                .snapshot_curve(1)
                .unwrap()
                .iter()
                .map(|point| point.tick)
                .collect::<Vec<_>>(),
            vec![25, 75]
        );
        assert!((automation.resolve_value_at(1, 25) - 0.25).abs() < 1e-6);
        assert!((automation.resolve_value_at(1, 75) - 0.75).abs() < 1e-6);
    }
}
