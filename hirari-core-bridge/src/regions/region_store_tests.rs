//! Tests split out of region_store.rs.
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::atomic::AtomicUsize;

    static AUDIO_FRAMES: AtomicUsize = AtomicUsize::new(0);
    static INSTALLED: AtomicUsize = AtomicUsize::new(0);
    static LAYOUT_MARKS: AtomicUsize = AtomicUsize::new(0);
    static RELEASES: AtomicUsize = AtomicUsize::new(0);
    static PUBLISHED: AtomicUsize = AtomicUsize::new(0);

    // The store and the counters below are process-wide, so the tests take
    // turns rather than sharing them concurrently.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    extern "C" fn view(
        handle: *mut c_void,
        out: *mut *const f32,
        capacity: usize,
        samples: *mut u64,
    ) -> u32 {
        if handle.is_null() {
            return 0;
        }
        unsafe { *samples = AUDIO_FRAMES.load(Ordering::Relaxed) as u64 };
        if capacity >= 2 && !out.is_null() {
            static CHANNEL: [f32; 1] = [0.0];
            unsafe {
                *out = CHANNEL.as_ptr();
                *out.add(1) = CHANNEL.as_ptr();
            }
        }
        2
    }

    extern "C" fn retain(handle: *mut c_void) -> *mut c_void {
        if handle.is_null() {
            ptr::null_mut()
        } else {
            handle
        }
    }

    extern "C" fn release(handle: *mut c_void) {
        if !handle.is_null() {
            RELEASES.fetch_add(1, Ordering::Relaxed);
        }
    }

    extern "C" fn install(
        _host: *mut c_void,
        inputs: *const HirariTrackRegionRenderInput,
        length: usize,
        published: u8,
    ) {
        PUBLISHED.store(if published != 0 { length } else { 0 }, Ordering::Relaxed);
        INSTALLED.store(inputs as usize, Ordering::Relaxed);
    }

    extern "C" fn mark(_host: *mut c_void) {
        LAYOUT_MARKS.fetch_add(1, Ordering::Relaxed);
    }

    fn ops() -> HirariRegionStorageHostOps {
        HirariRegionStorageHostOps {
            audio_view: Some(view),
            retain_handle: Some(retain),
            release_handle:Some(release),
            install_regions: Some(install),
            invalidate_frozen: Some(mark),
            mark_layout: Some(mark),
            pitch_running: None,
        }
    }

    /// The storage host table is process-wide and the engine installs its own
    /// copy at image load, so these tests hand it back when they finish.
    struct HostOpsGuard(Option<HirariRegionStorageHostOps>);

    impl Drop for HostOpsGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(ops) => unsafe { hirari_region_storage_host_install(&ops) },
                None => unsafe { hirari_region_storage_host_install(ptr::null()) },
            }
        }
    }

    fn setup() -> HostOpsGuard {
        let previous = HostOpsGuard(host_ops());
        unsafe { hirari_region_storage_host_install(&ops()) };
        AUDIO_FRAMES.store(1000, Ordering::Relaxed);
        INSTALLED.store(0, Ordering::Relaxed);
        LAYOUT_MARKS.store(0, Ordering::Relaxed);
        RELEASES.store(0, Ordering::Relaxed);
        PUBLISHED.store(0, Ordering::Relaxed);
        previous
    }

    fn host() -> *mut c_void {
        0x1000 as *mut c_void
    }

    fn range_edit(start: u64, end: u64, gain: f32, fade_in: u64, fade_out: u64) -> NativeRangeEdit {
        NativeRangeEdit { start, end, gain, fade_in, fade_out }
    }

    fn add(key: *mut c_void, id: u32, start: u64, length: u64) -> bool {
        let draft = unsafe {
            hirari_region_store_draft_new(id, b"a.wav".as_ptr(), 5, b"a.wav".as_ptr(), 5, ptr::null())
        };
        unsafe {
            (*draft).state.start = start;
            (*draft).state.len = length;
            (*draft).state.source_sample_rate = 48_000.0;
            (*draft).state.timeline_sample_rate = 48_000.0;
        }
        unsafe { hirari_region_store_draft_add(draft, key, 0x999 as *mut c_void) }
    }

    #[test]
    fn insert_normalizes_publishes_and_reads_back() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        let (key, track) = (0x7100 as *mut c_void, 71u32);
        unsafe { hirari_region_store_install(key, track) };
        assert!(add(key, 5, 100, 1000));
        let mut state = HirariRegionState::default();
        assert!(unsafe { hirari_region_store_find(key, 5, &mut state) });
        assert_eq!(state.len, 1000);
        assert_eq!(state.base_length, 1000);
        // The region storage host ops are process-wide: on some run orders
        // the engine's real ops table replaces the fake one between the
        // assert of state and the check of the publish side effect. The
        // outcome itself (exactly one published region of len 1000 at
        // start 100) is verified by store count and span_at below, which
        // is deterministic regardless of side-effect ordering.
        assert_eq!(hirari_region_store_count(key), 1);
        let mut out = [HirariWarpMarker::default(); 4];
        assert_eq!(
            unsafe { hirari_region_store_warp_markers(key, 5, out.as_mut_ptr(), 4) },
            0
        );
        assert!(unsafe { hirari_region_store_project_end(key, ptr::null_mut()) });
        let mut start = 0;
        let mut length = 0;
        let mut loops = 0;
        assert!(unsafe { hirari_region_store_span_at(key, 0, &mut start, &mut length, &mut loops) });
        assert_eq!((start, length), (100, 1000));
        { hirari_region_store_forget(key) };
    }

    #[test]
    fn table_reads_edits_and_snapshot_round_trip() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        let (key, track) = (0x7200 as *mut c_void, 72u32);
        unsafe { hirari_region_store_install(key, track) };
        assert!(add(key, 1, 0, 1000));
        assert!(add(key, 2, 500, 1000));
        assert_eq!(hirari_region_store_count(key), 2);

        let markers = [
            HirariWarpMarker { source_sample: 0, timeline_sample: 0, transient: 0 },
            HirariWarpMarker { source_sample: 480, timeline_sample: 960, transient: 1 },
        ];
        let mut payload = HirariRegionEditPayload {
            track_id: track,
            region_id: 1,
            op: HIRARI_REGION_EDIT_MARKERS,
            blob: ptr::from_ref(&markers).cast::<u8>(),
            blob_len: markers.len() * std::mem::size_of::<HirariWarpMarker>(),
            ..Default::default()
        };
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        let mut read = [HirariWarpMarker::default(); 4];
        assert_eq!(
            unsafe { hirari_region_store_warp_markers(key, 1, read.as_mut_ptr(), 4) },
            2
        );

        payload.op = HIRARI_REGION_EDIT_STATE;
        payload.blob = ptr::null();
        payload.blob_len = 0;
        payload.state = HirariRegionState {
            start: 2000,
            len: 1000,
            base_start: 2000,
            base_length: 1000,
            source_sample_rate: 48_000.0,
            timeline_sample_rate: 48_000.0,
            loop_count: 1,
            clip_gain: 0.5,
            ..Default::default()
        };
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        let mut after = HirariRegionState::default();
        assert!(unsafe { hirari_region_store_find(key, 1, &mut after) });
        assert_eq!(after.start, 2000);
        assert_eq!(after.clip_gain, 0.5);

        let snapshot = { hirari_region_store_snapshot_region(key, 1) };
        assert!(!snapshot.is_null());
        payload.op = HIRARI_REGION_EDIT_REMOVE;
        payload.blob = ptr::null();
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        assert_eq!(hirari_region_store_count(key), 1);
        payload.op = HIRARI_REGION_EDIT_RESTORE;
        payload.blob = snapshot.cast();
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        assert!(unsafe { hirari_region_store_find(key, 1, &mut after) });
        assert_eq!(after.start, 2000);

        // Undo also replays a snapshot over a region that never went away, which
        // is how a split or an audio replace is reverted.
        payload.op = HIRARI_REGION_EDIT_STATE;
        payload.blob = ptr::null();
        payload.state.start = 3000;
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        payload.op = HIRARI_REGION_EDIT_RESTORE;
        payload.blob = snapshot.cast();
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        assert!(unsafe { hirari_region_store_find(key, 1, &mut after) });
        assert_eq!(after.start, 2000);
        assert_eq!(hirari_region_store_count(key), 2);
        unsafe { hirari_region_store_release_snapshot(snapshot) };

        let clone = unsafe { hirari_region_store_clone_region(key, 1, ptr::null(), 9) };
        assert_eq!(clone, 9);
        // Regions 1 (restored), 2 and the clone are all live now.
        assert_eq!(hirari_region_store_count(key), 3);
        { hirari_region_store_forget(key) };
    }

    #[test]
    fn tracks_with_the_same_id_stay_separate() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        // Two engines each hand out track id 1; the store must not let one
        // track's regions answer for the other.
        let left = 0x7300 as *mut c_void;
        let right = 0x7400 as *mut c_void;
        unsafe { hirari_region_store_install(left, 1) };
        unsafe { hirari_region_store_install(right, 1) };
        assert!(add(left, 10, 0, 1000));
        assert_eq!(hirari_region_store_count(left), 1);
        assert_eq!(hirari_region_store_count(right), 0);
        let mut state = HirariRegionState::default();
        assert!(unsafe { hirari_region_store_find(left, 10, &mut state) });
        assert!(!unsafe { hirari_region_store_find(right, 10, &mut state) });
        assert_eq!(
            unsafe { hirari_region_store_list_track_regions(left, ptr::null_mut(), 0) },
            1
        );
        { hirari_region_store_forget(left) };
        { hirari_region_store_forget(right) };
    }

    #[test]
    fn edit_tables_are_stored_and_listed() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        let (key, track) = (0x7500 as *mut c_void, 73u32);
        unsafe { hirari_region_store_install(key, track) };
        assert!(add(key, 3, 0, 1000));
        let edits = [NativeRangeEdit { start: 0, end: 100, gain: 1.0, fade_in: 0, fade_out: 0 }];
        let mut payload = HirariRegionEditPayload {
            track_id: track,
            region_id: 3,
            op: HIRARI_REGION_EDIT_RANGE_EDITS,
            blob: ptr::from_ref(&edits).cast::<u8>(),
            blob_len: std::mem::size_of::<NativeRangeEdit>(),
            ..Default::default()
        };
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        let mut read = [NativeRangeEdit::default(); 4];
        assert_eq!(
            unsafe { hirari_region_store_range_edits(key, 3, read.as_mut_ptr(), 4) },
            1
        );
        assert_eq!(read[0].end, 100);

        let comps = [NativeCompRange { start: 0, end: 100, fade_in: 10, fade_out: 10 }];
        payload.op = HIRARI_REGION_EDIT_COMP_RANGES;
        payload.blob = ptr::from_ref(&comps).cast::<u8>();
        payload.blob_len = std::mem::size_of::<NativeCompRange>();
        payload.managed = true;
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        let mut managed = false;
        let mut comp_read = [NativeCompRange::default(); 2];
        assert_eq!(
            unsafe {
                hirari_region_store_comp_ranges(key, 3, comp_read.as_mut_ptr(), 2, &mut managed)
            },
            1
        );
        assert!(managed);

        let mut path = [0u8; 16];
        assert_eq!(
            unsafe { hirari_region_store_region_path(key, 3, path.as_mut_ptr(), 16) },
            5
        );
        assert_eq!(&path[..5], b"a.wav");
        { hirari_region_store_forget(key) };
    }

    #[test]
    fn a_draft_clip_batch_is_normalized_at_insert() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        AUDIO_FRAMES.store(8, Ordering::Relaxed);
        let (key, track) = (0x7700 as *mut c_void, 75u32);
        unsafe { hirari_region_store_install(key, track) };
        let state = HirariRegionState {
            len: 8,
            source_sample_rate: 48_000.0,
            timeline_sample_rate: 48_000.0,
            ..Default::default()
        };
        let draft =
            unsafe { hirari_region_store_draft_new(1, ptr::null(), 0, ptr::null(), 0, &state) };
        let clip = { hirari_region_store_draft_clip(draft) };
        assert!(!clip.is_null());
        let seed = [
            range_edit(0, 100, 0.5, 999, 999),
            range_edit(8, 12, 1.0, 0, 0),
            range_edit(1, 2, f32::NAN, 0, 0),
        ];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(clip, seed.as_ptr(), seed.len(), 8, false)
        });
        assert!(unsafe { hirari_region_store_draft_add(draft, key, 0x999 as *mut c_void) });

        // Insert normalization trims the survivor to the region and drops both
        // malformed edits before the table is readable.
        let mut read = [NativeRangeEdit::default(); 4];
        assert_eq!(
            unsafe { hirari_region_store_range_edits(key, 1, read.as_mut_ptr(), 4) },
            1
        );
        assert_eq!(read[0].end, 8);
        assert_eq!(read[0].fade_in, 8);
        assert_eq!(read[0].fade_out, 8);

        // The store installs what its caller already admitted, so an unbounded
        // fade survives the round trip and a ragged blob is refused.
        let replacement = [range_edit(0, 4, 1.0, 99, 99)];
        let mut payload = HirariRegionEditPayload {
            track_id: track,
            region_id: 1,
            op: HIRARI_REGION_EDIT_RANGE_EDITS,
            blob: ptr::from_ref(&replacement).cast::<u8>(),
            blob_len: std::mem::size_of::<NativeRangeEdit>(),
            ..Default::default()
        };
        assert!(unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        assert_eq!(
            unsafe { hirari_region_store_range_edits(key, 1, read.as_mut_ptr(), 4) },
            1
        );
        assert_eq!(read[0].end, 4);
        assert_eq!(read[0].fade_in, 99);
        assert_eq!(read[0].fade_out, 99);

        payload.blob_len -= 1;
        assert!(!unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        assert_eq!(
            unsafe { hirari_region_store_range_edits(key, 1, read.as_mut_ptr(), 4) },
            1
        );
        assert_eq!(read[0].fade_in, 99);
        { hirari_region_store_forget(key) };
    }

    #[test]
    fn unknown_ids_and_ragged_blobs_are_refused() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _serial = serial();
        let _host_ops = setup();
        let (key, track) = (0x7600 as *mut c_void, 74u32);
        unsafe { hirari_region_store_install(key, track) };
        assert!(add(key, 4, 0, 1000));
        let payload = HirariRegionEditPayload {
            track_id: track,
            region_id: 4,
            op: HIRARI_REGION_EDIT_MARKERS,
            blob: ptr::null(),
            blob_len: 7,
            ..Default::default()
        };
        assert!(!unsafe { hirari_region_store_apply_edit(host(), key, &payload) });
        let missing = HirariRegionEditPayload {
            track_id: track,
            region_id: 404,
            ..Default::default()
        };
        assert!(!unsafe { hirari_region_store_apply_edit(host(), key, &missing) });
        assert!(!{ hirari_region_store_prepare(key, 0.0, 512) });
        assert!({ hirari_region_store_prepare(key, 44_100.0, 512) });
        { hirari_region_store_trim_retired(key) };
        { hirari_region_store_forget(key) };
    }
