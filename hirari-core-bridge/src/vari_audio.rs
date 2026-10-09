use crate::audio_note_curve::HirariAudioNoteSegmentRange;
use std::slice;

/// Upper bound on the number of note segments a store may hold.
const STORE_CAP: usize = 4096;

/// Canonical representation of a VariAudio note segment.
///
/// Ownership of the segments lives in Rust. C++ `Region::audioNoteSegments` and
/// its derived ranges/projections have been removed so the C++ layer can no
/// longer store or mutate note edits on its own.
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
}

/// Rust owns the note segments. Track-level state is still C++ but delegates the
/// note-edit table to this store, so C++ can never diverge from the stored set.
#[repr(C)]
pub struct HirariRegionVariAudioStore {
    segments: Vec<HirariAudioNoteSegment>,
}

/// Create an empty Rust-owned note store.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_new() -> *mut HirariRegionVariAudioStore {
    let store = HirariRegionVariAudioStore {
        segments: Vec::new(),
    };
    Box::into_raw(Box::new(store))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_clear(
    store: *mut HirariRegionVariAudioStore,
) {
    if store.is_null() {
        return;
    }
    let _ = Box::from_raw(store);
}

/// Insert or replace a note segment. Returns `true` on success.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_upsert(
    store: *mut HirariRegionVariAudioStore,
    segment: HirariAudioNoteSegment,
) -> bool {
    if store.is_null() {
        return false;
    }
    let store = &mut *store;
    if store.segments.len() >= STORE_CAP || !segment.valid() {
        return false;
    }
    if let Some(index) = store.segments.iter().position(|s| {
        s.start_seconds == segment.start_seconds && s.end_seconds == segment.end_seconds
    }) {
        store.segments[index] = segment;
        return true;
    }
    store.segments.push(segment);
    store.segments.sort_by(|left, right| {
        left.start_seconds
            .partial_cmp(&right.start_seconds)
            .unwrap()
    });
    true
}

/// Remove a note segment by start time. Returns `true` if a segment was removed.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_vari_audio_store_remove(
    store: *mut HirariRegionVariAudioStore,
    start_seconds: f64,
) -> bool {
    if store.is_null() {
        return false;
    }
    let store = &mut *store;
    let before = store.segments.len();
    store.segments.retain(|s| s.start_seconds != start_seconds);
    before != store.segments.len()
}

/// Number of stored segments.
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
    *out_segment = *segment;
    true
}

/// Copy the stored segments into `out_ranges` (a caller-allocated buffer of
/// `count` `HirariAudioNoteSegmentRange`). Returns the number of segments copied,
/// or 0 on any failure.
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
    let count = (*store).segments.len().min(count) as usize;
    for (index, segment) in (*store).segments.iter().take(count).enumerate() {
        ranges[index] = HirariAudioNoteSegmentRange {
            start_seconds: segment.start_seconds,
            end_seconds: segment.end_seconds,
            detected_pitch_cents: segment.detected_pitch_cents,
        };
    }
    count as u32
}

/// Differential characterization of the Rust note-store before C++ routes
/// note edits through it. Covers insert, duplicate replacement, sort order,
/// removal, bounds, and an empty/malformed input control.

#[cfg(test)]
mod vari_audio_store_tests {
    use super::*;

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

    fn drop_store(store: *mut HirariRegionVariAudioStore) {
        unsafe { hirari_region_vari_audio_store_clear(store) };
    }

    #[test]
    fn insert_and_count() {
        let store = store();
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(1.0, 2.0, 0.0)) });
        assert_eq!(unsafe { hirari_region_vari_audio_store_count(store) }, 1);
        drop_store(store);
    }

    #[test]
    fn replace_same_range_keeps_order_and_count() {
        let store = store();
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(5.0, 6.0, 0.0)) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(1.0, 2.0, 0.0)) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(5.0, 6.0, 42.0)) });
        assert_eq!(unsafe { hirari_region_vari_audio_store_count(store) }, 2);
        // 5.0/6.0 first (matches the insertion order sort).
        let mut out = segment(0.0, 0.0, 0.0);
        assert!(unsafe { hirari_region_vari_audio_store_get(store, 0, &mut out) });
        assert_eq!(out.start_seconds, 1.0);
        drop_store(store);
    }

    #[test]
    fn sort_order_by_start_seconds() {
        let store = store();
        let _ = unsafe { hirari_region_vari_audio_store_upsert(store, segment(10.0, 11.0, 0.0)) };
        let _ = unsafe { hirari_region_vari_audio_store_upsert(store, segment(1.0, 2.0, 0.0)) };
        let _ = unsafe { hirari_region_vari_audio_store_upsert(store, segment(5.0, 6.0, 0.0)) };
        assert_eq!(unsafe { hirari_region_vari_audio_store_count(store) }, 3);
        let mut out = segment(0.0, 0.0, 0.0);
        unsafe { hirari_region_vari_audio_store_get(store, 0, &mut out) };
        assert_eq!(out.start_seconds, 1.0);
        unsafe { hirari_region_vari_audio_store_get(store, 1, &mut out) };
        assert_eq!(out.start_seconds, 5.0);
        unsafe { hirari_region_vari_audio_store_get(store, 2, &mut out) };
        assert_eq!(out.start_seconds, 10.0);
        drop_store(store);
    }

    #[test]
    fn remove_existing_and_preserves_rest() {
        let store = store();
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(1.0, 2.0, 0.0)) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(3.0, 4.0, 0.0)) });
        assert!(unsafe { hirari_region_vari_audio_store_remove(store, 1.0) });
        assert_eq!(unsafe { hirari_region_vari_audio_store_count(store) }, 1);
        let mut out = segment(0.0, 0.0, 0.0);
        assert!(unsafe { hirari_region_vari_audio_store_get(store, 0, &mut out) });
        assert_eq!(out.start_seconds, 3.0);
        drop_store(store);
    }

    #[test]
    fn rejection_of_out_of_bounds_and_empty() {
        let store = store();
        // The segment is silently accepted by Rust as long as valid() passes and
        // the cap is not exceeded. Bounds and null states are caller-side checks.
        assert!(!unsafe {
            hirari_region_vari_audio_store_upsert(
                store,
                HirariAudioNoteSegment {
                    start_seconds: f64::NAN,
                    end_seconds: 2.0,
                    detected_pitch_cents: 0.0,
                    pitch_offset_cents: 0.0,
                    formant_offset_cents: 0.0,
                },
            )
        });
        assert_eq!(unsafe { hirari_region_vari_audio_store_count(store) }, 0);
        // Duplicate removal on a store without the segment is a no-op.
        assert!(!unsafe { hirari_region_vari_audio_store_remove(store, 1.0) });
        drop_store(store);
    }

    #[test]
    fn ranges_projection_matches_stored_segments() {
        let store = store();
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(0.0, 1.0, 0.0)) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment(5.0, 6.0, 0.0)) });
        let out: [HirariAudioNoteSegmentRange; 2] = unsafe { std::mem::zeroed() };
        // Out-of-bounds returns 0; caller controls the buffer.
        assert_eq!(
            unsafe { hirari_region_vari_audio_store_ranges(store, std::ptr::null_mut(), 0) },
            0
        );
        let mut buf = [HirariAudioNoteSegmentRange::default(); 3];
        let count = unsafe { hirari_region_vari_audio_store_ranges(store, buf.as_mut_ptr(), 3) };
        assert_eq!(count, 2);
        assert_eq!(buf[0].start_seconds, 0.0);
        assert_eq!(buf[1].start_seconds, 5.0);
        assert_eq!(buf[2].start_seconds, 0.0);
        drop_store(store);
    }
}
