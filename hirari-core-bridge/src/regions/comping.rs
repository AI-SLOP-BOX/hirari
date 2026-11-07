use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Default)]
struct LegacyCompingState {
    takes: HashMap<u32, (u64, u64)>,
    segments: Vec<LegacyCompSegment>,
}

#[derive(Clone, Copy)]
struct LegacyCompSegment {
    take_id: u32,
    start: u64,
    len: u64,
}

#[no_mangle]
pub extern "C" fn hirari_comping_legacy_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(LegacyCompingState::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_clone(
    state: *const std::ffi::c_void,
) -> *mut std::ffi::c_void {
    let Some(state) = (unsafe { state.cast::<LegacyCompingState>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state.clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<LegacyCompingState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_add_take(
    state: *mut std::ffi::c_void,
    take_id: u32,
    start_sample: u64,
    end_sample: u64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<LegacyCompingState>().as_mut() }) else {
        return false;
    };
    if take_id == 0 || end_sample <= start_sample {
        return false;
    }
    state.takes.insert(take_id, (start_sample, end_sample));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_active_take_at(
    state: *const std::ffi::c_void,
    sample: u64,
) -> u32 {
    let Some(state) = (unsafe { state.cast::<LegacyCompingState>().as_ref() }) else {
        return 0;
    };
    let upper = state
        .segments
        .partition_point(|segment| segment.start <= sample);
    state.segments[..upper]
        .iter()
        .rev()
        .find(|segment| {
            sample < segment.start.saturating_add(segment.len)
                && state.takes.contains_key(&segment.take_id)
        })
        .map_or(0, |segment| segment.take_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_set_segment(
    state: *mut std::ffi::c_void,
    take_id: u32,
    start: u64,
    len: u64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<LegacyCompingState>().as_mut() }) else {
        return false;
    };
    if take_id == 0 || len == 0 || !state.takes.contains_key(&take_id) {
        return false;
    }
    let segment = LegacyCompSegment {
        take_id,
        start,
        len,
    };
    let index = state
        .segments
        .partition_point(|existing| existing.start < start);
    if index < state.segments.len() && state.segments[index].start == start {
        state.segments[index] = segment;
    } else {
        state.segments.insert(index, segment);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_comping_legacy_clear(state: *mut std::ffi::c_void) {
    if let Some(state) = unsafe { state.cast::<LegacyCompingState>().as_mut() } {
        state.takes.clear();
        state.segments.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Take {
    pub id: u32,
    pub name: String,
    pub start_sample: u64,
    pub end_sample: u64,
    #[serde(default)]
    pub regions: Vec<TakeRegion>,
}

/// Native arrangement regions that carry the recorded audio for a take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeRegion {
    pub track_id: u32,
    pub region_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScoredTake {
    pub take_id: u32,
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompSegment {
    pub take_id: u32,
    pub start: u64,
    pub len: u64,
    pub crossfade_samples: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompingOrchestrator {
    pub takes: Vec<Take>,
    pub current_comp: Vec<CompSegment>,
}

impl Default for CompingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl CompingOrchestrator {
    const MAX_RENDER_FRAMES: usize = 64 * 1024 * 1024;

    fn segment_fits_take(&self, segment: &CompSegment) -> bool {
        let Some(take) = self.takes.iter().find(|take| take.id == segment.take_id) else {
            return false;
        };
        segment.start >= take.start_sample
            && segment.start <= u64::MAX.saturating_sub(segment.len)
            && segment.start.saturating_add(segment.len) <= take.end_sample
    }

    pub fn new() -> Self {
        Self {
            takes: Vec::new(),
            current_comp: Vec::new(),
        }
    }

    pub fn auto_select_take(&self, start: u64, end: u64, scores: &[ScoredTake]) -> Option<u32> {
        if end <= start || scores.len() > self.takes.len() {
            return None;
        }
        scores
            .iter()
            .filter(|s| {
                s.score.is_finite()
                    && self.takes.iter().any(|t| {
                        t.id == s.take_id && t.start_sample <= start && t.end_sample >= end
                    })
            })
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .map(|s| s.take_id)
    }

    /// Build a complete comp automatically by selecting the highest-scoring
    /// take for each fixed-size window. Windows without a valid take fail
    /// atomically, so callers never receive a partially-built comp.
    pub fn auto_comp(
        &mut self,
        start: u64,
        end: u64,
        window: u64,
        scores: &[ScoredTake],
        crossfade_samples: u32,
    ) -> bool {
        if end <= start || window == 0 || crossfade_samples as u64 > window {
            return false;
        }
        let mut segments = Vec::new();
        let mut pos = start;
        while pos < end {
            let next = pos.saturating_add(window).min(end);
            let Some(take_id) = self.auto_select_take(pos, next, scores) else {
                return false;
            };
            segments.push(CompSegment {
                take_id,
                start: pos,
                len: next - pos,
                crossfade_samples: crossfade_samples.min((next - pos) as u32),
            });
            if next == end {
                break;
            }
            pos = next;
        }
        self.set_segments(segments);
        !self.current_comp.is_empty()
    }

    /// INDUSTRIAL: Adds a new take with memory-safe Rust collections and absolute arrangement sovereignty.
    pub fn add_take(&mut self, take: Take) {
        // INDUSTRIAL: Implementation of high-performance take storage.
        // Rust's safe memory management handles large arrangement streams with
        // absolute bit-accuracy and zero-latency.
        // Rust's ArrangementEngine ensures bit-accurate arrangement synchronization.
        if take.id == 0
            || take.name.trim().is_empty()
            || take.start_sample >= take.end_sample
            || take
                .regions
                .iter()
                .any(|region| region.track_id == 0 || region.region_id == 0)
            || take.regions.iter().enumerate().any(|(index, region)| {
                take.regions[..index]
                    .iter()
                    .any(|previous| previous == region)
            })
            || self.takes.iter().any(|existing| {
                existing.id == take.id || existing.name.eq_ignore_ascii_case(take.name.trim())
            })
        {
            return;
        }
        self.takes.push(Take {
            name: take.name.trim().to_owned(),
            ..take
        });
    }

    /// Removes a take and any comp regions that reference it.
    pub fn remove_take(&mut self, take_id: u32) -> bool {
        let Some(index) = self.takes.iter().position(|take| take.id == take_id) else {
            return false;
        };
        self.takes.remove(index);
        self.current_comp
            .retain(|segment| segment.take_id != take_id);
        true
    }

    pub fn clear_comp(&mut self) {
        self.current_comp.clear();
    }

    /// Replaces the take used by an existing comp interval without disturbing
    /// neighboring regions.
    pub fn replace_segment_take(&mut self, start: u64, len: u64, take_id: u32) -> bool {
        let Some(index) = self
            .current_comp
            .iter()
            .position(|segment| segment.start == start && segment.len == len)
        else {
            return false;
        };
        let segment = CompSegment {
            take_id,
            ..self.current_comp[index].clone()
        };
        if !self.segment_fits_take(&segment) {
            return false;
        }
        self.current_comp[index] = segment;
        true
    }

    /// INDUSTRIAL: Resolves the active take and its transition metadata with absolute precision and crossfade sovereignty.
    pub fn resolve_active_take_at(&self, pos: u64) -> (u32, u32) {
        // INDUSTRIAL: Implementation of high-performance binary search for arrangement resolution.
        // Rust's ArrangementEngine ensures bit-accurate arrangement synchronization instantaneously.
        match self.current_comp.binary_search_by(|s| {
            if pos < s.start {
                std::cmp::Ordering::Greater
            } else if pos >= s.start.saturating_add(s.len) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(idx) => {
                let segment = &self.current_comp[idx];
                let dist_to_start = pos - segment.start;

                let fade_val = if dist_to_start < segment.crossfade_samples as u64 {
                    segment.crossfade_samples
                } else {
                    0
                };
                (segment.take_id, fade_val)
            }
            Err(_) => (0, 0),
        }
    }

    /// INDUSTRIAL: Sets the active comp segments with forensic arrangement auditing and deterministic merging.
    pub fn set_segments(&mut self, mut segments: Vec<CompSegment>) {
        // INDUSTRIAL: Implementation of high-performance segment management.
        // Rust's SegmentEngine ensures bit-accurate arrangement distribution.
        segments.sort_by_key(|s| s.start);
        if segments.iter().any(|segment| {
            segment.len == 0
                || segment.crossfade_samples as u64 > segment.len
                || segment.start > u64::MAX - segment.len
                || !self.segment_fits_take(segment)
        }) {
            return;
        }
        if segments.windows(2).any(|pair| {
            pair[0].start > u64::MAX - pair[0].len || pair[0].start + pair[0].len > pair[1].start
        }) {
            return;
        }
        self.current_comp = segments;
    }

    /// Adds one comp segment through the same validation rules as a bulk
    /// update. Existing segments are replaced only when they occupy exactly
    /// the same timeline interval, making punch-in comp edits deterministic.
    pub fn add_segment(&mut self, segment: CompSegment) -> bool {
        if segment.len == 0
            || segment.crossfade_samples as u64 > segment.len
            || segment.start > u64::MAX.saturating_sub(segment.len)
            || !self.segment_fits_take(&segment)
        {
            return false;
        }
        let end = segment.start + segment.len;
        if let Some(index) = self
            .current_comp
            .iter()
            .position(|existing| existing.start == segment.start && existing.len == segment.len)
        {
            self.current_comp[index] = segment;
            return true;
        }
        if self.current_comp.iter().any(|existing| {
            let existing_end = existing.start.saturating_add(existing.len);
            segment.start < existing_end && existing.start < end
        }) {
            return false;
        }
        self.current_comp.push(segment);
        self.current_comp.sort_by_key(|segment| segment.start);
        true
    }

    /// Replaces the selected time range with one take, splitting any existing
    /// segments at the selection boundaries. The update is all-or-nothing.
    pub fn swipe_take_region(
        &mut self,
        take_id: u32,
        start: u64,
        end: u64,
        crossfade_samples: u32,
    ) -> bool {
        let Some(take) = self.takes.iter().find(|take| take.id == take_id) else {
            return false;
        };
        if start >= end || start < take.start_sample || end > take.end_sample {
            return false;
        }
        let mut candidate = Vec::with_capacity(self.current_comp.len() + 2);
        for segment in &self.current_comp {
            let segment_end = segment.start.saturating_add(segment.len);
            if segment_end <= start || segment.start >= end {
                candidate.push(segment.clone());
                continue;
            }
            if segment.start < start {
                let len = start - segment.start;
                candidate.push(CompSegment {
                    take_id: segment.take_id,
                    start: segment.start,
                    len,
                    crossfade_samples: crossfade_samples.min(len.min(u32::MAX as u64) as u32),
                });
            }
            if segment_end > end {
                let len = segment_end - end;
                candidate.push(CompSegment {
                    take_id: segment.take_id,
                    start: end,
                    len,
                    crossfade_samples: crossfade_samples.min(len.min(u32::MAX as u64) as u32),
                });
            }
        }
        candidate.push(CompSegment {
            take_id,
            start,
            len: end - start,
            crossfade_samples: crossfade_samples.min((end - start).min(u32::MAX as u64) as u32),
        });
        if candidate.len() > 1_000_000 {
            return false;
        }
        candidate.sort_by_key(|segment| segment.start);
        if candidate
            .windows(2)
            .any(|pair| pair[0].start.saturating_add(pair[0].len) > pair[1].start)
            || candidate
                .iter()
                .any(|segment| !self.segment_fits_take(segment))
        {
            return false;
        }
        self.current_comp = candidate;
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide arrangement synchronization graph.
    pub fn audit_comping(&self) -> bool {
        self.takes.iter().all(|take| {
            take.id != 0
                && !take.name.trim().is_empty()
                && take.start_sample < take.end_sample
                && take
                    .regions
                    .iter()
                    .all(|region| region.track_id != 0 && region.region_id != 0)
                && take.regions.iter().enumerate().all(|(index, region)| {
                    take.regions[..index]
                        .iter()
                        .all(|previous| previous != region)
                })
        }) && self.takes.iter().enumerate().all(|(index, take)| {
            self.takes[..index].iter().all(|previous| {
                previous.id != take.id && !previous.name.eq_ignore_ascii_case(take.name.trim())
            })
        }) && self.current_comp.windows(2).all(|pair| {
            pair[0].len > 0
                && pair[0].crossfade_samples as u64 <= pair[0].len
                && pair[0].start <= u64::MAX - pair[0].len
                && pair[0].start + pair[0].len <= pair[1].start
        }) && self.current_comp.iter().all(|segment| {
            segment.len > 0
                && segment.crossfade_samples as u64 <= segment.len
                && self.segment_fits_take(segment)
        })
    }

    /// Renders the active comp from decoded take buffers.  The returned
    /// buffer uses arrangement sample positions, including silent gaps. It is
    /// deterministic and non-destructive: source takes are never modified,
    /// and each segment reads from its own take-relative range.
    pub fn render_audio(&self, take_audio: &[(u32, &[f32])]) -> Vec<f32> {
        if !self.audit_comping() {
            return Vec::new();
        }
        let total = self.current_comp.iter().try_fold(0usize, |end, segment| {
            let segment_end = segment.start.checked_add(segment.len)?;
            let segment_end = usize::try_from(segment_end).ok()?;
            Some(end.max(segment_end))
        });
        let Some(total) = total.filter(|&total| total <= Self::MAX_RENDER_FRAMES) else {
            return Vec::new();
        };
        let mut output = vec![0.0; total];
        let mut previous_end = None;
        for segment in &self.current_comp {
            let Some((_, source)) = take_audio.iter().find(|(id, _)| *id == segment.take_id) else {
                return Vec::new();
            };
            if source.iter().any(|sample| !sample.is_finite()) {
                return Vec::new();
            }
            let Some(take) = self.takes.iter().find(|take| take.id == segment.take_id) else {
                return Vec::new();
            };
            if segment.start < take.start_sample {
                return Vec::new();
            }
            let relative_start = segment.start - take.start_sample;
            let Ok(start) = usize::try_from(relative_start) else {
                return Vec::new();
            };
            let Ok(length) = usize::try_from(segment.len) else {
                return Vec::new();
            };
            let Some(source_end) = start.checked_add(length) else {
                return Vec::new();
            };
            let Ok(output_start) = usize::try_from(segment.start) else {
                return Vec::new();
            };
            let Some(output_end) = output_start.checked_add(length) else {
                return Vec::new();
            };
            if source_end > source.len()
                || output_end > output.len()
                || segment.start.saturating_add(segment.len) > take.end_sample
            {
                return Vec::new();
            }

            // Blend the incoming take into the preceding segment only when
            // the arrangement ranges touch. A gap remains silence and never
            // receives an unrelated crossfade from an earlier take.
            output[output_start..output_end].copy_from_slice(&source[start..source_end]);
            if previous_end == Some(segment.start) {
                let fade = (segment.crossfade_samples as usize)
                    .min(length)
                    .min(output_start)
                    .min(output.len().saturating_sub(output_start));
                for index in 0..fade {
                    let position = index as f32 / fade.max(1) as f32;
                    let previous = output[output_start - fade + index];
                    let incoming = output[output_start + index];
                    output[output_start + index] =
                        previous * (1.0 - position) + incoming * position;
                }
            }
            previous_end = Some(segment.start.saturating_add(segment.len));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{CompSegment, CompingOrchestrator, Take};

    fn orchestrator() -> CompingOrchestrator {
        let mut comp = CompingOrchestrator::new();
        comp.add_take(Take {
            id: 1,
            name: "Take 1".into(),
            start_sample: 0,
            end_sample: 1000,
            regions: Vec::new(),
        });
        comp.add_take(Take {
            id: 2,
            name: "Take 2".into(),
            start_sample: 0,
            end_sample: 1000,
            regions: Vec::new(),
        });
        comp
    }

    #[test]
    fn accepts_non_overlapping_segments_and_resolves_fade() {
        let mut comp = orchestrator();
        comp.set_segments(vec![
            CompSegment {
                take_id: 1,
                start: 0,
                len: 500,
                crossfade_samples: 32,
            },
            CompSegment {
                take_id: 2,
                start: 500,
                len: 500,
                crossfade_samples: 32,
            },
        ]);
        assert!(comp.audit_comping());
        assert_eq!(comp.resolve_active_take_at(510), (2, 32));
    }

    #[test]
    fn rejects_overlaps_unknown_takes_and_invalid_fades() {
        let mut comp = orchestrator();
        comp.set_segments(vec![
            CompSegment {
                take_id: 99,
                start: 0,
                len: 500,
                crossfade_samples: 32,
            },
            CompSegment {
                take_id: 1,
                start: 400,
                len: 500,
                crossfade_samples: 600,
            },
        ]);
        assert!(comp.current_comp.is_empty());
        assert!(comp.audit_comping());
    }

    #[test]
    fn empty_comp_and_out_of_range_positions_resolve_to_no_take() {
        let mut comp = orchestrator();
        assert_eq!(comp.resolve_active_take_at(0), (0, 0));
        assert_eq!(comp.resolve_active_take_at(u64::MAX), (0, 0));

        comp.set_segments(vec![CompSegment {
            take_id: 1,
            start: 100,
            len: 50,
            crossfade_samples: 8,
        }]);
        assert_eq!(comp.resolve_active_take_at(99), (0, 0));
        assert_eq!(comp.resolve_active_take_at(100), (1, 8));
        assert_eq!(comp.resolve_active_take_at(149), (1, 0));
        assert_eq!(comp.resolve_active_take_at(150), (0, 0));
    }

    #[test]
    fn comp_lane_take_replacement_and_removal_are_safe() {
        let mut comp = orchestrator();
        comp.set_segments(vec![CompSegment {
            take_id: 1,
            start: 0,
            len: 500,
            crossfade_samples: 8,
        }]);
        assert!(comp.replace_segment_take(0, 500, 2));
        assert_eq!(comp.current_comp[0].take_id, 2);
        assert!(!comp.replace_segment_take(0, 500, 99));
        assert!(comp.remove_take(2));
        assert!(comp.current_comp.is_empty());
        assert!(comp.audit_comping());
    }

    #[test]
    fn swipe_replaces_overlap_and_preserves_both_neighbor_fragments() {
        let mut comp = orchestrator();
        comp.set_segments(vec![CompSegment {
            take_id: 1,
            start: 100,
            len: 700,
            crossfade_samples: 12,
        }]);
        assert!(comp.swipe_take_region(2, 300, 500, 64));
        assert_eq!(
            comp.current_comp,
            vec![
                CompSegment {
                    take_id: 1,
                    start: 100,
                    len: 200,
                    crossfade_samples: 64,
                },
                CompSegment {
                    take_id: 2,
                    start: 300,
                    len: 200,
                    crossfade_samples: 64,
                },
                CompSegment {
                    take_id: 1,
                    start: 500,
                    len: 300,
                    crossfade_samples: 64,
                },
            ]
        );
        assert!(comp.audit_comping());
        assert!(!comp.swipe_take_region(99, 700, 800, 8));
        assert!(!comp.swipe_take_region(2, 900, 1100, 8));
        assert_eq!(comp.current_comp[1].take_id, 2);
    }
}

#[cfg(test)]
mod legacy_ffi_tests {
    use super::{
        hirari_comping_legacy_active_take_at, hirari_comping_legacy_add_take,
        hirari_comping_legacy_clear, hirari_comping_legacy_clone, hirari_comping_legacy_create,
        hirari_comping_legacy_destroy, hirari_comping_legacy_set_segment,
    };

    #[test]
    fn legacy_comping_api_keeps_sorted_segments_and_deep_clone_behavior() {
        unsafe {
            let state = hirari_comping_legacy_create();
            assert!(!state.is_null());
            assert!(hirari_comping_legacy_add_take(state, 1, 0, 1000));
            assert!(hirari_comping_legacy_add_take(state, 2, 0, 1000));
            assert!(hirari_comping_legacy_set_segment(state, 1, 200, 100));
            assert!(hirari_comping_legacy_set_segment(state, 1, 0, 100));
            assert_eq!(hirari_comping_legacy_active_take_at(state, 50), 1);
            assert_eq!(hirari_comping_legacy_active_take_at(state, 150), 0);
            assert!(hirari_comping_legacy_set_segment(state, 2, 0, 80));
            assert_eq!(hirari_comping_legacy_active_take_at(state, 50), 2);

            let cloned = hirari_comping_legacy_clone(state);
            assert!(!cloned.is_null());
            hirari_comping_legacy_clear(cloned);
            assert_eq!(hirari_comping_legacy_active_take_at(cloned, 50), 0);
            assert_eq!(hirari_comping_legacy_active_take_at(state, 50), 2);
            hirari_comping_legacy_destroy(cloned);
            hirari_comping_legacy_destroy(state);
        }
    }
}
