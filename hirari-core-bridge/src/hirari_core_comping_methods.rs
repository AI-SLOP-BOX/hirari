impl HirariCore {
    /// Returns the next stable take identity from the persisted comp registry.
    /// The recording preview keeps only a bounded recent history, so its
    /// length cannot be used as the project-wide comp take ID.
    pub fn next_comp_take_id(&self) -> u32 {
        let Ok(comp) = self.comping.lock() else {
            return 0;
        };
        comp.takes
            .iter()
            .map(|take| take.id)
            .max()
            .map_or(1, |id| id.checked_add(1).unwrap_or(0))
    }

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
        self.register_comp_take_with_regions(take_id, name, start_sample, end_sample, Vec::new())
    }

    /// Registers a take together with the native regions that contain its
    /// recorded audio. These links let later comp operations reach the actual
    /// arrangement signal instead of stopping at take metadata.
    pub fn register_comp_take_with_regions(
        &self,
        take_id: u32,
        name: &str,
        start_sample: u64,
        end_sample: u64,
        regions: Vec<comping::TakeRegion>,
    ) -> bool {
        if take_id == 0 || end_sample <= start_sample || name.trim().is_empty() {
            return false;
        }
        let before = self.comping_snapshot_json();
        let changed = self
            .comping
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
                    regions,
                });
                comp.takes.last().is_some_and(|take| take.id == take_id)
            })
            .unwrap_or(false);
        if changed {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
                self.record_comping_history(before);
            }
        }
        changed
    }

    /// Applies every audio-cycle pass to the comp registry while the caller's
    /// native region undo transaction is still open. The caller records one
    /// sidecar snapshot after that transaction commits.
    pub(crate) fn register_recording_cycle_takes(
        &self,
        takes: &[comping::Take],
    ) -> bool {
        if takes.is_empty() || takes.iter().any(|take| {
            take.id == 0 || take.start_sample >= take.end_sample || take.name.trim().is_empty()
        }) {
            return false;
        }
        let before = {
            let Ok(comp) = self.comping.lock() else { return false };
            comp.clone()
        };
        let mut candidate = before.clone();
        for take in takes {
            let previous_len = candidate.takes.len();
            candidate.add_take(take.clone());
            if candidate.takes.len() != previous_len + 1 {
                return false;
            }
        }
        let Some(selected) = takes.last().and_then(|take| {
            candidate.takes.iter().find(|candidate| candidate.id == take.id)
        }).cloned() else { return false };
        if candidate.current_comp.is_empty() {
            candidate.current_comp.push(comping::CompSegment {
                take_id: selected.id,
                start: selected.start_sample,
                len: selected.end_sample - selected.start_sample,
                crossfade_samples: 0,
            });
        } else {
            for segment in &mut candidate.current_comp {
                segment.take_id = selected.id;
            }
            if !candidate.audit_comping() {
                candidate.current_comp = vec![comping::CompSegment {
                    take_id: selected.id,
                    start: selected.start_sample,
                    len: selected.end_sample - selected.start_sample,
                    crossfade_samples: 0,
                }];
            }
        }
        if !candidate.audit_comping() || !self.apply_comping_audio_masks(&candidate) {
            let _ = self.apply_comping_audio_masks(&before);
            return false;
        }
        let Ok(mut current) = self.comping.lock() else {
            let _ = self.apply_comping_audio_masks(&before);
            return false;
        };
        if *current != before {
            drop(current);
            let _ = self.apply_comping_audio_masks(&before);
            return false;
        }
        *current = candidate;
        true
    }

    /// Removes an unreferenced comp take. Active segments are never silently
    /// rewritten; callers must clear or replace those segments first.
    pub fn remove_comp_take(&self, take_id: u32) -> bool {
        if take_id == 0 {
            return false;
        }
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        if comp
            .current_comp
            .iter()
            .any(|segment| segment.take_id == take_id)
        {
            return false;
        }
        let Some(take) = comp.takes.iter().find(|take| take.id == take_id).cloned() else {
            return false;
        };
        for region in &take.regions {
            if !self.set_region_comp_ranges(region.track_id, region.region_id, &[], false) {
                return false;
            }
        }
        let old_len = comp.takes.len();
        comp.takes.retain(|take| take.id != take_id);
        let changed = comp.takes.len() != old_len;
        drop(comp);
        if changed {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
                self.record_comping_history(before);
            }
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
        if valid {
            let candidate = comp.clone();
            if !self.apply_comping_audio_masks(&candidate) {
                if let Ok(previous) = serde_json::from_str::<comping::CompingOrchestrator>(&before)
                {
                    *comp = previous.clone();
                    drop(comp);
                    let _ = self.apply_comping_audio_masks(&previous);
                }
                return false;
            }
        }
        drop(comp);
        if valid {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
                self.record_comping_history(before);
            }
        }
        valid
    }

    /// Applies one non-destructive quick-swipe over an absolute timeline range.
    /// Existing material outside the swipe is retained; intersected segments
    /// are split at the swipe edges and the selected take owns the new range.
    pub fn swipe_comp_take_region(
        &self,
        take_id: u32,
        start_sample: u64,
        end_sample: u64,
        crossfade_samples: u32,
    ) -> bool {
        if take_id == 0 || end_sample <= start_sample {
            return false;
        }
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        let Some(take) = comp.takes.iter().find(|take| take.id == take_id) else {
            return false;
        };
        if start_sample < take.start_sample
            || end_sample > take.end_sample
            || crossfade_samples as u64 > end_sample - start_sample
        {
            return false;
        }

        let mut next = Vec::with_capacity(comp.current_comp.len().saturating_add(2));
        for segment in &comp.current_comp {
            let Some(segment_end) = segment.start.checked_add(segment.len) else {
                return false;
            };
            if segment_end <= start_sample || segment.start >= end_sample {
                next.push(segment.clone());
                continue;
            }
            if segment.start < start_sample {
                next.push(comping::CompSegment {
                    take_id: segment.take_id,
                    start: segment.start,
                    len: start_sample - segment.start,
                    crossfade_samples: crossfade_samples.min(
                        (start_sample - segment.start).min(u64::from(u32::MAX)) as u32,
                    ),
                });
            }
            if segment_end > end_sample {
                next.push(comping::CompSegment {
                    take_id: segment.take_id,
                    start: end_sample,
                    len: segment_end - end_sample,
                    crossfade_samples: segment.crossfade_samples.min(
                        (segment_end - end_sample).min(u64::from(u32::MAX)) as u32,
                    ),
                });
            }
        }
        next.push(comping::CompSegment {
            take_id,
            start: start_sample,
            len: end_sample - start_sample,
            crossfade_samples,
        });
        next.sort_by_key(|segment| segment.start);
        if next.windows(2).any(|pair| {
            pair[0].start.saturating_add(pair[0].len) > pair[1].start
        }) {
            return false;
        }
        let mut candidate = comp.clone();
        candidate.set_segments(next);
        if !candidate.audit_comping() || !self.apply_comping_audio_masks(&candidate) {
            let _ = self.apply_comping_audio_masks(&comp);
            return false;
        }
        *comp = candidate;
        drop(comp);
        if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
            self.record_comping_history(before);
        }
        true
    }

    fn apply_comping_audio_masks(&self, state: &comping::CompingOrchestrator) -> bool {
        type Range = (u64, u64, u64, u64);
        let Ok(layout) =
            serde_json::from_str::<Vec<NativeLayoutTrack>>(&self.get_project_layout_json())
        else {
            return false;
        };
        let mut linked = std::collections::BTreeMap::new();
        for take in &state.takes {
            for region in &take.regions {
                let key = (region.track_id, region.region_id);
                if linked.insert(key, Vec::<Range>::new()).is_some() {
                    return false;
                }
            }
        }
        let managed = !state.current_comp.is_empty();
        if managed {
            for (index, segment) in state.current_comp.iter().enumerate() {
                let Some(take) = state.takes.iter().find(|take| take.id == segment.take_id) else {
                    return false;
                };
                let segment_end = match segment.start.checked_add(segment.len) {
                    Some(end) => end,
                    None => return false,
                };
                let previous_segment = index.checked_sub(1).and_then(|i| state.current_comp.get(i));
                let fade_in = previous_segment
                    .filter(|previous| {
                        previous.take_id != segment.take_id
                            && previous.start.checked_add(previous.len) == Some(segment.start)
                    })
                    .map_or(0, |previous| {
                        let previous_take_end = state
                            .takes
                            .iter()
                            .find(|candidate| candidate.id == previous.take_id)
                            .map_or(previous.start, |candidate| candidate.end_sample);
                        let previous_end = previous.start.saturating_add(previous.len);
                        u64::from(segment.crossfade_samples)
                            .min(previous.len)
                            .min(segment.len)
                            .min(previous_take_end.saturating_sub(previous_end))
                    });
                let next_segment = state.current_comp.get(index + 1);
                let fade_out = next_segment
                    .filter(|next| next.take_id != segment.take_id && next.start == segment_end)
                    .map_or(0, |next| {
                        u64::from(next.crossfade_samples)
                            .min(segment.len)
                            .min(next.len)
                    });
                let extended_end = segment_end.saturating_add(fade_out).min(take.end_sample);
                let actual_fade_out = extended_end.saturating_sub(segment_end);
                let mut linked_segment = false;
                for take_region in &take.regions {
                    let Some(region) = layout
                        .iter()
                        .find(|track| track.id == take_region.track_id)
                        .and_then(|track| {
                            track
                                .regions
                                .iter()
                                .find(|region| region.id == take_region.region_id)
                        })
                    else {
                        return false;
                    };
                    let Some(region_end) = region.start.checked_add(region.len) else {
                        return false;
                    };
                    let absolute_start = segment.start.max(region.start);
                    let absolute_end = extended_end.min(region_end);
                    if absolute_start >= absolute_end {
                        continue;
                    }
                    let local_start = absolute_start - region.start;
                    let local_end = absolute_end - region.start;
                    let local_fade_in = if absolute_start == segment.start {
                        fade_in.min(local_end - local_start)
                    } else {
                        0
                    };
                    let local_fade_out = if absolute_end == extended_end {
                        actual_fade_out.min(local_end - local_start)
                    } else {
                        0
                    };
                    let Some(take_ranges) =
                        linked.get_mut(&(take_region.track_id, take_region.region_id))
                    else {
                        return false;
                    };
                    take_ranges.push((local_start, local_end, local_fade_in, local_fade_out));
                    linked_segment = true;
                }
                if !linked_segment && !take.regions.is_empty() {
                    return false;
                }
            }
        }

        for ((track_id, region_id), mut ranges) in linked {
            ranges.sort_by_key(|range| range.0);
            if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                return false;
            }
            if !self.set_region_comp_ranges(track_id, region_id, &ranges, managed) {
                return false;
            }
        }
        true
    }

    fn clear_removed_comp_audio_masks(
        &self,
        previous: &comping::CompingOrchestrator,
        next: &comping::CompingOrchestrator,
    ) {
        let retained = next
            .takes
            .iter()
            .flat_map(|take| take.regions.iter())
            .map(|region| (region.track_id, region.region_id))
            .collect::<std::collections::BTreeSet<_>>();
        for region in previous.takes.iter().flat_map(|take| take.regions.iter()) {
            if !retained.contains(&(region.track_id, region.region_id)) {
                let _ = self.set_region_comp_ranges(region.track_id, region.region_id, &[], false);
            }
        }
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
            current_comp: candidate.clone(),
        };
        if !candidate_state.audit_comping() {
            candidate_state.current_comp = vec![comping::CompSegment {
                take_id,
                start: take.start_sample,
                len: take.end_sample.saturating_sub(take.start_sample),
                crossfade_samples: 0,
            }];
            if !candidate_state.audit_comping() {
                return false;
            }
        }
        if !self.apply_comping_audio_masks(&candidate_state) {
            let _ = self.apply_comping_audio_masks(&comp);
            return false;
        }
        comp.current_comp = std::mem::take(&mut candidate_state.current_comp);
        drop(comp);
        if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
            self.record_comping_history(before);
        }
        true
    }

    /// Sets the crossfade for every active comp segment. The UI supplies a
    /// normalized value, while the Core owns the frame-unit conversion and
    /// clamps it against each segment's actual length.
    pub fn set_comp_crossfade_normalized(&self, normalized: f32) -> bool {
        if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return false;
        }
        self.set_comp_crossfade_samples((normalized * 4096.0).round() as u32)
    }

    /// Sets the active comp boundary fade in audio frames.
    pub fn set_comp_crossfade_samples(&self, samples: u32) -> bool {
        let before = self.comping_snapshot_json();
        let Ok(mut comp) = self.comping.lock() else {
            return false;
        };
        for segment in &mut comp.current_comp {
            segment.crossfade_samples = u64::from(samples).min(segment.len) as u32;
        }
        let valid = comp.audit_comping();
        if valid {
            let candidate = comp.clone();
            if !self.apply_comping_audio_masks(&candidate) {
                if let Ok(previous) = serde_json::from_str::<comping::CompingOrchestrator>(&before)
                {
                    *comp = previous.clone();
                    drop(comp);
                    let _ = self.apply_comping_audio_masks(&previous);
                }
                return false;
            }
        }
        drop(comp);
        if valid {
            if let Ok(before) = serde_json::from_str::<comping::CompingOrchestrator>(&before) {
                self.record_comping_history(before);
            }
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
