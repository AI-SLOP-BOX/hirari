//! Tests for the region edit host, split out of region_edit_host.rs.
//! Tests for the region edit host, split out of region_edit_host.rs.
use super::*;

    use std::slice;

use crate::vari_audio::{
    hirari_region_vari_audio_store_new, HirariRegionVariAudioStore,
};
use crate::audio_note_curve::HirariAudioNoteAnchor;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        present: bool,
        state: HirariRegionState,
        applied: Vec<HirariRegionEditPayload>,
        applied_blobs: Vec<Vec<u8>>,
        undo: Vec<HirariRegionEditPayload>,
        redo: Vec<HirariRegionEditPayload>,
        undo_name: String,
        coalesce: bool,
        perform: bool,
        note_store: usize,
        warp_markers: Vec<HirariWarpMarker>,
        undo_markers: Vec<HirariWarpMarker>,
        redo_markers: Vec<HirariWarpMarker>,
        range_edits: Vec<NativeRangeEdit>,
        comp_ranges: Vec<NativeCompRange>,
        comp_managed: bool,
        cloned_states: Vec<HirariRegionState>,
    }

    static FAKE: OnceLock<Mutex<FakeHost>> = OnceLock::new();
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn fake() -> &'static Mutex<FakeHost> {
        FAKE.get_or_init(|| Mutex::new(FakeHost::default()))
    }

    unsafe extern "C" fn fake_find(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut HirariRegionState,
    ) -> bool {
        let guard = fake().lock().unwrap();
        if !guard.present {
            return false;
        }
        unsafe { out.write(guard.state) };
        true
    }

    unsafe extern "C" fn fake_apply(
        _host: *mut c_void,
        payload: *const HirariRegionEditPayload,
    ) -> bool {
        if FAIL_APPLY.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let payload = unsafe { ptr::read(payload) };
        let blob = if payload.blob_len != 0 && !payload.blob.is_null() {
            unsafe { slice::from_raw_parts(payload.blob, payload.blob_len) }.to_vec()
        } else {
            Vec::new()
        };
        let mut guard = fake().lock().unwrap();
        guard.applied_blobs.push(blob);
        guard.applied.push(payload);
        true
    }

    static FAIL_APPLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static RELEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static SNAPSHOT_TARGET: u8 = 0;

    unsafe extern "C" fn fake_snapshot(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
    ) -> *mut c_void {
        &SNAPSHOT_TARGET as *const u8 as *mut c_void
    }

    unsafe extern "C" fn fake_release(_host: *mut c_void, _handle: *mut c_void) {
        RELEASED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    unsafe extern "C" fn fake_list(
        _host: *mut c_void,
        _group: u32,
        out: *mut HirariRegionGroupMember,
        capacity: usize,
    ) -> usize {
        if out.is_null() || capacity < 2 {
            return 2;
        }
        let state = fake().lock().unwrap().state;
        unsafe {
            out.write(HirariRegionGroupMember {
                track_id: 1,
                region_id: 2,
                start: state.start,
                base_start: state.base_start,
                locked: false,
            });
            out.add(1).write(HirariRegionGroupMember {
                track_id: 1,
                region_id: 3,
                start: state.start + 100,
                base_start: state.base_start + 100,
                locked: false,
            });
        }
        2
    }

    unsafe extern "C" fn fake_note_edit(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        context: *mut c_void,
        mutate: HirariRegionNoteEditFn,
    ) -> bool {
        let store = {
            let guard = fake().lock().unwrap();
            if !guard.present {
                return false;
            }
            guard.note_store as *mut HirariRegionVariAudioStore
        };
        if store.is_null() {
            return false;
        }
        unsafe { mutate(context, store) }
    }

    unsafe extern "C" fn fake_record_undo(
        _host: *mut c_void,
        name: *const c_char,
        undo: *const HirariRegionEditPayload,
        undo_count: usize,
        redo: *const HirariRegionEditPayload,
        redo_count: usize,
        coalesce: bool,
        perform: bool,
    ) -> bool {
        let mut guard = fake().lock().unwrap();
        guard.coalesce = coalesce;
        guard.perform = perform;
        guard.undo_name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let undo = unsafe { slice::from_raw_parts(undo, undo_count) }.to_vec();
        let redo = unsafe { slice::from_raw_parts(redo, redo_count) }.to_vec();
        let copy_markers = |payloads: &[HirariRegionEditPayload]| {
            payloads
                .iter()
                .find(|payload| {
                    payload.op == HIRARI_REGION_EDIT_GEOMETRY && !payload.blob.is_null()
                })
                .map(|payload| unsafe {
                    slice::from_raw_parts(
                        payload.blob.cast::<HirariWarpMarker>(),
                        payload.blob_len / std::mem::size_of::<HirariWarpMarker>(),
                    )
                    .to_vec()
                })
                .unwrap_or_default()
        };
        guard.undo_markers = copy_markers(&undo);
        guard.redo_markers = copy_markers(&redo);
        guard.undo = undo;
        guard.redo = redo;
        true
    }

    unsafe extern "C" fn fake_range_edits(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut NativeRangeEdit,
        capacity: usize,
    ) -> usize {
        let guard = fake().lock().unwrap();
        let count = guard.range_edits.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.range_edits.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_markers(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut HirariWarpMarker,
        capacity: usize,
    ) -> usize {
        let guard = fake().lock().unwrap();
        let count = guard.warp_markers.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.warp_markers.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_comp_ranges(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut NativeCompRange,
        capacity: usize,
        managed: *mut bool,
    ) -> usize {
        let guard = fake().lock().unwrap();
        if !managed.is_null() {
            unsafe { *managed = guard.comp_managed };
        }
        let count = guard.comp_ranges.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.comp_ranges.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_clone(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        state: *const HirariRegionState,
        new_region_id: u32,
    ) -> u32 {
        let mut guard = fake().lock().unwrap();
        guard.cloned_states.push(if state.is_null() {
            HirariRegionState::default()
        } else {
            unsafe { *state }
        });
        new_region_id
    }

    unsafe extern "C" fn fake_list_tracks(_host: *mut c_void, out: *mut u32, capacity: usize) -> usize {
        if out.is_null() || capacity < 1 {
            return 1;
        }
        unsafe { out.write(1) };
        1
    }

    unsafe extern "C" fn fake_list_regions(
        _host: *mut c_void,
        track_id: u32,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        if track_id != 1 {
            return 0;
        }
        if out.is_null() || capacity < 2 {
            return 2;
        }
        unsafe {
            out.write(1);
            out.add(1).write(2);
        }
        2
    }

    unsafe extern "C" fn fake_retain_track(_host: *mut c_void, _track_id: u32) -> *mut c_void {
        1usize as *mut c_void
    }

    unsafe extern "C" fn fake_release_track(_handle: *mut c_void) {}

    unsafe extern "C" fn fake_note_snapshot(
        _handle: *mut c_void,
        _region_id: u32,
    ) -> *mut c_void {
        hirari_region_vari_audio_store_new().cast()
    }

    static FAKE_AUDIO: u8 = 0;
    static FAIL_DECODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static RELEASED_AUDIO: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static FAKE_RATE_BITS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(48_000f64.to_bits());
    static FAKE_AUDIO_SAMPLES: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(48_000);

    fn fake_acquire(
        _path: *const u8,
        _length: usize,
        rate: *mut f64,
        handle: *mut *mut c_void,
    ) -> i32 {
        if FAIL_DECODE.load(std::sync::atomic::Ordering::SeqCst) {
            return crate::audio_import_cache::HIRARI_IMPORT_NEEDS_DECODE;
        }
        unsafe {
            handle.write(&FAKE_AUDIO as *const u8 as *mut c_void);
            rate.write(44_100.0);
        }
        crate::audio_import_cache::HIRARI_IMPORT_CACHE_HIT
    }

    fn fake_import_decode(
        _path: *const u8,
        _length: usize,
    ) -> crate::preview_audio_runtime::NativeDecodedAudio {
        crate::preview_audio_runtime::NativeDecodedAudio {
            samples: ptr::null_mut(),
            sample_count: 0,
            channels: 0,
            sample_rate: 0.0,
            bits_per_sample: 0,
            status: 3,
        }
    }

    /// The import seam the replace-audio path reads: a cached hit hands back
    /// the marker handle, and a decode always reports an unusable file.
    fn fake_import_ops() -> crate::audio_import_cache::ImportOps {
        crate::audio_import_cache::ImportOps {
            acquire: fake_acquire,
            store: |_, _, _, _| true,
            create: |_| ptr::null_mut(),
            storage: |handle| handle,
            channels: |_| 1,
            frames: |_| FAKE_AUDIO_SAMPLES.load(std::sync::atomic::Ordering::SeqCst),
            weak: |_| ptr::null_mut(),
            release: |_| {},
            decode: fake_import_decode,
        }
    }

    unsafe extern "C" fn fake_release_audio(_handle: *mut c_void) {
        RELEASED_AUDIO.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_sample_rate(_host: *mut c_void) -> f64 {
        f64::from_bits(FAKE_RATE_BITS.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn fake_ops() -> HirariRegionHostOps {
        HirariRegionHostOps {
            find_region: Some(fake_find),
            apply_edit: Some(fake_apply),
            list_sync_group: Some(fake_list),
            record_undo: Some(fake_record_undo),
            note_edit: Some(fake_note_edit),
            snapshot_region: Some(fake_snapshot),
            release_snapshot: Some(fake_release),
            warp_markers: Some(fake_markers),
            range_edits: Some(fake_range_edits),
            sample_rate: Some(fake_sample_rate),
            release_audio: Some(fake_release_audio),
            clone_region: Some(fake_clone),
            comp_ranges: Some(fake_comp_ranges),
            list_tracks: Some(fake_list_tracks),
            list_regions: Some(fake_list_regions),
            retain_track: Some(fake_retain_track),
            release_track: Some(fake_release_track),
            note_snapshot: Some(fake_note_snapshot),
            ..Default::default()
        }
    }

    fn reset(state: HirariRegionState) {
        *fake().lock().unwrap() = FakeHost {
            present: true,
            state,
            ..Default::default()
        };
        // The Rust allocator keys counters by host, so every test restarts them
        // to keep the id it hands the fake host predictable.
        unsafe { crate::id_host::hirari_ids_reset_project(1usize as *mut c_void) };
    }

    #[test]
    fn scalar_edit_clamps_and_records_one_undo_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            clip_gain: 1.0,
            len: 100,
            start: 0,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 1, 2.5, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_INPLACE);
        assert!((guard.applied[0].state.clip_gain - 2.0).abs() < 1e-6);
        assert_eq!(guard.undo_name, "Region Gain");
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.redo.len(), 1);
        assert!((guard.undo[0].state.clip_gain - 1.0).abs() < 1e-6);
        assert!((guard.redo[0].state.clip_gain - 2.0).abs() < 1e-6);
        drop(guard);

    }

    #[test]
    fn scalar_edit_rejects_non_finite_without_applying() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(!edit_scalar_with(&ops, host, 1, 2, 1, f32::NAN, 0.0));
        assert!(fake().lock().unwrap().applied.is_empty());

    }

    fn trim_base_state() -> HirariRegionState {
        HirariRegionState {
            start: 1000,
            len: 1000,
            base_start: 1000,
            base_length: 2000,
            source_offset: 0,
            base_source_offset: 0,
            source_length: 0,
            loop_count: 1,
            fade_in_samples: 64,
            fade_out_samples: 64,
            warp_ratio: 1.0,
            ..Default::default()
        }
    }

    fn one_marker() -> HirariWarpMarker {
        HirariWarpMarker {
            source_sample: 10,
            timeline_sample: 12,
            transient: 0,
        }
    }

    #[test]
    fn trim_edit_applies_the_geometry_and_records_the_markers_for_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());
        fake().lock().unwrap().warp_markers = vec![one_marker()];

        assert!(edit_trim_with(&ops, host, 1, 2, 0.25, 0.75));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.applied[0].state.start, 1500);
        assert_eq!(guard.applied[0].state.len, 1000);
        assert_eq!(guard.applied[0].state.source_offset, 500);
        assert_eq!(guard.applied[0].state.source_length, 1000);
        assert!(guard.applied[0].blob.is_null());
        assert_eq!(guard.undo_name, "Trim Region");
        assert!(!guard.coalesce);
        assert_eq!(guard.undo[0].state.start, 1000);
        assert_eq!(guard.undo[0].state.source_offset, 0);
        assert_eq!(guard.undo_markers.len(), 1);
        assert_eq!(guard.undo_markers[0].source_sample, 10);
        assert_eq!(guard.undo_markers[0].timeline_sample, 12);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn trim_edit_clamps_the_fractions_and_rejects_inverted_or_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());

        assert!(!edit_trim_with(&ops, host, 1, 2, 0.8, 0.2));
        assert!(!edit_trim_with(&ops, host, 1, 2, f32::NAN, 0.5));
        assert!(fake().lock().unwrap().applied.is_empty());

        let mut locked = trim_base_state();
        locked.locked = true;
        reset(locked);
        assert!(!edit_trim_with(&ops, host, 1, 2, 0.25, 0.75));
        assert!(fake().lock().unwrap().applied.is_empty());

        // Out-of-range fractions clamp instead of rejecting, like the C++
        // wrapper did before handing them to the geometry transform.
        reset(trim_base_state());
        assert!(edit_trim_with(&ops, host, 1, 2, -0.5, 1.5));
        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo[0].state.start, 1000);
    }

    #[test]
    fn warp_ratio_edit_recomputes_the_length_and_clears_markers_on_redo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        let state = HirariRegionState {
            len: 1000,
            base_length: 1000,
            source_length: 1000,
            loop_count: 1,
            warp_ratio: 1.0,
            ..Default::default()
        };
        reset(state);
        fake().lock().unwrap().warp_markers = vec![one_marker()];

        assert!(edit_warp_ratio_with(&ops, host, 1, 2, 2.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.applied[0].state.len, 500);
        assert!((guard.applied[0].state.warp_ratio - 2.0).abs() < 1e-9);
        assert_eq!(guard.undo_name, "Flex Warp Ratio");
        assert_eq!(guard.undo[0].state.len, 1000);
        assert!((guard.undo[0].state.warp_ratio - 1.0).abs() < 1e-9);
        assert_eq!(guard.undo_markers.len(), 1);
        assert_eq!(guard.undo_markers[0].source_sample, 10);
        assert_eq!(guard.undo_markers[0].timeline_sample, 12);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn warp_ratio_edit_rejects_out_of_range_values() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());

        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, 0.49));
        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, 2.01));
        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, f64::NAN));
        assert!(fake().lock().unwrap().applied.is_empty());
    }

    #[test]
    fn remove_applies_then_records_a_restore_that_owns_the_snapshot() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { start: 10, len: 100, ..Default::default() });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);

        assert!(edit_remove_with(&ops, host, 1, 2));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo_name, "Remove Region");
        assert!(!guard.coalesce);
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.undo[0].blob, &SNAPSHOT_TARGET as *const u8);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert!(guard.redo[0].blob.is_null());
        // Ownership moved to the undo entry, so nothing was released here.
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_remove_releases_the_snapshot_and_records_nothing() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { len: 100, ..Default::default() });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);
        FAIL_APPLY.store(true, std::sync::atomic::Ordering::SeqCst);

        let removed = edit_remove_with(&ops, host, 1, 2);
        FAIL_APPLY.store(false, std::sync::atomic::Ordering::SeqCst);

        assert!(!removed);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(fake().lock().unwrap().undo.is_empty());
    }

    #[test]
    fn remove_rejects_zero_ids_and_missing_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { len: 100, ..Default::default() });
        assert!(!edit_remove_with(&ops, host, 0, 2));
        assert!(!edit_remove_with(&ops, host, 1, 0));
        fake().lock().unwrap().present = false;
        assert!(!edit_remove_with(&ops, host, 1, 2));
        assert!(fake().lock().unwrap().applied.is_empty());
    }
include!("region_edit_host_tests_second.rs");
include!("region_edit_host_tests_auxiliary.rs");
