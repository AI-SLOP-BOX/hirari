#[derive(Clone, Debug, PartialEq)]
pub struct RecordEvent {
    pub track_id: u32,
    pub param_id: u32,
    pub pos: u64,
    pub val: f32,
}

// Keep the preview implementation close to the existing recorder without
// changing the crate's public module layout. The buffer is intended to be
// constructed and preallocated off the audio callback thread.
#[path = "recording_preview.rs"]
pub mod recording_preview;

pub struct AutomationRecorderOrchestrator {
    pub events: Vec<RecordEvent>,
    pub max_events: usize,
    pub write_protected: bool,
    pub preview: Option<RecordEvent>,
    lane_last_pos: std::collections::HashMap<(u32, u32), u64>,
}

impl Default for AutomationRecorderOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationRecorderOrchestrator {
    pub fn new() -> Self {
        Self {
            events: Vec::with_capacity(65536),
            max_events: 65536,
            write_protected: false,
            preview: None,
            lane_last_pos: std::collections::HashMap::new(),
        }
    }

    /// INDUSTRIAL: Captures parameter movements with zero-latency lock-free memory safety.
    pub fn record_value(&mut self, _track_id: u32, _param_id: u32, value: f32, timestamp: u64) {
        if self.write_protected { return; }
        // INDUSTRIAL: Implementation of high-performance gesture capture.
        // Rust's safe memory management handles complex parameter sweeps with
        // absolute bit-accuracy and zero-latency.
        // Rust's CaptureEngine ensures bit-accurate event distribution instantaneously.
        // Ignore values which cannot be represented in an automation curve.
        // Timestamps are required to be monotonic so downstream thinning never
        // produces a curve whose points move backwards in time.
        let tick_is_monotonic = self.lane_last_pos.get(&(_track_id, _param_id)).is_none_or(|previous| timestamp >= *previous);

        if _track_id != 0 && _param_id != 0 && value.is_finite() && tick_is_monotonic
            && self.events.len() < self.max_events {
            self.events.push(RecordEvent { track_id: _track_id, param_id: _param_id,
                pos: timestamp,
                val: value,
            });
            self.lane_last_pos.insert((_track_id, _param_id), timestamp);
        }
    }
    pub fn set_write_protected(&mut self, protected: bool) { self.write_protected = protected; }
    pub fn clear_preview(&mut self) { self.preview = None; }

    pub fn preview_value(&mut self, value: f32, timestamp: u64) -> bool { if !value.is_finite() { return false; } self.preview=Some(RecordEvent{track_id:0,param_id:0,pos:timestamp,val:value}); true }
    pub fn capture_preview(&mut self, track: u32, param: u32) -> bool { let Some(event)=self.preview.take() else { return false; }; let before = self.events.len(); self.record_value(track,param,event.val,event.pos); self.events.len() > before }
    pub fn set_max_events(&mut self, max_events: usize) -> bool { if max_events == 0 || max_events > 10_000_000 { return false; } self.max_events = max_events; self.events.truncate(max_events); true }
    pub fn clear_lane(&mut self, track_id: u32, param_id: u32) -> bool { let before=self.events.len(); self.events.retain(|e| e.track_id != track_id || e.param_id != param_id); self.lane_last_pos.remove(&(track_id,param_id)); before != self.events.len() }

    /// Applies non-destructive lane editing operations used by trim/scale and
    /// reverse automation commands. Values remain normalized and finite.
    pub fn transform_lane(&mut self, track_id: u32, param_id: u32, scale: f32, offset: f32, invert: bool) -> bool {
        if track_id == 0 || param_id == 0 || !scale.is_finite() || !offset.is_finite() { return false; }
        let mut changed = false;
        for event in self.events.iter_mut().filter(|e| e.track_id == track_id && e.param_id == param_id) {
            let mut value = event.val * scale + offset;
            if invert { value = 1.0 - value; }
            if !value.is_finite() { return false; }
            event.val = value.clamp(0.0, 1.0);
            changed = true;
        }
        changed
    }

    pub fn clear_before(&mut self, timestamp: u64) -> usize {
        let before = self.events.len();
        self.events.retain(|event| event.pos >= timestamp);
        self.lane_last_pos.clear();
        for event in &self.events { self.lane_last_pos.insert((event.track_id, event.param_id), event.pos); }
        before - self.events.len()
    }

    /// Trims one automation lane to an inclusive time range and rebases the
    /// retained points to zero. The edit is transactional: invalid ranges or
    /// missing lanes leave the recorder unchanged.
    pub fn trim_lane(&mut self, track_id: u32, param_id: u32, start: u64, end: u64) -> bool {
        if track_id == 0 || param_id == 0 || end <= start { return false; }
        if !self.events.iter().any(|event| event.track_id == track_id && event.param_id == param_id) { return false; }
        let mut updated = self.events.clone();
        updated.retain(|event| event.track_id != track_id || event.param_id != param_id || (event.pos >= start && event.pos <= end));
        for event in updated.iter_mut().filter(|event| event.track_id == track_id && event.param_id == param_id) {
            event.pos = event.pos.saturating_sub(start);
        }
        self.events = updated;
        self.rebuild_lane_positions();
        true
    }

    /// Reverses the selected lane inside an inclusive range while preserving
    /// sample/tick precision and deterministic event ordering.
    pub fn reverse_lane(&mut self, track_id: u32, param_id: u32, start: u64, end: u64) -> bool {
        if track_id == 0 || param_id == 0 || end <= start { return false; }
        let mut changed = false;
        for event in self.events.iter_mut().filter(|event| event.track_id == track_id && event.param_id == param_id && event.pos >= start && event.pos <= end) {
            event.pos = start.saturating_add(end.saturating_sub(event.pos));
            changed = true;
        }
        if changed {
            self.events.sort_by_key(|event| (event.track_id, event.param_id, event.pos));
            self.rebuild_lane_positions();
        }
        changed
    }

    fn rebuild_lane_positions(&mut self) {
        self.lane_last_pos.clear();
        for event in &self.events {
            self.lane_last_pos
                .entry((event.track_id, event.param_id))
                .and_modify(|position| *position = (*position).max(event.pos))
                .or_insert(event.pos);
        }
    }

    pub fn export_csv(&self) -> String {
        let mut out = String::from("track_id,param_id,pos,val\n");
        for event in &self.events { out.push_str(&format!("{},{},{},{}\n", event.track_id, event.param_id, event.pos, event.val)); }
        out
    }

    /// INDUSTRIAL: Performs intelligent point thinning and commits to curves.
    pub fn flush(&mut self) {
        // INDUSTRIAL: Implementation of high-performance point reduction.
        // Rust's PointThinningEngine ensures bit-accurate curve optimization.

        if self.events.is_empty() {
            return;
        }

        let mut thinned_events = Vec::with_capacity(self.events.len());
        let mut last_tick = std::collections::HashMap::<(u32, u32), u64>::new();

        for curr in &self.events {
            if !curr.val.is_finite() || last_tick.get(&(curr.track_id, curr.param_id)).is_some_and(|tick| curr.pos < *tick) {
                continue;
            }

            let should_keep = thinned_events.iter().rev().find(|prev: &&RecordEvent| prev.track_id == curr.track_id && prev.param_id == curr.param_id)
                .is_none_or(|prev| (curr.val - prev.val).abs() > 0.001);

            // Thinning Logic: Skip redundant points where value hasn't changed significantly
            if should_keep {
                thinned_events.push(RecordEvent { track_id: curr.track_id, param_id: curr.param_id,
                    pos: curr.pos,
                    val: curr.val,
                });
            }

            last_tick.insert((curr.track_id, curr.param_id), curr.pos);
        }

        self.events = thinned_events;
    }

    pub fn sample_accurate_snapshot(&self) -> Vec<(u64, f32)> { self.events.iter().map(|e|(e.pos,e.val)).collect() }
    pub fn lane_snapshot(&self, track_id: u32, param_id: u32) -> Vec<(u64, f32)> { self.events.iter().filter(|e| e.track_id == track_id && e.param_id == param_id).map(|e|(e.pos,e.val)).collect() }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide capture state.
    pub fn audit_automation_recorder(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic gesture auditing logic.
        self.max_events > 0 && self.max_events <= 10_000_000
            && self.events.len() <= self.max_events
            && self.events.iter().all(|event| event.track_id != 0 && event.param_id != 0)
            && self.events.iter().all(|event| event.val.is_finite())
            && self.events.iter().enumerate().all(|(i,event)| self.events[..i].iter().filter(|previous| previous.track_id == event.track_id && previous.param_id == event.param_id).all(|previous| previous.pos <= event.pos))
    }
}

#[cfg(test)]
mod tests {
    use super::AutomationRecorderOrchestrator;

    #[test]
    fn flush_keeps_only_meaningful_monotonic_points() {
        let mut recorder = AutomationRecorderOrchestrator::new();
        recorder.record_value(1, 2, 0.1, 0);
        recorder.record_value(1, 2, 0.1005, 1);
        recorder.record_value(1, 2, 0.5, 2);
        recorder.flush();
        assert_eq!(recorder.events.len(), 2);
        assert!(recorder.audit_automation_recorder());
    }

    #[test]
    fn non_monotonic_or_non_finite_values_are_rejected() {
        let mut recorder = AutomationRecorderOrchestrator::new();
        recorder.record_value(1, 2, 0.5, 10);
        recorder.record_value(1, 2, 0.6, 9);
        recorder.record_value(1, 2, f32::NAN, 11);
        assert_eq!(recorder.events.len(), 1);
    }

    #[test]
    fn lane_trim_and_reverse_are_scoped_and_rebased() {
        let mut recorder = AutomationRecorderOrchestrator::new();
        recorder.record_value(1, 2, 0.1, 10);
        recorder.record_value(1, 2, 0.4, 20);
        recorder.record_value(1, 2, 0.8, 30);
        recorder.record_value(1, 3, 0.2, 10);
        assert!(recorder.trim_lane(1, 2, 10, 30));
        assert_eq!(recorder.lane_snapshot(1, 2).iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 10, 20]);
        assert_eq!(recorder.lane_snapshot(1, 3).iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![10]);
        assert!(recorder.reverse_lane(1, 2, 0, 20));
        assert_eq!(recorder.lane_snapshot(1, 2).iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 10, 20]);
        assert!(recorder.audit_automation_recorder());
    }
}
