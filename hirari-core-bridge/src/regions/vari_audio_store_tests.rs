//! Tests split out of vari_audio.rs.
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
fn clone_is_independent_and_null_safe() {
    let store = store();
    assert!(upsert(store, segment(1.0, 2.0, 0.0)));
    let copy = unsafe { hirari_region_vari_audio_store_clone(store) };
    assert!(!copy.is_null());
    assert_eq!(count(copy), 1);
    assert!(upsert(copy, segment(3.0, 4.0, 0.0)));
    // Editing the copy must not touch the source table.
    assert_eq!(count(copy), 2);
    assert_eq!(count(store), 1);
    assert!(unsafe { hirari_region_vari_audio_store_clone(std::ptr::null()) }.is_null());
    drop_store(copy);
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
    assert!(unsafe { hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 2.0, 100.0, 0.0) });
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
    assert!(!unsafe { hirari_region_vari_audio_store_upsert_anchor(store, 99.0, 1.0, 100.0, 0.0) });
    assert!(!unsafe { hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 1.0, 9999.0, 0.0) });
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
    let anchors = unsafe { std::slice::from_raw_parts(curves[0].anchors, curves[0].anchor_count) };
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
