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
struct Segment {
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

fn rebuild_phase_prefixes(store: &mut HirariRegionVariAudioStore) {
    use crate::audio_note_curve::HirariAudioNotePhaseView;
    let stores = &mut store.segments;
    if stores.is_empty() {
        return;
    }
    let mut views: Vec<HirariAudioNotePhaseView> = stores
        .iter()
        .map(|segment| HirariAudioNotePhaseView {
            start_seconds: segment.value.start_seconds,
            end_seconds: segment.value.end_seconds,
            pitch_offset_cents: segment.value.pitch_offset_cents,
            anchors: segment.anchors.as_ptr(),
            anchor_count: segment.anchors.len(),
            integral_prefix: segment.integral_prefix.as_ptr(),
        })
        .collect();
    let mut order: Vec<u32> = vec![0; views.len()];
    let mut corrections: Vec<f64> = vec![0.0; views.len()];
    if unsafe {
        hirari_audio_note_rebuild_phase_prefixes(
            views.as_mut_ptr(),
            views.len(),
            order.as_mut_ptr(),
            corrections.as_mut_ptr(),
        )
    } {
        let mut reordered: Vec<Segment> = Vec::with_capacity(stores.len());
        for &index in order.iter() {
            reordered.push(stores[index as usize].clone());
        }
        for (index, segment) in reordered.iter_mut().enumerate() {
            segment.correction_before_seconds = corrections[index];
        }
        *stores = reordered;
    }
}

/// Create an empty Rust-owned note store.
///
/// # Safety
/// Always returns a fresh, uniquely-owned store. The caller owns the
/// returned pointer and must release it with
/// `hirari_region_vari_audio_store_clear` exactly once.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_new() -> *mut HirariRegionVariAudioStore {
    let store = HirariRegionVariAudioStore {
        segments: Vec::new(),
    };
    Box::into_raw(Box::new(store))
}

///
/// # Safety
/// `store` must be null or come from `hirari_region_vari_audio_store_new`,
/// and no other pointer into the store may be used afterwards: the call
/// consumes the box, dropping the table and every anchor array with it.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_clear(
    store: *mut HirariRegionVariAudioStore,
) {
    if store.is_null() {
        return;
    }
    let _ = Box::from_raw(store);
}

/// Insert or replace a note segment, optionally with anchors.
/// Returns `true` on success. Mirrors `Track::upsertAudioNoteSegment`
/// plus `Track::normalizeRegionAdd` validation exactly.
///
/// # Safety
/// `store` must be a live store from `hirari_region_vari_audio_store_new`.
/// When `anchor_count > 0`, `anchors` must be readable for
/// `anchor_count * size_of::<HirariAudioNoteAnchor>()` bytes and stays
/// borrowed only for the duration of the call (contents are copied).
/// The store must not be aliased by another thread while it runs.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_upsert(
    store: *mut HirariRegionVariAudioStore,
    segment: HirariAudioNoteSegment,
    anchors: *const HirariAudioNoteAnchor,
    anchor_count: usize,
) -> bool {
    if store.is_null() || !segment.valid() || anchor_count > 4096 {
        return false;
    }
    let mut entry = Segment {
        value: segment,
        anchors: Vec::new(),
        integral_prefix: Vec::new(),
        correction_before_seconds: 0.0,
    };
    if anchor_count > 0 {
        if anchors.is_null() {
            return false;
        }
        entry
            .anchors
            .extend_from_slice(std::slice::from_raw_parts(anchors, anchor_count));
        entry.normalize();
    }
    entry.rebuild_prefix();
    let store = &mut *store;
    let ranges = store.ranges();
    let slot = segment.slot(&ranges);
    // -2 means the interval conflicts with an existing segment: C++ rejects the
    // edit outright (`if (slot == -2) return false;`). The cap only guards
    // insertion; replacing a segment in place must stay possible at the cap.
    if slot == -2 {
        return false;
    }
    if slot < 0 {
        if store.segments.len() >= STORE_CAP {
            return false;
        }
        store.segments.push(entry);
    } else {
        store.segments[slot as usize] = entry;
    }
    store.rebuild_ordered();
    store.rebuild_derived();
    true
}

/// Anchor-level upsert matching `Track::upsertAudioNoteAnchor`:
/// 1 ns tolerance match, 4096 anchor cap unless replaced in place.
///
/// # Safety
/// `store` must be a live store. No other pointer into the store may be
/// live on another thread while the table is reordered.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_upsert_anchor(
    store: *mut HirariRegionVariAudioStore,
    segment_start: f64,
    pos: f64,
    cents: f64,
    formant: f64,
) -> bool {
    if store.is_null()
        || !segment_start.is_finite()
        || !pos.is_finite()
        || !cents.is_finite()
        || pos < 0.0
        || !formant.is_finite()
        || cents.abs() > 4800.0
        || formant.abs() > 2400.0
    {
        return false;
    }
    let store = &mut *store;
    let index = match store
        .segments
        .iter()
        .position(|segment| (segment.value.start_seconds - segment_start).abs() < 1.0e-9)
    {
        Some(index) => index,
        None => return false,
    };
    if store.segments[index].anchors.len() >= 4096
        && !store.segments[index]
            .anchors
            .iter()
            .any(|anchor| (anchor.position_seconds - pos).abs() < 1.0e-9)
    {
        return false;
    }
    let segment = &mut store.segments[index];
    let mut scratch: Vec<HirariAudioNoteAnchor> = Vec::with_capacity(segment.anchors.len() + 1);
    scratch.extend_from_slice(&segment.anchors);
    scratch.push(HirariAudioNoteAnchor {
        position_seconds: segment.value.end_seconds,
        pitch_cents: 0.0,
        formant_cents: 0.0,
    });
    let count = unsafe {
        hirari_audio_note_curve_upsert_anchor(
            scratch.as_mut_ptr(),
            segment.anchors.len(),
            scratch.len(),
            segment.value.start_seconds,
            segment.value.end_seconds,
            pos,
            cents,
            formant,
        )
    };
    if count == usize::MAX {
        return false;
    }
    scratch.truncate(count);
    segment.anchors = scratch;
    segment.rebuild_prefix();
    store.rebuild_derived();
    true
}

/// Timing warp matching `Track::warpAudioNoteSegment` including the
/// overlap-guard against every other segment.
///
/// # Safety
/// `store` must be a live store. No other pointer into the store may be
/// live on another thread while the table is reordered.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_warp(
    store: *mut HirariRegionVariAudioStore,
    segment_start: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    if store.is_null()
        || !segment_start.is_finite()
        || !new_start.is_finite()
        || !new_end.is_finite()
        || new_end <= new_start
    {
        return false;
    }
    let store = &mut *store;
    let index = match store
        .segments
        .iter()
        .position(|segment| (segment.value.start_seconds - segment_start).abs() < 1.0e-9)
    {
        Some(index) => index,
        None => return false,
    };
    if !store.segments[index].value.valid() {
        return false;
    }
    let ranges = store.ranges();
    if ranges.iter().enumerate().any(|(other, range)| {
        other != index && new_start < range.end_seconds && range.start_seconds < new_end
    }) {
        return false;
    }
    let old_start = store.segments[index].value.start_seconds;
    let old_end = store.segments[index].value.end_seconds;
    let segment = &mut store.segments[index];
    if !unsafe {
        hirari_audio_note_curve_warp_anchors(
            segment.anchors.as_mut_ptr(),
            segment.anchors.len(),
            old_start,
            old_end,
            new_start,
            new_end,
        )
    } {
        return false;
    }
    segment.value.start_seconds = new_start;
    segment.value.end_seconds = new_end;
    if !segment.value.valid() {
        return false;
    }
    segment.rebuild_prefix();
    store.rebuild_ordered();
    store.rebuild_derived();
    true
}

/// Remove a note segment by 1 ns positional match. Returns `true` iff removed.
///
/// # Safety
/// `store` must be a live store. No other pointer into the store may be
/// live on another thread while the table is reordered.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_remove(
    store: *mut HirariRegionVariAudioStore,
    start_seconds: f64,
) -> bool {
    if store.is_null() || !start_seconds.is_finite() {
        return false;
    }
    let store = &mut *store;
    let before = store.segments.len();
    store
        .segments
        .retain(|s| (s.value.start_seconds - start_seconds).abs() >= 1.0e-9);
    if store.segments.len() == before {
        return false;
    }
    store.rebuild_derived();
    true
}

/// Replace the entire table (undo restore / hydration / project load).
/// All entries must validate; the whole set is rejected otherwise.
///
/// # Safety
/// `store` must be a live store. When `segment_count > 0`, `segments`,
/// `anchor_offsets` and `anchor_counts` must be readable for
/// `segment_count` elements, and `anchors_flat` for `total` elements
/// where `total` is the sum of `anchor_counts`. All buffers stay
/// borrowed for the call only; the store copies what it keeps.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_set(
    store: *mut HirariRegionVariAudioStore,
    segments: *const HirariAudioNoteSegment,
    segment_count: usize,
    anchors_flat: *const HirariAudioNoteAnchor,
    anchor_offsets: *const usize,
    anchor_counts: *const usize,
) -> bool {
    if store.is_null() || segment_count > 4096 {
        return false;
    }
    if segment_count == 0 {
        (*store).segments.clear();
        return true;
    }
    if segments.is_null() || anchor_offsets.is_null() || anchor_counts.is_null() {
        return false;
    }
    let values = std::slice::from_raw_parts(segments, segment_count);
    let offsets = std::slice::from_raw_parts(anchor_offsets, segment_count);
    let counts = std::slice::from_raw_parts(anchor_counts, segment_count);
    let total: usize = counts.iter().sum();
    if total > 4096usize * 4096 {
        return false;
    }
    if total > 0 && anchors_flat.is_null() {
        return false;
    }
    let flat: &[HirariAudioNoteAnchor] = if total > 0 {
        std::slice::from_raw_parts(anchors_flat, total)
    } else {
        &[]
    };
    // Build first, publish second: one malformed entry must not leave the
    // store holding half of a rejected batch.
    let mut staged: Vec<Segment> = Vec::with_capacity(segment_count);
    for (index, value) in values.iter().enumerate() {
        if !value.valid() {
            return false;
        }
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
            entry.normalize();
        }
        entry.rebuild_prefix();
        staged.push(entry);
    }
    let store = &mut *store;
    if !store.replace_raw(values, flat, offsets, counts) {
        return false;
    }
    store.segments = staged;
    store.rebuild_ordered();
    store.rebuild_derived();
    true
}

/// Replace the table for project load / region split / pitch analysis: the
/// batch is repaired instead of rejected. Mirrors `Track::normalizeRegionAdd`,
/// which drops malformed entries and orders what survives.
///
/// # Safety
/// `store` must be a live store. When `segment_count > 0`, `segments`,
/// `anchor_offsets` and `anchor_counts` must be readable for
/// `segment_count` elements, and `anchors_flat` for the anchor total
/// implied by `anchor_counts`.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_load(
    store: *mut HirariRegionVariAudioStore,
    segments: *const HirariAudioNoteSegment,
    segment_count: usize,
    anchors_flat: *const HirariAudioNoteAnchor,
    anchor_offsets: *const usize,
    anchor_counts: *const usize,
) -> bool {
    if store.is_null() || segment_count > 4096 {
        return false;
    }
    if segment_count == 0 {
        return hirari_region_vari_audio_store_clear_segments(store);
    }
    if segments.is_null() || anchor_offsets.is_null() || anchor_counts.is_null() {
        return false;
    }
    let values = std::slice::from_raw_parts(segments, segment_count);
    let offsets = std::slice::from_raw_parts(anchor_offsets, segment_count);
    let counts = std::slice::from_raw_parts(anchor_counts, segment_count);
    let total: usize = counts.iter().sum();
    if total > 4096usize * 4096 {
        return false;
    }
    if total > 0 && anchors_flat.is_null() {
        return false;
    }
    let flat: &[HirariAudioNoteAnchor] = if total > 0 {
        std::slice::from_raw_parts(anchors_flat, total)
    } else {
        &[]
    };
    let store = &mut *store;
    if !store.replace_raw(values, flat, offsets, counts) {
        return false;
    }
    store.normalize();
    true
}

/// Applies the region-add policy in place (cap, drop malformed segments,
/// normalize anchor windows, sort, rebuild phases).
///
/// # Safety
/// `store` must be a live store. No other pointer into the store may be
/// live on another thread while the table is reordered.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_normalize(
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    if store.is_null() {
        return false;
    }
    (*store).normalize();
    true
}

/// Table sizes, so a caller can allocate exactly once before materializing.
///
/// # Safety
/// `store` must be null or a live store. Both out-parameters must be
/// writable.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_sizes(
    store: *const HirariRegionVariAudioStore,
    out_segment_count: *mut usize,
    out_anchor_total: *mut usize,
) -> bool {
    if out_segment_count.is_null() || out_anchor_total.is_null() {
        return false;
    }
    if store.is_null() {
        *out_segment_count = 0;
        *out_anchor_total = 0;
        return true;
    }
    let store = &*store;
    *out_segment_count = store.segments.len();
    *out_anchor_total = store.segments.iter().map(|s| s.anchors.len()).sum();
    true
}

/// Clear without freeing the Rust store (production clear path).
///
/// # Safety
/// `store` must be a live store. Any frozen snapshot taken earlier stays
/// valid, so the audio thread may keep reading it.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_clear_segments(
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    if store.is_null() {
        return false;
    }
    let store = &mut *store;
    if store.segments.is_empty() {
        return false;
    }
    store.segments.clear();
    true
}

/// Snapshot into a frozen, audio-readable projection.
///
/// # Safety
/// `store` must be a live store, exclusively borrowed for the call. The
/// result is a fresh snapshot; release it with
/// `hirari_region_vari_audio_frozen_free` once no reader keeps it.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_freeze(
    store: *const HirariRegionVariAudioStore,
) -> *mut HirariVariAudioFrozen {
    if store.is_null() {
        return std::ptr::null_mut();
    }
    let frozen = HirariVariAudioFrozen::build(&*store);
    Box::into_raw(Box::new(frozen))
}

/// Release a frozen snapshot.
///
/// # Safety
/// `frozen` must be null or come from `hirari_region_vari_audio_freeze`,
/// and no thread may still be reading the views it handed out.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_frozen_free(frozen: *mut HirariVariAudioFrozen) {
    if frozen.is_null() {
        return;
    }
    let _ = Box::from_raw(frozen);
}

/// Ranges view of a frozen snapshot (audio thread reads these values).
///
/// # Safety
/// `frozen` must be a live snapshot. The returned pointer and `*out_count`
/// stay valid until the snapshot is freed, so the audio thread may read
/// them without a lock.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_frozen_ranges(
    frozen: *const HirariVariAudioFrozen,
    out_count: *mut usize,
) -> *const HirariAudioNoteSegmentRange {
    if frozen.is_null() {
        return std::ptr::null();
    }
    let frozen = &*frozen;
    if !out_count.is_null() {
        *out_count = frozen.ranges.len();
    }
    frozen.ranges.as_ptr()
}

/// Curve views of a frozen snapshot (audio thread reads these values).
///
/// # Safety
/// `frozen` must be a live snapshot. The returned views reference the
/// snapshot's own backing arrays and stay valid until the snapshot is
/// freed, so the audio thread may read them without a lock.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_frozen_curves(
    frozen: *const HirariVariAudioFrozen,
    out_count: *mut usize,
) -> *const HirariAudioNoteCurveView {
    if frozen.is_null() {
        return std::ptr::null();
    }
    let frozen = &*frozen;
    if !out_count.is_null() {
        *out_count = frozen.curves.len();
    }
    frozen.curves.as_ptr()
}

/// Materialize the full table for serialization, split, and undo.
/// Anchors are flattened in segment order; `anchor_offsets[i]` and
/// `anchor_counts[i]` describe the layout.
///
/// # Safety
/// `store` must be a live store. `out_segment_count` and
/// `out_anchor_total` must be writable. On success `out_segments` needs
/// `segment_count` elements, `out_anchors` the returned anchor total,
/// and `out_anchor_offsets`/`out_anchor_counts` one element per
/// segment.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_materialize(
    store: *const HirariRegionVariAudioStore,
    out_segments: *mut HirariAudioNoteSegment,
    segment_capacity: usize,
    out_anchors: *mut HirariAudioNoteAnchor,
    anchor_capacity: usize,
    out_anchor_offsets: *mut usize,
    out_anchor_counts: *mut usize,
    out_segment_count: *mut usize,
    out_anchor_total: *mut usize,
) -> bool {
    if store.is_null() || out_segment_count.is_null() || out_anchor_total.is_null() {
        return false;
    }
    let store = &*store;
    let needed_anchors: usize = store.segments.iter().map(|s| s.anchors.len()).sum();
    // Validate every capacity before publishing a single count: a caller that
    // reads the outputs after a `false` return must not mix a stale count with
    // a buffer that was never filled.
    if store.segments.len() > segment_capacity || needed_anchors > anchor_capacity {
        return false;
    }
    if !store.segments.is_empty()
        && (out_segments.is_null() || out_anchor_offsets.is_null() || out_anchor_counts.is_null())
    {
        return false;
    }
    if needed_anchors > 0 && out_anchors.is_null() {
        return false;
    }
    *out_segment_count = store.segments.len();
    *out_anchor_total = needed_anchors;
    let mut anchor_written = 0usize;
    for (index, segment) in store.segments.iter().enumerate() {
        *out_segments.add(index) = segment.value;
        *out_anchor_offsets.add(index) = anchor_written;
        *out_anchor_counts.add(index) = segment.anchors.len();
        for anchor in segment.anchors.iter() {
            *out_anchors.add(anchor_written) = *anchor;
            anchor_written += 1;
        }
    }
    true
}

/// Number of stored segments.
///
/// # Safety
/// `store` must be null or a live store.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_count(
    store: *const HirariRegionVariAudioStore,
) -> u32 {
    if store.is_null() {
        return 0;
    }
    (*store).segments.len() as u32
}

/// Copy segment at `index` into `out_segment`. Returns `true` on success.
///
/// # Safety
/// `store` must be a live store and `out_segment` writable.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_get(
    store: *const HirariRegionVariAudioStore,
    index: u32,
    out_segment: *mut HirariAudioNoteSegment,
) -> bool {
    if store.is_null() || out_segment.is_null() || index as usize >= (*store).segments.len() {
        return false;
    }
    let store = &*store;
    let segment = &store.segments[index as usize];
    *out_segment = segment.value;
    true
}

/// Copy the stored segments into `out_ranges` (a caller-allocated buffer of
/// `count` `HirariAudioNoteSegmentRange`). Returns the number of segments copied,
/// or 0 on any failure.
///
/// # Safety
/// `store` must be a live store. When `count > 0`, `out_ranges` must be
/// writable for `count` elements; only the returned prefix is written.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_ranges(
    store: *const HirariRegionVariAudioStore,
    out_ranges: *mut HirariAudioNoteSegmentRange,
    count: usize,
) -> u32 {
    if store.is_null() || count == 0 || out_ranges.is_null() {
        return 0;
    }
    let ranges = slice::from_raw_parts_mut(out_ranges, count);
    let count = (*store).segments.len().min(count);
    for (index, segment) in (*store).segments.iter().take(count).enumerate() {
        ranges[index] = HirariAudioNoteSegmentRange {
            start_seconds: segment.value.start_seconds,
            end_seconds: segment.value.end_seconds,
            detected_pitch_cents: segment.value.detected_pitch_cents,
        };
    }
    count as u32
}

/// Differential characterization of the Rust note-store the C++ engine now
/// routes its note edits through: insert and replacement, overlap rejection,
/// sort order, removal, anchor upsert/warp, the frozen projection, batch
/// load/normalize and an empty/malformed input control.
#[cfg(test)]
mod vari_audio_store_tests {
    use super::*;

    const NONE: *const HirariAudioNoteAnchor = std::ptr::null();

    fn store() -> *mut HirariRegionVariAudioStore {
        unsafe { hirari_region_vari_audio_store_new() }
    }

    fn segment(start: f64, end: f64, pitch: f64) -> HirariAudioNoteSegment {
        HirariAudioNoteSegment {
            start_seconds: start,
            end_seconds: end,
            detected_pitch_cents: pitch,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        }
    }

    fn anchor(position: f64, cents: f64) -> HirariAudioNoteAnchor {
        HirariAudioNoteAnchor {
            position_seconds: position,
            pitch_cents: cents,
            formant_cents: 0.0,
        }
    }

    fn drop_store(store: *mut HirariRegionVariAudioStore) {
        unsafe { hirari_region_vari_audio_store_clear(store) };
    }

    fn upsert(store: *mut HirariRegionVariAudioStore, value: HirariAudioNoteSegment) -> bool {
        unsafe { hirari_region_vari_audio_store_upsert(store, value, NONE, 0) }
    }

    fn count(store: *mut HirariRegionVariAudioStore) -> u32 {
        unsafe { hirari_region_vari_audio_store_count(store) }
    }

    fn get(store: *mut HirariRegionVariAudioStore, index: u32) -> HirariAudioNoteSegment {
        let mut out = segment(0.0, 0.0, 0.0);
        assert!(unsafe { hirari_region_vari_audio_store_get(store, index, &mut out) });
        out
    }

    fn starts(store: *mut HirariRegionVariAudioStore) -> Vec<f64> {
        (0..count(store))
            .map(|index| get(store, index).start_seconds)
            .collect()
    }

    #[test]
    fn insert_and_count() {
        let store = store();
        assert!(upsert(store, segment(1.0, 2.0, 0.0)));
        assert_eq!(count(store), 1);
        drop_store(store);
    }

    #[test]
    fn replace_same_range_keeps_order_and_count() {
        let store = store();
        assert!(upsert(store, segment(5.0, 6.0, 0.0)));
        assert!(upsert(store, segment(1.0, 2.0, 0.0)));
        assert!(upsert(store, segment(5.0, 6.0, 42.0)));
        assert_eq!(count(store), 2);
        assert_eq!(starts(store), vec![1.0, 5.0]);
        assert_eq!(get(store, 1).detected_pitch_cents, 42.0);
        drop_store(store);
    }

    #[test]
    fn sort_order_by_start_seconds() {
        let store = store();
        assert!(upsert(store, segment(10.0, 11.0, 0.0)));
        assert!(upsert(store, segment(1.0, 2.0, 0.0)));
        assert!(upsert(store, segment(5.0, 6.0, 0.0)));
        assert_eq!(count(store), 3);
        assert_eq!(starts(store), vec![1.0, 5.0, 10.0]);
        drop_store(store);
    }

    #[test]
    fn overlapping_insert_is_rejected_and_keeps_the_table() {
        let store = store();
        assert!(upsert(store, segment(1.0, 4.0, 0.0)));
        // Different start, overlapping interval -> C++ rejects the edit.
        assert!(!upsert(store, segment(2.0, 6.0, 0.0)));
        assert_eq!(count(store), 1);
        assert_eq!(starts(store), vec![1.0]);
        drop_store(store);
    }

    #[test]
    fn remove_existing_and_preserves_rest() {
        let store = store();
        assert!(upsert(store, segment(1.0, 2.0, 0.0)));
        assert!(upsert(store, segment(3.0, 4.0, 0.0)));
        assert!(unsafe { hirari_region_vari_audio_store_remove(store, 1.0) });
        assert_eq!(count(store), 1);
        assert_eq!(starts(store), vec![3.0]);
        drop_store(store);
    }

    #[test]
    fn rejection_of_out_of_bounds_and_empty() {
        let store = store();
        assert!(!upsert(
            store,
            HirariAudioNoteSegment {
                start_seconds: f64::NAN,
                end_seconds: 2.0,
                detected_pitch_cents: 0.0,
                pitch_offset_cents: 0.0,
                formant_offset_cents: 0.0,
            }
        ));
        // Inverted interval is rejected as well.
        assert!(!upsert(store, segment(5.0, 5.0, 0.0)));
        // A null anchor pointer with a non-zero count is rejected.
        assert!(!unsafe {
            hirari_region_vari_audio_store_upsert(store, segment(1.0, 2.0, 0.0), NONE, 4)
        });
        assert_eq!(count(store), 0);
        // Removal on a store without the segment is a no-op.
        assert!(!unsafe { hirari_region_vari_audio_store_remove(store, 1.0) });
        drop_store(store);
    }

    #[test]
    fn ranges_projection_matches_stored_segments() {
        let store = store();
        assert!(upsert(store, segment(0.0, 1.0, 0.0)));
        assert!(upsert(store, segment(5.0, 6.0, 0.0)));
        // A null/zero buffer copies nothing.
        assert_eq!(
            unsafe { hirari_region_vari_audio_store_ranges(store, std::ptr::null_mut(), 0) },
            0
        );
        let mut buf = [HirariAudioNoteSegmentRange::default(); 3];
        let count = unsafe { hirari_region_vari_audio_store_ranges(store, buf.as_mut_ptr(), 3) };
        assert_eq!(count, 2);
        assert_eq!(buf[0].start_seconds, 0.0);
        assert_eq!(buf[0].detected_pitch_cents, 0.0);
        assert_eq!(buf[1].start_seconds, 5.0);
        drop_store(store);
    }

    #[test]
    fn set_round_trips_through_materialize() {
        let store = store();
        let values = [segment(0.0, 4.0, 0.0), segment(4.0, 8.0, 7.0)];
        let anchors = [anchor(1.0, 10.0), anchor(3.0, -20.0), anchor(5.0, 30.0)];
        // Segment 0 owns anchors [1.0, 3.0]; segment 1 owns [5.0].
        let offsets = [0usize, 2];
        let counts = [2usize, 1];
        assert!(unsafe {
            hirari_region_vari_audio_store_set(
                store,
                values.as_ptr(),
                values.len(),
                anchors.as_ptr(),
                offsets.as_ptr(),
                counts.as_ptr(),
            )
        });
        assert_eq!(count(store), 2);

        let mut out_segments = [segment(0.0, 0.0, 0.0); 4];
        let mut out_anchors = [anchor(0.0, 0.0); 8];
        let mut out_offsets = [0usize; 4];
        let mut out_counts = [0usize; 4];
        let mut segment_count = 0usize;
        let mut anchor_total = 0usize;
        assert!(unsafe {
            hirari_region_vari_audio_store_materialize(
                store,
                out_segments.as_mut_ptr(),
                out_segments.len(),
                out_anchors.as_mut_ptr(),
                out_anchors.len(),
                out_offsets.as_mut_ptr(),
                out_counts.as_mut_ptr(),
                &mut segment_count,
                &mut anchor_total,
            )
        });
        assert_eq!(segment_count, 2);
        assert_eq!(anchor_total, 3);
        assert_eq!(out_counts[..2], [2, 1]);
        assert_eq!(out_anchors[0].pitch_cents, 10.0);
        assert_eq!(out_anchors[2].pitch_cents, 30.0);
        // Too small a buffer reports failure without touching the counts.
        assert!(!unsafe {
            hirari_region_vari_audio_store_materialize(
                store,
                out_segments.as_mut_ptr(),
                1,
                out_anchors.as_mut_ptr(),
                out_anchors.len(),
                out_offsets.as_mut_ptr(),
                out_counts.as_mut_ptr(),
                &mut segment_count,
                &mut anchor_total,
            )
        });
        assert_eq!(segment_count, 2);
        assert_eq!(anchor_total, 3);
        drop_store(store);
    }

    #[test]
    fn set_rejects_invalid_batches_and_overlaps() {
        let store = store();
        assert!(upsert(store, segment(0.0, 4.0, 0.0)));
        // One invalid entry rejects the whole batch.
        let bad = [segment(0.0, 4.0, 0.0), segment(f64::NAN, 8.0, 0.0)];
        assert!(!unsafe {
            hirari_region_vari_audio_store_set(
                store,
                bad.as_ptr(),
                bad.len(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        });
        assert_eq!(count(store), 1);
        // Overlapping neighbours are rejected once sorted.
        let overlapping = [segment(0.0, 6.0, 0.0), segment(4.0, 8.0, 0.0)];
        assert!(!unsafe {
            hirari_region_vari_audio_store_set(
                store,
                overlapping.as_ptr(),
                overlapping.len(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        });
        assert_eq!(count(store), 1);
        // An empty batch clears the table.
        assert!(unsafe {
            hirari_region_vari_audio_store_set(
                store,
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        });
        assert_eq!(count(store), 0);
        drop_store(store);
    }

    #[test]
    fn upsert_anchor_moves_and_warps_the_curve() {
        let store = store();
        assert!(upsert(store, segment(0.0, 10.0, 0.0)));
        assert!(unsafe {
            hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 2.0, 100.0, 0.0)
        });
        // Warping rescales the anchor position (2.0 -> 4.0 at 2x).
        assert!(unsafe { hirari_region_vari_audio_store_warp(store, 0.0, 0.0, 20.0) });
        let frozen = unsafe { hirari_region_vari_audio_freeze(store) };
        let mut curve_count = 0usize;
        let curves = unsafe { hirari_region_vari_audio_frozen_curves(frozen, &mut curve_count) };
        assert_eq!(curve_count, 1);
        let curves = unsafe { std::slice::from_raw_parts(curves, curve_count) };
        assert_eq!(curves[0].anchor_count, 1);
        let first = unsafe { curves[0].anchors.read() };
        assert!((first.position_seconds - 4.0).abs() < 1.0e-9);
        assert!((first.pitch_cents - 100.0).abs() < 1.0e-9);
        unsafe { hirari_region_vari_audio_frozen_free(frozen) };
        // Unknown segment and out-of-clamp values are rejected.
        assert!(!unsafe {
            hirari_region_vari_audio_store_upsert_anchor(store, 99.0, 1.0, 100.0, 0.0)
        });
        assert!(!unsafe {
            hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 1.0, 9999.0, 0.0)
        });
        drop_store(store);
    }

    #[test]
    fn freeze_views_stay_contiguous_for_every_segment() {
        let store = store();
        let values = [segment(0.0, 4.0, 0.0), segment(4.0, 8.0, 0.0)];
        let anchors = [anchor(1.0, 10.0), anchor(2.0, 20.0), anchor(3.0, 30.0)];
        let offsets = [0usize, 3];
        let counts = [3usize, 0];
        assert!(unsafe {
            hirari_region_vari_audio_store_set(
                store,
                values.as_ptr(),
                values.len(),
                anchors.as_ptr(),
                offsets.as_ptr(),
                counts.as_ptr(),
            )
        });
        let frozen = unsafe { hirari_region_vari_audio_freeze(store) };
        let mut curve_count = 0usize;
        let curves = unsafe { hirari_region_vari_audio_frozen_curves(frozen, &mut curve_count) };
        assert_eq!(curve_count, 2);
        let curves = unsafe { std::slice::from_raw_parts(curves, curve_count) };
        let stride = std::mem::size_of::<HirariAudioNoteAnchor>();
        // Curve 0 must still address its own anchors inside the snapshot.
        assert_eq!(
            curves[1].anchors as usize,
            curves[0].anchors as usize + stride * curves[0].anchor_count
        );
        let anchors =
            unsafe { std::slice::from_raw_parts(curves[0].anchors, curves[0].anchor_count) };
        assert_eq!(anchors[0].pitch_cents, 10.0);
        assert_eq!(anchors[2].pitch_cents, 30.0);
        unsafe { hirari_region_vari_audio_frozen_free(frozen) };
        drop_store(store);
    }

    #[test]
    fn load_repairs_instead_of_rejecting() {
        let store = store();
        let bad = [segment(f64::NAN, 4.0, 0.0), segment(0.0, 4.0, 0.0)];
        let counts = [1usize, 0];
        let offsets = [0usize, 1];
        let anchors = [anchor(9.0, 5.0)]; // outside segment 0 -> dropped
        assert!(unsafe {
            hirari_region_vari_audio_store_load(
                store,
                bad.as_ptr(),
                bad.len(),
                anchors.as_ptr(),
                offsets.as_ptr(),
                counts.as_ptr(),
            )
        });
        assert_eq!(count(store), 1);
        assert_eq!(starts(store), vec![0.0]);
        let mut segment_count = 0usize;
        let mut anchor_total = 0usize;
        assert!(unsafe {
            hirari_region_vari_audio_store_sizes(store, &mut segment_count, &mut anchor_total)
        });
        assert_eq!(segment_count, 1);
        // The out-of-window anchor was dropped by the normalize policy.
        assert_eq!(anchor_total, 0);
        drop_store(store);
    }

    #[test]
    fn normalize_caps_and_orders_the_table() {
        let store = store();
        let mut values = Vec::new();
        for index in 0..8 {
            values.push(segment(8.0 - index as f64, 9.0 - index as f64, 0.0));
        }
        values.push(segment(f64::NAN, 12.0, 0.0));
        let counts: Vec<usize> = values.iter().map(|_| 0).collect();
        let offsets: Vec<usize> = counts.clone();
        assert!(unsafe {
            hirari_region_vari_audio_store_load(
                store,
                values.as_ptr(),
                values.len(),
                NONE,
                offsets.as_ptr(),
                counts.as_ptr(),
            )
        });
        assert!(unsafe { hirari_region_vari_audio_store_normalize(store) });
        assert_eq!(count(store), 8);
        let ordered = starts(store);
        assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
        drop_store(store);
    }

    #[test]
    fn clear_segments_reports_whether_anything_was_dropped() {
        let store = store();
        assert!(!unsafe { hirari_region_vari_audio_store_clear_segments(store) });
        assert!(upsert(store, segment(0.0, 4.0, 0.0)));
        assert!(unsafe { hirari_region_vari_audio_store_clear_segments(store) });
        assert_eq!(count(store), 0);
        drop_store(store);
    }

    #[test]
    fn store_handle_rejects_null() {
        assert_eq!(
            unsafe { hirari_region_vari_audio_store_count(std::ptr::null()) },
            0
        );
        assert!(!upsert(std::ptr::null_mut(), segment(0.0, 1.0, 0.0)));
        assert!(unsafe { hirari_region_vari_audio_freeze(std::ptr::null()) }.is_null());
    }
}
