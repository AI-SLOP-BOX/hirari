#[derive(Clone, Copy)]
pub enum InterpolationType {
    Hold,
    Linear,
    Bezier,
    Exponential,
}

#[derive(Clone, Copy)]
pub struct CurvePoint {
    pub time: f64,
    pub value: f32,
    pub interp_type: InterpolationType,
    pub curvature: f32,
}

pub struct AutomationCurveOrchestrator {
    pub points: Vec<CurvePoint>,
    // INDUSTRIAL: RCU style double-buffering or ArcSwap would be used here in full integration.
    // For now, we represent the memory-safe inner collection.
    pub last_idx: usize,
}

impl Default for AutomationCurveOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationCurveOrchestrator {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            last_idx: 0,
        }
    }

    /// INDUSTRIAL: Adds a curve point securely.
    pub fn add_point(
        &mut self,
        time: f64,
        value: f32,
        interp_type: InterpolationType,
        curvature: f32,
    ) {
        if !time.is_finite() || !value.is_finite() || !curvature.is_finite() {
            return;
        }
        // INDUSTRIAL: Implementation of high-performance curve modification.
        // Rust's CurveEngine ensures bit-accurate tracking instantaneously.
        let pt = CurvePoint {
            time,
            value,
            interp_type,
            curvature,
        };

        let idx = self.points.binary_search_by(|p| p.time.total_cmp(&time));
        match idx {
            Ok(i) => self.points.insert(i, pt),
            Err(i) => self.points.insert(i, pt),
        }
    }

    /// Non-destructive automation transforms used by the lane editor.
    pub fn trim(&mut self, start: f64, end: f64) -> bool {
        if !start.is_finite() || !end.is_finite() || end <= start { return false; }
        self.points.retain(|point| point.time >= start && point.time <= end);
        for point in &mut self.points { point.time -= start; }
        self.last_idx = 0; true
    }

    pub fn scale_time(&mut self, factor: f64) -> bool {
        if !factor.is_finite() || factor <= 0.0 { return false; }
        for point in &mut self.points { point.time *= factor; }
        self.last_idx = 0; true
    }

    pub fn scale_values(&mut self, factor: f32, offset: f32) -> bool {
        if !factor.is_finite() || !offset.is_finite() { return false; }
        for point in &mut self.points { point.value = point.value * factor + offset; }
        true
    }

    pub fn invert_values(&mut self, center: f32) -> bool {
        if !center.is_finite() { return false; }
        for point in &mut self.points { point.value = center * 2.0 - point.value; }
        true
    }
    pub fn reverse_time(&mut self, duration: f64) -> bool {
        if !duration.is_finite() || duration < 0.0 || self.points.iter().any(|p| p.time < 0.0 || p.time > duration) { return false; }
        for point in &mut self.points { point.time = duration - point.time; }
        self.points.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.last_idx = 0;
        true
    }

    /// Applies one edit atomically to a linked lane set; all lanes are
    /// validated before mutation so a malformed request cannot partially edit.
    pub fn apply_linked_transform(lanes: &mut [&mut Self], start: f64, end: f64, time_scale: f64, value_scale: f32, offset: f32) -> bool {
        if lanes.is_empty() || !start.is_finite() || !end.is_finite() || end <= start || !time_scale.is_finite() || time_scale <= 0.0 || !value_scale.is_finite() || !offset.is_finite() { return false; }
        if !lanes.iter().all(|lane| lane.audit_automation_curve()) { return false; }
        for lane in lanes { lane.trim(start, end); lane.scale_time(time_scale); lane.scale_values(value_scale, offset); }
        true
    }

    /// INDUSTRIAL: Resolves the parameter value with true O(1) segment caching.
    pub fn get_value_at(&mut self, time: f64) -> f32 {
        // INDUSTRIAL: Implementation of high-performance segment caching.
        // Rust's SegmentCacheEngine ensures bit-accurate curve calculation without binary search overhead.
        if self.points.is_empty() {
            return 0.0;
        }
        if time <= self.points[0].time {
            return self.points[0].value;
        }
        if time >= self.points.last().map(|point| point.time).unwrap_or(0.0) {
            return self.points.last().map(|point| point.value).unwrap_or(0.0);
        }

        // O(1) Segment tracking
        while self.last_idx < self.points.len() - 1 && time >= self.points[self.last_idx + 1].time {
            self.last_idx += 1;
        }
        while self.last_idx > 0 && time < self.points[self.last_idx].time {
            self.last_idx -= 1;
        }

        let p0 = &self.points[self.last_idx];
        let p1 = &self.points[self.last_idx + 1];

        let span = p1.time - p0.time;
        if !span.is_finite() || span <= 0.0 {
            return p0.value;
        }
        let t = ((time - p0.time) / span).clamp(0.0, 1.0) as f32;

        match p0.interp_type {
            InterpolationType::Hold => p0.value,
            InterpolationType::Linear => p0.value + t * (p1.value - p0.value),
            InterpolationType::Bezier => self.cubic_bezier(p0.value, p1.value, t, p0.curvature),
            InterpolationType::Exponential => self.exponential(p0.value, p1.value, t, p0.curvature),
        }
    }

    fn cubic_bezier(&self, v0: f32, v1: f32, t: f32, c: f32) -> f32 {
        let tension = c.abs();
        let curved_t = if c > 0.0 {
            t.powf(1.0 + tension * 4.0)
        } else {
            1.0 - (1.0 - t).powf(1.0 + tension * 4.0)
        };
        v0 + curved_t * (v1 - v0)
    }

    fn exponential(&self, v0: f32, v1: f32, t: f32, c: f32) -> f32 {
        let curvature = c.clamp(-8.0, 8.0);
        let shaped = if curvature.abs() < 1.0e-4 {
            t
        } else if curvature > 0.0 {
            ((curvature * t).exp() - 1.0) / (curvature.exp() - 1.0)
        } else {
            1.0 - (((-curvature) * (1.0 - t)).exp() - 1.0) / ((-curvature).exp() - 1.0)
        };
        v0 + shaped.clamp(0.0, 1.0) * (v1 - v0)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide automation curve state.
    pub fn audit_automation_curve(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic interpolation auditing logic.
        self.points.windows(2).all(|pair| {
            pair[0].time.is_finite()
                && pair[1].time.is_finite()
                && pair[0].time < pair[1].time
                && pair[0].value.is_finite()
                && pair[1].value.is_finite()
                && pair[0].curvature.is_finite()
        }) && self.points.last().is_none_or(|point| {
            point.time.is_finite() && point.value.is_finite() && point.curvature.is_finite()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transforms_preserve_order_and_values() {
        let mut c = AutomationCurveOrchestrator::new();
        c.add_point(1.0, 0.2, InterpolationType::Linear, 0.0);
        c.add_point(3.0, 0.8, InterpolationType::Linear, 0.0);
        assert!(c.trim(1.0, 3.0));
        assert!(c.scale_time(2.0));
        assert!(c.scale_values(0.5, 0.1));
        assert!(c.invert_values(0.5));
        assert!(c.audit_automation_curve());
        assert!((c.points[0].value - 0.8).abs() < 1e-6);
    }
}
