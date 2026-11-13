// Split out of region_edit_host_tests.rs; textually included into the test module.
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
