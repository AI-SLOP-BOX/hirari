impl AuraCore {
    /// Registers a recorded take as a source for non-destructive comping.
    /// Take metadata lives in the Core so the Arrange view and renderer use
    /// the same boundaries instead of maintaining competing selections.
    pub fn register_comp_take(
        &self,
        take_id: u32,
        name: &str,
        start_sample: u64,
        end_sample: u64,
    ) -> bool {
        if take_id == 0 || end_sample <= start_sample || name.trim().is_empty() {
            return false;
        }
        let before = self.comping_snapshot_json();
        let changed = self.comping
            .lock()
            .map(|mut comp| {
                if comp.takes.iter().any(|take| take.id == take_id) {
                    return false;
                }
                comp.add_take(comping::Take {
                    id: take_id,
                    name: name.chars().take(128).collect(),
                    start_sample,
                    end_sample,
                });
                true
            })
            .unwrap_or(false);
        if changed {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) { self.record_comping_history(before); }
        }
        changed
    }

    /// Removes an unreferenced comp take. Active segments are never silently
    /// rewritten; callers must clear or replace those segments first.
    pub fn remove_comp_take(&self, take_id: u32) -> bool {
        if take_id == 0 { return false; }
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else { return false; };
        if comp.current_comp.iter().any(|segment| segment.take_id == take_id) {
            return false;
        }
        let old_len = comp.takes.len();
        comp.takes.retain(|take| take.id != take_id);
        let changed = comp.takes.len() != old_len;
        drop(comp);
        if changed {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) { self.record_comping_history(before); }
        }
        changed
    }

    /// Replaces the current comp with sorted, non-overlapping segments.
    /// `crossfade_samples` is validated against each segment length.
    pub fn set_comp_segments(&self, segments: &[(u32, u64, u64, u32)]) -> bool {
        let mut segments = segments
            .iter()
            .map(
                |(take_id, start, len, crossfade_samples)| comping::CompSegment {
                    take_id: *take_id,
                    start: *start,
                    len: *len,
                    crossfade_samples: *crossfade_samples,
                },
            )
            .collect::<Vec<_>>();
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        segments.sort_by_key(|segment| segment.start);
        let valid = segments.iter().all(|segment| {
            segment.len > 0
                && segment.crossfade_samples as u64 <= segment.len
                && segment.start <= u64::MAX - segment.len
                && comp.takes.iter().any(|take| take.id == segment.take_id)
        }) && segments.windows(2).all(|pair| {
            pair[0].start <= u64::MAX - pair[0].len && pair[0].start + pair[0].len <= pair[1].start
        });
        if !valid {
            return false;
        }
        comp.set_segments(segments);
        let valid = comp.audit_comping();
        drop(comp);
        if valid {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) { self.record_comping_history(before); }
        }
        valid
    }

    /// Applies a take choice to the current comp without maintaining a second
    /// UI-side selection model. Existing timeline intervals are preserved;
    /// when no intervals exist, the take becomes a one-segment comp so the
    /// first real selection is immediately audible and renderable.
    pub fn select_comp_take(&self, take_id: u32) -> bool {
        if take_id == 0 {
            return false;
        }
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        let Some(take) = comp.takes.iter().find(|take| take.id == take_id).cloned() else {
            return false;
        };
        let mut candidate = comp.current_comp.clone();
        if candidate.is_empty() {
            candidate.push(comping::CompSegment {
                take_id,
                start: take.start_sample,
                len: take.end_sample.saturating_sub(take.start_sample),
                crossfade_samples: 0,
            });
        } else {
            for segment in &mut candidate {
                segment.take_id = take_id;
            }
        }
        let mut candidate_state = comping::CompingOrchestrator {
            takes: comp.takes.clone(),
            current_comp: candidate,
        };
        if !candidate_state.audit_comping() {
            return false;
        }
        comp.current_comp = std::mem::take(&mut candidate_state.current_comp);
        drop(comp);
        if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) { self.record_comping_history(before); }
        true
    }

    /// Sets the crossfade for every active comp segment. The UI supplies a
    /// normalized value, while the Core owns the frame-unit conversion and
    /// clamps it against each segment's actual length.
    pub fn set_comp_crossfade_normalized(&self, normalized: f32) -> bool {
        if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return false;
        }
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        for segment in &mut comp.current_comp {
            let requested = (normalized * 4096.0).round() as u64;
            segment.crossfade_samples = requested
                .min(segment.len)
                .min(u64::from(u32::MAX)) as u32;
        }
        let valid = comp.audit_comping();
        drop(comp);
        if valid {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) { self.record_comping_history(before); }
        }
        valid
    }

    pub fn set_comp_segments_diagnostic_json(&self, snapshot: &str) -> String {
        let segments = match serde_json::from_str::<Vec<(u32, u64, u64, u32)>>(snapshot) {
            Ok(value) => value,
            Err(_) => {
                return serde_json::to_string(&crate::bridge_error::BridgeError::new(
                    "invalid_comp_segments_json",
                    "comp segments must be an array of [take_id,start,length,crossfade] tuples",
                ))
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
            }
        };
        if self.set_comp_segments(&segments) {
            return format!("{{\"ok\":true,\"segment_count\":{}}}", segments.len());
        }
        let result = crate::bridge_error::BridgeError::new(
            "invalid_comp_segments",
            "segments overlap, reference an unknown take, or have invalid bounds",
        );
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Returns the take and fade distance selected at a timeline sample.
    pub fn resolve_comp_at(&self, sample: u64) -> (u32, u32) {
        self.comping
            .lock()
            .map(|comp| comp.resolve_active_take_at(sample))
            .unwrap_or((0, 0))
    }

    pub fn render_comped_audio(&self, take_audio: &[(u32, &[f32])]) -> Vec<f32> {
        self.comping
            .lock()
            .map(|comp| comp.render_audio(take_audio))
            .unwrap_or_default()
    }

    pub fn midi_events_json(&self) -> String {
        self.midi_events
            .lock()
            .ok()
            .and_then(|events| serde_json::to_string(&*events).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }
}
