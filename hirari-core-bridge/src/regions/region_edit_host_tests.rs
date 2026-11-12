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

    #[test]
    fn missing_region_is_reported() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        fake().lock().unwrap().present = false;

        assert!(!edit_scalar_with(&ops, host, 1, 2, 1, 0.5, 0.0));

    }

    #[test]
    fn move_keeps_the_untrimmed_base_start_equal_to_start() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 10,
            len: 100,
            base_start: 10,
            source_offset: 5,
            base_source_offset: 5,
            warp_ratio: 1.0,
            ..Default::default()
        });

        assert!(edit_move_with(&ops, host, 1, 2, 4096.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_STATE);
        assert_eq!(guard.applied[0].state.start, 4096);
        assert_eq!(guard.applied[0].state.base_start, 4096);
        assert_eq!(guard.undo[0].state.start, 10);
        assert_eq!(guard.undo_name, "Move Region");
        drop(guard);

    }

    #[test]
    fn sync_group_move_applies_every_member() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            len: 50,
            base_start: 100,
            sync_group: 7,
            warp_ratio: 1.0,
            ..Default::default()
        });

        assert!(edit_move_sync_group_with(&ops, host, 1, 2, 400.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 2);
        assert_eq!(guard.undo.len(), 2);
        assert_eq!(guard.redo.len(), 2);
        assert_eq!(guard.applied[0].state.start, 400);
        assert_eq!(guard.applied[1].state.start, 500);
        assert_eq!(guard.undo_name, "Move Region Sync Group");
        drop(guard);

    }

    #[test]
    fn locking_a_region_ignores_the_lock_and_records_no_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            locked: true,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 8, 1.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(guard.applied[0].state.locked);
        assert!(guard.undo.is_empty());
        assert!(guard.redo.is_empty());
        drop(guard);
    }

    #[test]
    fn pitch_preserve_backfills_the_source_length_before_recording() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            source_length: 0,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 9, 1.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(guard.applied[0].state.pitch_preserve_warp);
        assert_eq!(guard.applied[0].state.source_length, 100);
        assert_eq!(guard.undo_name, "Preserve Warp Pitch");
        drop(guard);
    }

    #[test]
    fn note_anchor_edit_records_the_whole_table_before_and_after() {
        use crate::vari_audio::{
            hirari_region_vari_audio_store_new, hirari_region_vari_audio_store_upsert,
            HirariAudioNoteSegment,
        };

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 44_100,
            ..Default::default()
        });
        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 1.0,
            detected_pitch_cents: 0.0,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        assert!(unsafe {
            hirari_region_vari_audio_store_upsert(store, segment, ptr::null(), 0)
        });
        fake().lock().unwrap().note_store = store as usize;
        assert!(note_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 100.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_NOTE_STORE);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_NOTE_STORE);
        assert!(!guard.undo[0].blob.is_null());
        assert!(!guard.redo[0].blob.is_null());
        assert_eq!(guard.undo_name, "Edit Audio Pitch Curve 1:2");
        let before = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let after = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);

        unsafe {
            crate::vari_audio::vari_store_release(before);
            crate::vari_audio::vari_store_release(after);
            crate::vari_audio::vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    #[test]
    fn note_anchor_edit_rejects_out_of_range_input_before_touching_the_host() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());

        assert!(!note_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 5000.0, 0.0));
        assert!(!note_warp_with(&ops, host, 1, 2, 0.0, 1.0, 1.0));
        assert!(fake().lock().unwrap().undo.is_empty());
    }

    fn store_with_segment(detected: f64, anchor_position: f64) -> *mut HirariRegionVariAudioStore {
        use crate::vari_audio::{
            hirari_region_vari_audio_store_new, hirari_region_vari_audio_store_upsert,
            HirariAudioNoteSegment,
        };

        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 1.0,
            detected_pitch_cents: detected,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        let anchor = HirariAudioNoteAnchor {
            position_seconds: anchor_position,
            pitch_cents: 50.0,
            formant_cents: 0.0,
        };
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment, &anchor, 1) });
        store
    }

    #[test]
    fn note_segment_edit_keeps_the_detected_pitch_and_curve() {
        use crate::vari_audio::{hirari_region_vari_audio_store_get, vari_store_release};

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let store = store_with_segment(42.0, 0.5);
        fake().lock().unwrap().note_store = store as usize;

        assert!(note_segment_with(&ops, host, 1, 2, 0.0, 1.0, 100.0, -30.0));

        let mut stored = crate::vari_audio::HirariAudioNoteSegment::default();
        assert!(unsafe { hirari_region_vari_audio_store_get(store, 0, &mut stored) });
        assert_eq!(stored.detected_pitch_cents, 42.0);
        assert_eq!(stored.pitch_offset_cents, 100.0);
        assert_eq!(stored.formant_offset_cents, -30.0);
        assert_eq!(unsafe { crate::vari_audio::vari_store_view(store, 0.0) }.unwrap().anchors.len(), 1);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo_name, "Edit Audio Pitch Segments 1:2");
        assert!(guard.coalesce);
        let undo = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let redo = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);
        unsafe {
            vari_store_release(undo);
            vari_store_release(redo);
            vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    #[test]
    fn note_move_anchor_repositions_the_anchor() {
        use crate::vari_audio::vari_store_release;

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let store = store_with_segment(0.0, 0.5);
        fake().lock().unwrap().note_store = store as usize;

        assert!(note_move_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 0.8, 70.0, false));
        let view = unsafe { crate::vari_audio::vari_store_view(store, 0.0) }.unwrap();
        assert_eq!(view.anchors.len(), 1);
        assert!((view.anchors[0].position_seconds - 0.8).abs() < 1e-9);
        assert!((view.anchors[0].pitch_cents - 70.0).abs() < 1e-9);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo_name, "Move Audio Pitch Anchor 1:2");
        let undo = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let redo = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);
        unsafe {
            vari_store_release(undo);
            vari_store_release(redo);
            vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    fn marker(source: u64, timeline: u64, transient: u8) -> HirariWarpMarker {
        HirariWarpMarker {
            source_sample: source,
            timeline_sample: timeline,
            transient,
        }
    }

    #[test]
    fn warp_marker_edit_orders_markers_and_records_marker_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0)];

        let input = [marker(500, 500, 0), marker(100, 100, 1)];
        assert!(set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            input.as_ptr(),
            input.len()
        ));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.undo_name, "Edit Region Warp Markers");
        assert!(!guard.coalesce);
        assert!(same_markers(&guard.undo_markers, &[marker(0, 0, 0)]));
        assert!(same_markers(
            &guard.redo_markers,
            &[marker(100, 100, 1), marker(500, 500, 0)]
        ));
    }

    #[test]
    fn warp_marker_edit_rejects_dropped_markers_and_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });

        // A duplicate timeline anchor is dropped by normalization, so the
        // edit is rejected instead of silently shortening the list.
        let duplicate = [marker(0, 0, 0), marker(0, 0, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            duplicate.as_ptr(),
            duplicate.len()
        ));
        // Beyond the region's timeline length.
        let beyond = [marker(0, 0, 0), marker(1_100, 1_100, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            beyond.as_ptr(),
            beyond.len()
        ));
        // A null buffer with a non-zero count must not be read.
        assert!(!set_warp_markers_with(&ops, host, 1, 2, ptr::null(), 1));
        assert!(fake().lock().unwrap().applied.is_empty());

        fake().lock().unwrap().state.locked = true;
        let valid = [marker(0, 0, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len()
        ));
    }

    #[test]
    fn warp_marker_edit_accepts_an_empty_list_as_a_clear() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0), marker(10, 10, 0)];

        assert!(set_warp_markers_with(&ops, host, 1, 2, ptr::null(), 0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert!(guard.applied[0].blob.is_null());
        assert_eq!(guard.applied[0].blob_len, 0);
        assert_eq!(guard.undo_markers.len(), 2);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn packed_warp_markers_convert_the_parallel_slices() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0)];

        let source = [500u64, 100];
        let timeline = [500u64, 100];
        let flags = [0u8, 1];
        assert!(set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            source.as_ptr(),
            source.len(),
            timeline.as_ptr(),
            timeline.len(),
            flags.as_ptr(),
            flags.len(),
        ));

        let guard = fake().lock().unwrap();
        assert!(same_markers(
            &guard.redo_markers,
            &[marker(100, 100, 1), marker(500, 500, 0)]
        ));
    }

    #[test]
    fn packed_warp_markers_reject_disagreeing_lengths_and_oversized_lists() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let source = [1u64, 2];
        let timeline = [1u64];
        let flags = [1u8, 1];
        assert!(!set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            source.as_ptr(),
            source.len(),
            timeline.as_ptr(),
            timeline.len(),
            flags.as_ptr(),
            flags.len(),
        ));
        // The cap is enforced before any allocation, so null slices with an
        // oversized count are rejected without touching memory.
        assert!(!set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            ptr::null(),
            100_001,
            ptr::null(),
            100_001,
            ptr::null(),
            100_001,
        ));
    }

    #[test]
    fn range_edit_upsert_stores_the_list_and_performs_the_undo_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(set_range_edit_with(&ops, host, 1, 2, 10, 50, 0.5, 2, 2));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_RANGE_EDITS);
        assert_eq!(guard.applied[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
        assert_eq!(guard.undo_name, "Region Range Edit");
        assert!(guard.perform, "range edits keep the native perform path");
        assert!(guard.coalesce);
        assert_eq!(guard.undo[0].blob_len, 0);
        assert_eq!(guard.redo[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
    }

    #[test]
    fn range_edit_rejects_out_of_bounds_and_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 101, 0.5, 2, 2));
        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 50, f32::NAN, 2, 2));
        fake().lock().unwrap().state.locked = true;
        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 50, 0.5, 2, 2));
        assert!(fake().lock().unwrap().applied.is_empty());
    }

    #[test]
    fn clearing_range_edits_records_the_previous_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });
        fake().lock().unwrap().range_edits = vec![NativeRangeEdit {
            start: 10,
            end: 50,
            gain: 0.5,
            fade_in: 2,
            fade_out: 2,
        }];

        assert!(clear_range_edits_with(&ops, host, 1, 2));

        {
            let guard = fake().lock().unwrap();
            assert_eq!(guard.undo_name, "Clear Region Range Edits");
            assert_eq!(guard.undo[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
            assert_eq!(guard.redo[0].blob_len, 0);
        }
        fake().lock().unwrap().range_edits.clear();
        assert!(!clear_range_edits_with(&ops, host, 1, 2));
    }

    #[test]
    fn comp_range_batch_is_admitted_and_stored_verbatim() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 400,
            ..Default::default()
        });
        let packed = [0u64, 100, 10, 10, 100, 250, 0, 25, 250, 400, 5, 5];

        assert!(set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            packed.as_ptr(),
            packed.len(),
            true
        ));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_COMP_RANGES);
        assert!(guard.applied[0].managed);
        assert_eq!(guard.applied[0].blob_len, std::mem::size_of_val(&packed));
        let stored = unsafe {
            slice::from_raw_parts(guard.applied[0].blob.cast::<u64>(), packed.len())
        };
        assert_eq!(stored, packed);
    }

    #[test]
    fn comp_range_batch_rejects_malformed_or_locked_batches() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        // A trailing word that is not a full range.
        let ragged = [0u64, 50, 0, 0, 1];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            ragged.as_ptr(),
            ragged.len(),
            true
        ));
        // Ordered but past the region length.
        let beyond = [0u64, 101, 0, 0];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            beyond.as_ptr(),
            beyond.len(),
            true
        ));
        // A null pointer with a non-zero count must not be read.
        assert!(!set_comp_ranges_with(&ops, host, 1, 2, ptr::null(), 4, true));
        assert!(fake().lock().unwrap().applied.is_empty());

        // Locking only refuses turning the managed flag on.
        fake().lock().unwrap().state.locked = true;
        let valid = [0u64, 50, 0, 0];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len(),
            true
        ));
        assert!(set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len(),
            false
        ));
        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(!guard.applied[0].managed);
    }

    #[test]
    fn replace_audio_decodes_and_records_the_snapshot_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        crate::audio_import_cache::set_import_ops(Some(fake_import_ops()));
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            warp_ratio: 1.0,
            ..Default::default()
        });
        RELEASED_AUDIO.store(0, std::sync::atomic::Ordering::SeqCst);
        FAIL_DECODE.store(false, std::sync::atomic::Ordering::SeqCst);
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);

        let path = b"/tmp/replacement.wav";
        assert!(replace_audio_with(&ops, host, 1, 2, path));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_REPLACE_AUDIO);
        let blob = &guard.applied_blobs[0];
        assert_eq!(blob.len(), REPLACE_AUDIO_HEADER_BYTES + path.len());
        let raw = usize::from_ne_bytes(blob[0..8].try_into().unwrap());
        assert_eq!(raw, &FAKE_AUDIO as *const u8 as usize);
        assert_eq!(&blob[REPLACE_AUDIO_HEADER_BYTES..], path);
        // The decoded length and the rate conversion resolve the geometry.
        let state = guard.applied[0].state;
        assert_eq!(state.source_sample_rate, 44_100.0);
        assert_eq!(state.timeline_sample_rate, 48_000.0);
        assert_eq!(state.source_length, 48_000);
        assert_eq!(state.base_length, 48_000);
        assert_eq!(state.source_offset, 0);
        assert_eq!(state.base_source_offset, 0);
        assert_eq!(state.base_start, state.start);
        assert_eq!(state.len, 52_245);
        assert_eq!(guard.undo_name, "Replace Region Audio");
        assert!(!guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        drop(guard);
        // The handle moved into the payload, so it is not released here.
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 0);
        crate::audio_import_cache::set_import_ops(None);
    }

    #[test]
    fn replace_audio_rejects_bad_paths_files_regions_and_rates() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        crate::audio_import_cache::set_import_ops(Some(fake_import_ops()));
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });
        RELEASED_AUDIO.store(0, std::sync::atomic::Ordering::SeqCst);
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);

        assert!(!replace_audio_with(&ops, host, 1, 2, b""));
        FAIL_DECODE.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/missing.wav"));
        FAIL_DECODE.store(false, std::sync::atomic::Ordering::SeqCst);

        // The file decodes but the region is gone: the handle is released.
        fake().lock().unwrap().present = false;
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/x.wav"));
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A timeline rate the native setter refused before the swap.
        fake().lock().unwrap().present = true;
        FAKE_RATE_BITS.store(0f64.to_bits(), std::sync::atomic::Ordering::SeqCst);
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/x.wav"));
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(fake().lock().unwrap().applied.is_empty());
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);
        crate::audio_import_cache::set_import_ops(None);
    }

    fn splittable_state() -> HirariRegionState {
        HirariRegionState {
            len: 1000,
            source_length: 1000,
            loop_count: 1,
            warp_ratio: 1.0,
            source_sample_rate: 48_000.0,
            timeline_sample_rate: 48_000.0,
            ..Default::default()
        }
    }

    #[test]
    fn split_rejects_positions_outside_the_region() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            ..splittable_state()
        });
        for beat in [f64::NAN, f64::INFINITY, -1.0, 50.5, 100.0, 1100.0, 1200.0] {
            assert_eq!(
                split_region_with(&ops, host, 1, 2, beat),
                0,
                "beat {beat} must be rejected"
            );
        }
        let guard = fake().lock().unwrap();
        assert!(guard.applied.is_empty());
        assert!(guard.cloned_states.is_empty());
        assert!(guard.undo.is_empty());
    }

    #[test]
    fn split_applies_both_sides_and_records_the_restore_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            ..splittable_state()
        });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);
        {
            let mut guard = fake().lock().unwrap();
            guard.comp_managed = true;
            guard.comp_ranges = vec![NativeCompRange {
                start: 0,
                end: 500,
                fade_in: 0,
                fade_out: 0,
            }];
            guard.warp_markers = vec![HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            }];
        }

        let right = split_region_with(&ops, host, 1, 2, 600.0);
        assert_eq!(right, 3);

        let guard = fake().lock().unwrap();
        // The clone starts from the original payload; the right-hand state
        // arrives with the first payload op.
        assert_eq!(guard.cloned_states, vec![HirariRegionState::default()]);
        let ops_applied: Vec<i32> = guard.applied.iter().map(|payload| payload.op).collect();
        assert_eq!(
            ops_applied,
            vec![
                HIRARI_REGION_EDIT_STATE,
                HIRARI_REGION_EDIT_MARKERS,
                HIRARI_REGION_EDIT_RANGE_EDITS,
                HIRARI_REGION_EDIT_COMP_RANGES,
                HIRARI_REGION_EDIT_NOTE_STORE,
                HIRARI_REGION_EDIT_STATE,
                HIRARI_REGION_EDIT_MARKERS,
                HIRARI_REGION_EDIT_RANGE_EDITS,
                HIRARI_REGION_EDIT_COMP_RANGES,
                HIRARI_REGION_EDIT_NOTE_STORE,
            ]
        );
        assert_eq!(guard.applied[0].state.start, 100);
        assert_eq!(guard.applied[0].state.len, 500);
        assert_eq!(guard.applied[5].state.start, 600);
        assert_eq!(guard.applied[5].state.len, 500);
        // Both sides keep the region's managed flag.
        assert!(guard.applied[3].managed);
        assert!(guard.applied[8].managed);
        assert_eq!(guard.undo_name, "Split Region");
        assert!(guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo.len(), 2);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.undo[1].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo[1].region_id, 3);
        assert_eq!(guard.redo.len(), 2);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[1].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[1].region_id, 3);
        drop(guard);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn duplicate_copies_the_policy_state_and_records_remove_restore() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            base_start: 100,
            base_length: 1000,
            source_offset: 100,
            ..splittable_state()
        });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);

        let id = duplicate_region_with(&ops, host, 1, 2, 5000);
        assert_eq!(id, 3);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.cloned_states.len(), 1);
        assert_eq!(guard.cloned_states[0].start, 5000);
        // The untrimmed base backs up by the ceil of the trimmed span.
        assert_eq!(guard.cloned_states[0].base_start, 4900);
        assert!(guard.applied.is_empty());
        assert_eq!(guard.undo_name, "Duplicate Region");
        assert!(guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo[0].region_id, 3);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        drop(guard);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn duplicate_rejects_an_empty_or_loopless_region() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        for state in [
            HirariRegionState {
                len: 0,
                ..splittable_state()
            },
            HirariRegionState {
                loop_count: 0,
                ..splittable_state()
            },
        ] {
            reset(state);
            assert_eq!(duplicate_region_with(&ops, host, 1, 2, 5000), 0);
            let guard = fake().lock().unwrap();
            assert!(guard.cloned_states.is_empty());
            assert!(guard.undo.is_empty());
            drop(guard);
        }
    }

    #[test]
    fn owned_history_copies_marker_payloads_before_replay() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset(HirariRegionState::default());
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = fake_ops();
        ops.record_undo = None;
        ops.undo_state = Some(fake_undo_state);
        let host = 1usize as *mut c_void;

        let mut undo_markers = vec![HirariWarpMarker {
            source_sample: 10,
            timeline_sample: 20,
            transient: 1,
        }];
        let redo_markers = vec![HirariWarpMarker {
            source_sample: 30,
            timeline_sample: 40,
            transient: 0,
        }];
        let payload = |markers: &[HirariWarpMarker]| HirariRegionEditPayload {
            track_id: 1,
            region_id: 2,
            op: HIRARI_REGION_EDIT_MARKERS,
            blob: markers.as_ptr().cast(),
            blob_len: std::mem::size_of_val(markers),
            ..Default::default()
        };
        assert!(record_with(
            &ops,
            host,
            b"Markers\0",
            &[payload(&undo_markers)],
            &[payload(&redo_markers)],
            false,
            false,
        ));
        // The caller's buffer is released after the edit returns.
        undo_markers[0].source_sample = 999;
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_MARKERS);
        let replayed =
            unsafe { ptr::read_unaligned(guard.applied_blobs[0].as_ptr().cast::<HirariWarpMarker>()) };
        assert_eq!(replayed.source_sample, 10);
        assert_eq!(replayed.timeline_sample, 20);
        drop(guard);
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn evicted_history_entries_release_region_snapshots() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset(HirariRegionState::default());
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(1, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = fake_ops();
        ops.record_undo = None;
        ops.undo_state = Some(fake_undo_state);
        let host = 1usize as *mut c_void;
        let handle = unsafe { fake_snapshot(host, 1, 2) };

        let restore = restore_payload(1, 2, handle);
        let remove = state_payload(1, 2, HIRARI_REGION_EDIT_REMOVE, HirariRegionState::default());
        assert!(record_with(
            &ops,
            host,
            b"Remove Region\0",
            &[restore],
            &[remove],
            false,
            false,
        ));
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
        // A second entry pushes the first past the depth cap, so its snapshot
        // handle must be handed back to the host.
        assert!(record_with(
            &ops,
            host,
            b"Remove Region\0",
            &[restore],
            &[remove],
            false,
            false,
        ));
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 1);
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
