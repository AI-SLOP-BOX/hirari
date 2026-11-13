
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
        let state = { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
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
        let state = { crate::undo_transaction::hirari_undo_manager_create(1, 300) };
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
