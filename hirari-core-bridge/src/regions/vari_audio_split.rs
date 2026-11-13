//! Source-splitting math, split out of vari_audio.rs.
use super::*;

pub(crate) struct SourceSegment<'a> {
    pub(crate) value: HirariAudioNoteSegment,
    pub(crate) anchors: &'a [HirariAudioNoteAnchor],
    pub(crate) pitch_offset_cents: f64,
    pub(crate) formant_offset_cents: f64,
}

type SplitEntry = (HirariAudioNoteSegment, Vec<HirariAudioNoteAnchor>);

pub(crate) fn split_source_segments(
    source: &[SourceSegment<'_>],
    sample_rate: f64,
    split_offset: u64,
    region_length: u64,
) -> (Vec<SplitEntry>, Vec<SplitEntry>) {
    // Matches `std::max(1.0, m_sampleRate)`: non-finite rates collapse to 1.0
    // the way the C++ comparison did.
    let rate = if sample_rate.is_finite() {
        sample_rate.max(1.0)
    } else {
        1.0
    };
    let split_seconds = split_offset as f64 / rate;
    let region_seconds = region_length as f64 / rate;
    let mut left_entries = Vec::new();
    let mut right_entries = Vec::new();
    for segment in source {
        let left_end = segment.value.end_seconds.min(split_seconds);
        if left_end > segment.value.start_seconds {
            left_entries.push(window_segment(
                segment,
                segment.value.start_seconds,
                left_end,
                0.0,
            ));
        }
        let right_start = segment.value.start_seconds.max(split_seconds);
        let right_end = segment.value.end_seconds.min(region_seconds);
        if right_end > right_start {
            right_entries.push(window_segment(
                segment,
                right_start,
                right_end,
                split_seconds,
            ));
        }
    }
    (left_entries, right_entries)
}

/// Boundary anchors store the curve component (offset removed), sampled from
/// the exported `hirari_audio_note_curve_at`, exactly as the C++ splitter did.
pub(crate) fn window_segment(
    segment: &SourceSegment<'_>,
    start: f64,
    end: f64,
    shift: f64,
) -> SplitEntry {
    let value = HirariAudioNoteSegment {
        start_seconds: start - shift,
        end_seconds: end - shift,
        ..segment.value
    };
    if segment.anchors.is_empty() {
        return (value, Vec::new());
    }
    let boundary = |position: f64| {
        let mut output = [0.0f64; 2];
        unsafe {
            crate::audio_note_curve::hirari_audio_note_curve_at(
                segment.anchors.as_ptr(),
                segment.anchors.len(),
                position,
                segment.pitch_offset_cents,
                segment.formant_offset_cents,
                output.as_mut_ptr(),
            )
        };
        HirariAudioNoteAnchor {
            position_seconds: position,
            pitch_cents: output[0] - segment.pitch_offset_cents,
            formant_cents: output[1] - segment.formant_offset_cents,
        }
    };
    let mut anchors = Vec::with_capacity(segment.anchors.len() + 2);
    anchors.push(boundary(start));
    for anchor in segment.anchors {
        if anchor.position_seconds > start && anchor.position_seconds < end {
            anchors.push(*anchor);
        }
    }
    let end_anchor = boundary(end);
    anchors.push(end_anchor);
    for anchor in &mut anchors {
        anchor.position_seconds -= shift;
    }
    (value, anchors)
}

pub(crate) unsafe fn load_entries(
    store: *mut HirariRegionVariAudioStore,
    entries: &[(HirariAudioNoteSegment, Vec<HirariAudioNoteAnchor>)],
) -> bool {
    if entries.is_empty() {
        // An empty source splits into two empty tables. Clearing an already
        // empty store reports false, which is not a failure here.
        let _ = unsafe { hirari_region_vari_audio_store_clear_segments(store) };
        return true;
    }
    let mut values = Vec::with_capacity(entries.len());
    let mut offsets = Vec::with_capacity(entries.len());
    let mut counts = Vec::with_capacity(entries.len());
    let mut flat = Vec::new();
    for (value, anchors) in entries {
        offsets.push(flat.len());
        counts.push(anchors.len());
        flat.extend_from_slice(anchors);
        values.push(*value);
    }
    unsafe {
        hirari_region_vari_audio_store_load(
            store,
            values.as_ptr(),
            values.len(),
            if flat.is_empty() {
                std::ptr::null()
            } else {
                flat.as_ptr()
            },
            offsets.as_ptr(),
            counts.as_ptr(),
        )
    }
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
#[path = "vari_audio_store_tests.rs"]
mod vari_audio_store_tests;
