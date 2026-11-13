//! Phase-prefix snapshot cache, split out of vari_audio.rs.
use super::*;

pub(crate) fn rebuild_phase_prefixes(store: &mut HirariRegionVariAudioStore) {
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

/// Deep-copies a store into a fresh, uniquely-owned one. Analysis workers
/// take snapshots through this so the engine's live table keeps being edited
/// while the worker compares before/after states.
///
/// # Safety
/// `store` must be null or a live store from
/// `hirari_region_vari_audio_store_new`; it is only read.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_clone(
    store: *const HirariRegionVariAudioStore,
) -> *mut HirariRegionVariAudioStore {
    if store.is_null() {
        return std::ptr::null_mut();
    }
    let source = unsafe { &*store };
    Box::into_raw(Box::new(HirariRegionVariAudioStore {
        segments: source.segments.clone(),
    }))
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

/// Splits one region's note table at a timeline boundary, writing the two
/// halves into fresh stores. This is the policy the C++ split used to carry:
/// segments are windowed, boundary anchors are sampled from the source curve,
/// and each half is repaired/ordered by the same load path as project load.
///
/// # Safety
/// `source` must be live; `left` and `right` must be distinct, writable stores.
pub(crate) unsafe fn vari_store_split(
    source: *const HirariRegionVariAudioStore,
    left: *mut HirariRegionVariAudioStore,
    right: *mut HirariRegionVariAudioStore,
    sample_rate: f64,
    split_offset: u64,
    region_length: u64,
) -> bool {
    if source.is_null() || left.is_null() || right.is_null() || left == right {
        return false;
    }
    let source = &*source;
    let entries: Vec<SourceSegment> = source
        .segments
        .iter()
        .map(|segment| SourceSegment {
            value: segment.value,
            anchors: &segment.anchors,
            pitch_offset_cents: segment.value.pitch_offset_cents,
            formant_offset_cents: segment.value.formant_offset_cents,
        })
        .collect();
    let (left_entries, right_entries) =
        split_source_segments(&entries, sample_rate, split_offset, region_length);
    let left_ok = load_entries(left, &left_entries);
    let right_ok = load_entries(right, &right_entries);
    left_ok && right_ok
}

/// Splits a published note table. The engine splits from the immutable
/// projection so a concurrent note edit cannot mutate the table mid-read; the
/// snapshot keeps its own backing arrays alive for the duration of the call.
///
/// # Safety
/// `source` must be live; `left` and `right` must be distinct, writable stores.
pub(crate) unsafe fn vari_frozen_split(
    source: *const HirariVariAudioFrozen,
    left: *mut HirariRegionVariAudioStore,
    right: *mut HirariRegionVariAudioStore,
    sample_rate: f64,
    split_offset: u64,
    region_length: u64,
) -> bool {
    if source.is_null() || left.is_null() || right.is_null() || left == right {
        return false;
    }
    let source = &*source;
    if source.ranges.len() != source.curves.len() {
        return false;
    }
    let mut entries = Vec::with_capacity(source.ranges.len());
    for (range, curve) in source.ranges.iter().zip(source.curves.iter()) {
        let anchors = if curve.anchor_count == 0 {
            &[][..]
        } else if curve.anchors.is_null() {
            return false;
        } else {
            unsafe { slice::from_raw_parts(curve.anchors, curve.anchor_count) }
        };
        entries.push(SourceSegment {
            value: HirariAudioNoteSegment {
                start_seconds: range.start_seconds,
                end_seconds: range.end_seconds,
                detected_pitch_cents: range.detected_pitch_cents,
                pitch_offset_cents: curve.pitch_offset_cents,
                formant_offset_cents: curve.formant_offset_cents,
            },
            anchors,
            pitch_offset_cents: curve.pitch_offset_cents,
            formant_offset_cents: curve.formant_offset_cents,
        });
    }
    let (left_entries, right_entries) =
        split_source_segments(&entries, sample_rate, split_offset, region_length);
    let left_ok = load_entries(left, &left_entries);
    let right_ok = load_entries(right, &right_entries);
    left_ok && right_ok
}

/// C ABI for [`vari_store_split`].
///
/// # Safety
/// `source` must be live; `left` and `right` must be distinct, writable stores.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_split(
    source: *const HirariRegionVariAudioStore,
    left: *mut HirariRegionVariAudioStore,
    right: *mut HirariRegionVariAudioStore,
    sample_rate: f64,
    split_offset: u64,
    region_length: u64,
) -> bool {
    unsafe {
        vari_store_split(
            source,
            left,
            right,
            sample_rate,
            split_offset,
            region_length,
        )
    }
}

/// C ABI for [`vari_frozen_split`].
///
/// # Safety
/// `source` must be a live snapshot; `left` and `right` must be distinct,
/// writable stores.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_frozen_split(
    source: *const HirariVariAudioFrozen,
    left: *mut HirariRegionVariAudioStore,
    right: *mut HirariRegionVariAudioStore,
    sample_rate: f64,
    split_offset: u64,
    region_length: u64,
) -> bool {
    unsafe {
        vari_frozen_split(
            source,
            left,
            right,
            sample_rate,
            split_offset,
            region_length,
        )
    }
}
