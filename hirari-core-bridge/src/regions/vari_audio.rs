// ============================================================
// Hirari VariAudio note-segment ownership: Rust holds the full
// authoritative table for a Region. C++ `Region::audioNoteSegments`
// is gone; Region keeps only an opaque store pointer plus the
// immutable rendered snapshot (see `region_processing.rs` and
// `core/engine/track_part_*.inc` for the FFI users).
//
// Every behavior below is a 1:1 port of the C++ it replaces
// (`core/editing/audio_note_segment.hpp` and the `Track`
// mutators): validation, clamp bounds, overlap/dedup rules, anchor
// window surgery, sort order, and derived projection rebuilds all
// match the old paths exactly so callers and serialization observe
// no semantic change.
//
// Realtime contract: the audio thread never touches the store.
// `HirariVariAudioFrozen` is an immutable snapshot built on the
// control thread and retained via a shared handle. Its curve views
// point into the snapshot's own backing arrays and are stable for
// its lifetime. No allocation, no lock on the audio thread.
// ============================================================

use crate::audio_note_curve::{
    hirari_audio_note_curve_build_integral_prefix, hirari_audio_note_curve_normalize_anchors,
    hirari_audio_note_curve_upsert_anchor, hirari_audio_note_curve_warp_anchors,
    hirari_audio_note_rebuild_phase_prefixes, hirari_audio_note_upsert_segment_slot,
    HirariAudioNoteAnchor, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
};

use std::slice;

// ============================================================
// Characterized C++ behavior (all confirmed by code inspection).
// Any fix must first prove it reproduces these exact rules.
// ============================================================
//
// insert (Track::upsertAudioNoteSegment):
//   - reject if !segment.valid() or analysis gate running.
//   - slot search: -2 defer overlap to analysis bridge ->
//     `hirari_audio_note_curve_finalize_analysis_job` in
//     Region; analysis bridge upserts it.
//   - slot = HirariAudioNoteUpsertSegmentSlot::Begin or PushBack:
//     Append to the end, then sort by startSeconds.
//   - slot = HirariAudioNoteUpsertSegmentSlot::Update:
//     overwrite segment index = slot.
//   - phase prefix rebuild after each change.
//   - publishRegionSnapshot.
//
// set (Track::setAudioNoteSegments):
//   - reject if > 4096 or analysis gate running.
//   - EVERY segment must pass valid(); one bad entry rejects the
//     whole batch (mirrors hirari_audio_note_validate_segments).
//   - after sorting, neighbouring overlap is rejected too.
//   - sort by startSeconds; rebuild phase prefixes.
//
// normalize/load (Track::normalizeRegionAdd):
//   - resize > 4096 truncates, invalid or >4096 anchors removed.
//   - anchors normalize (NaN/gain/clamp handled inside Rust).
//   - pitch central rebuild; sort by startSeconds; phase rebuild.
//
// upsert anchor (Track::upsertAudioNoteAnchor):
//   - reject if NaN/Inf/negative seconds, >4800 pitch,
//     >2400 formant, no matching segment at 1e-9 tolerance.
//   - cap: 4096 anchors unless the exact same position exists.
//   - upsert + phase rebuild.
//
// clear/warp/remove: same validation as C++ (NaN checks, 4096
// caps, -2 handling for overlap, 1e-9 positional match).
//
// pitch-at query (Track::audioNotePitchCents):
//   - find newest anchor-range match, else 0 except when empty.
//
// ranges projection: start/end/detected pitch.
//
// pitch-curve block: delegates to curve scheduler with exact
// zero-order fallback at silence.
//
// Serialize: rejected on null view pointers when counts are
// nonzero; IDs not persisted (C++ `id` is implicit).
//

/// Upper bound on the number of note segments a store may hold.
const STORE_CAP: usize = 4096;

/// Canonical representation of a VariAudio note segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioNoteSegment {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub detected_pitch_cents: f64,
    pub pitch_offset_cents: f64,
    pub formant_offset_cents: f64,
}

impl HirariAudioNoteSegment {
    fn valid(self) -> bool {
        if !self.start_seconds.is_finite()
            || !self.end_seconds.is_finite()
            || self.end_seconds <= self.start_seconds
            || !self.detected_pitch_cents.is_finite()
            || !self.pitch_offset_cents.is_finite()
            || !self.formant_offset_cents.is_finite()
        {
            return false;
        }
        true
    }

    /// Delegates to the single, tested implementation of the C++ slot rule
    /// (`hirari_audio_note_upsert_segment_slot`). Returns the replacement
    /// index, `-1` for "append", or `-2` when the interval conflicts with an
    /// existing segment. Do not reimplement this logic here: the tolerance,
    /// the same-start replacement rule and the overlap rejection all have to
    /// stay identical to the FFI the C++ callers use.
    fn slot(self, ranges: &[HirariAudioNoteSegmentRange]) -> i64 {
        unsafe {
            hirari_audio_note_upsert_segment_slot(
                ranges.as_ptr(),
                ranges.len(),
                self.start_seconds,
                self.end_seconds,
            )
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Segment {
    value: HirariAudioNoteSegment,
    anchors: Vec<HirariAudioNoteAnchor>,
    integral_prefix: Vec<f64>,
    correction_before_seconds: f64,
}

impl Segment {
    fn normalize(&mut self) {
        let count = unsafe {
            hirari_audio_note_curve_normalize_anchors(
                self.anchors.as_mut_ptr(),
                self.anchors.len(),
                self.value.start_seconds,
                self.value.end_seconds,
            )
        };
        if count == usize::MAX {
            return;
        }
        self.anchors.truncate(count);
    }

    fn rebuild_prefix(&mut self) {
        self.integral_prefix.resize(self.anchors.len(), 0.0);
        unsafe {
            hirari_audio_note_curve_build_integral_prefix(
                self.anchors.as_ptr(),
                self.anchors.len(),
                self.value.pitch_offset_cents,
                self.integral_prefix.as_mut_ptr(),
            )
        };
    }
}

/// Rust owns the note segments for one region. C++ only ever holds the opaque
/// store pointer plus a frozen snapshot; it never indexes the table itself.
#[repr(C)]
pub struct HirariRegionVariAudioStore {
    segments: Vec<Segment>,
}

/// What the edit bridge needs to compute a curve anchor without exposing the
/// table's internals: the segment bounds, its offsets and a copy of its anchors.
pub(crate) struct VariSegmentView {
    pub start: f64,
    pub end: f64,
    pub detected_pitch: f64,
    pub pitch_offset: f64,
    pub formant_offset: f64,
    pub anchors: Vec<HirariAudioNoteAnchor>,
}

/// Clones a table into a fresh handle. The caller owns the result and releases
/// it with `hirari_region_vari_audio_store_clear`.
pub(crate) unsafe fn vari_store_clone(
    store: *mut HirariRegionVariAudioStore,
) -> *mut HirariRegionVariAudioStore {
    if store.is_null() {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(HirariRegionVariAudioStore {
        segments: (*store).segments.clone(),
    }))
}

pub(crate) unsafe fn vari_store_restore(
    destination: *mut HirariRegionVariAudioStore,
    snapshot: *mut HirariRegionVariAudioStore,
) -> bool {
    if destination.is_null() || snapshot.is_null() {
        return false;
    }
    (*destination).segments = (*snapshot).segments.clone();
    true
}

pub(crate) unsafe fn vari_store_release(store: *mut HirariRegionVariAudioStore) {
    if !store.is_null() {
        let _ = Box::from_raw(store);
    }
}

pub(crate) unsafe fn vari_store_view(
    store: *mut HirariRegionVariAudioStore,
    segment_start: f64,
) -> Option<VariSegmentView> {
    if store.is_null() {
        return None;
    }
    let segment = (*store)
        .segments
        .iter()
        .find(|segment| (segment.value.start_seconds - segment_start).abs() < 1e-9)?;
    Some(VariSegmentView {
        start: segment.value.start_seconds,
        end: segment.value.end_seconds,
        detected_pitch: segment.value.detected_pitch_cents,
        pitch_offset: segment.value.pitch_offset_cents,
        formant_offset: segment.value.formant_offset_cents,
        anchors: segment.anchors.clone(),
    })
}

/// Immutable rendered snapshot prepared on the control thread and read
/// on the audio thread. Curve views point into the snapshot's own
/// backing arrays and are stable for its lifetime.
#[repr(C)]
pub struct HirariVariAudioFrozen {
    anchors: Vec<HirariAudioNoteAnchor>,
    integral_prefix: Vec<f64>,
    curves: Vec<HirariAudioNoteCurveView>,
    ranges: Vec<HirariAudioNoteSegmentRange>,
}
impl HirariVariAudioFrozen {
    /// Segments of the immutable projection, for control-side readers such as
    /// the project-layout serializer.
    pub(crate) fn segment_views(
        &self,
    ) -> impl Iterator<Item = (&HirariAudioNoteCurveView, &HirariAudioNoteSegmentRange)> {
        self.curves.iter().zip(self.ranges.iter())
    }

    /// Build the frozen snapshot: flatten anchors/prefix per segment,
    /// resolve curve view pointers into the frozen backing arrays.
    fn build(store: &HirariRegionVariAudioStore) -> Self {
        // Pass 1 fills the backing arrays and records where each segment's
        // slice starts. The views are only resolved in pass 2, once every
        // array has its final length: taking `as_ptr()` while the Vec is still
        // growing hands out pointers that the next `extend_from_slice` can
        // invalidate by reallocating.
        let segment_count = store.segments.len();
        let anchor_total: usize = store.segments.iter().map(|s| s.anchors.len()).sum();
        let prefix_total: usize = store.segments.iter().map(|s| s.integral_prefix.len()).sum();
        let mut frozen = Self {
            anchors: Vec::with_capacity(anchor_total),
            integral_prefix: Vec::with_capacity(prefix_total),
            curves: Vec::with_capacity(segment_count),
            ranges: Vec::with_capacity(segment_count),
        };
        let mut anchor_offsets: Vec<usize> = Vec::with_capacity(segment_count);
        let mut prefix_offsets: Vec<usize> = Vec::with_capacity(segment_count);
        for segment in store.segments.iter() {
            frozen.ranges.push(HirariAudioNoteSegmentRange {
                start_seconds: segment.value.start_seconds,
                end_seconds: segment.value.end_seconds,
                detected_pitch_cents: segment.value.detected_pitch_cents,
            });
            anchor_offsets.push(frozen.anchors.len());
            prefix_offsets.push(frozen.integral_prefix.len());
            frozen.anchors.extend_from_slice(&segment.anchors);
            frozen
                .integral_prefix
                .extend_from_slice(&segment.integral_prefix);
        }
        for (index, segment) in store.segments.iter().enumerate() {
            frozen.curves.push(HirariAudioNoteCurveView {
                anchors: unsafe { frozen.anchors.as_ptr().add(anchor_offsets[index]) },
                anchor_count: segment.anchors.len(),
                integral_prefix: unsafe {
                    frozen.integral_prefix.as_ptr().add(prefix_offsets[index])
                },
                start_seconds: segment.value.start_seconds,
                end_seconds: segment.value.end_seconds,
                pitch_offset_cents: segment.value.pitch_offset_cents,
                correction_before_seconds: segment.correction_before_seconds,
                formant_offset_cents: segment.value.formant_offset_cents,
            });
        }
        frozen
    }
}

impl HirariRegionVariAudioStore {
    fn ranges(&self) -> Vec<HirariAudioNoteSegmentRange> {
        // Projection used by the slot/overlap rules. Kept conservative with the
        // clip: only start/end/detected pitch participate in the edit policy.
        self.segments
            .iter()
            .map(|segment| HirariAudioNoteSegmentRange {
                start_seconds: segment.value.start_seconds,
                end_seconds: segment.value.end_seconds,
                detected_pitch_cents: segment.value.detected_pitch_cents,
            })
            .collect()
    }

    fn rebuild_derived(&mut self) {
        // Integral prefix per segment, then phase prefixes across all.
        for segment in self.segments.iter_mut() {
            segment.rebuild_prefix();
        }
        rebuild_phase_prefixes(self);
    }

    fn rebuild_ordered(&mut self) {
        // `valid()` already rejects non-finite starts, so `partial_cmp` cannot
        // observe NaN here; total_cmp keeps that invariant explicit.
        self.segments.sort_by(|left, right| {
            left.value
                .start_seconds
                .total_cmp(&right.value.start_seconds)
        });
    }

    /// Replaces the table from flattened C input without applying any edit
    /// policy. The store is only touched once the whole batch has been built,
    /// so a rejected batch leaves the previous table intact.
    fn replace_raw(
        &mut self,
        values: &[HirariAudioNoteSegment],
        flat: &[HirariAudioNoteAnchor],
        offsets: &[usize],
        counts: &[usize],
    ) -> bool {
        debug_assert_eq!(values.len(), offsets.len());
        debug_assert_eq!(values.len(), counts.len());
        let mut restored: Vec<Segment> = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let offset = offsets[index];
            let count = counts[index];
            if count > 4096 || offset.checked_add(count).is_none_or(|end| end > flat.len()) {
                return false;
            }
            let mut entry = Segment {
                value: *value,
                anchors: Vec::new(),
                integral_prefix: Vec::new(),
                correction_before_seconds: 0.0,
            };
            if count > 0 {
                entry
                    .anchors
                    .extend_from_slice(&flat[offset..offset + count]);
            }
            restored.push(entry);
        }
        self.segments = restored;
        true
    }

    /// Region-add / project-load policy: cap the table, drop malformed
    /// segments, normalize each anchor window, sort, rebuild the phases.
    /// Unlike `set`, nothing is rejected wholesale; bad entries are dropped.
    fn normalize(&mut self) {
        if self.segments.len() > STORE_CAP {
            self.segments.truncate(STORE_CAP);
        }
        self.segments.retain(|segment| segment.value.valid());
        for segment in self.segments.iter_mut() {
            segment.normalize();
            segment.rebuild_prefix();
        }
        self.rebuild_ordered();
        self.rebuild_derived();
    }
}

#[path = "vari_audio_phase_prefixes.rs"]
mod phase_prefixes;
pub(crate) use phase_prefixes::*;
#[path = "vari_audio_split.rs"]
mod split_src;
pub(crate) use split_src::*;

#[cfg(test)]
#[path = "vari_audio_store_tests.rs"]
mod vari_audio_store_tests;
