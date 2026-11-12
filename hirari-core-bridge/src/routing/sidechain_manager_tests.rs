//! Tests split out of sidechain_manager.rs.
    use super::*;

    fn link(source: u32, dest: u32, plugin_idx: u32) -> SidechainLinkRust {
        SidechainLinkRust {
            source_track_id: source,
            dest_track_id: dest,
            plugin_idx,
            input_bus: 0,
            enabled: true,
            level: 1.0,
            tap_point: SidechainTapPointRust::PostFX,
        }
    }

    #[test]
    fn audit_rejects_duplicate_routes_and_invalid_levels() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(1, 2, 0)).unwrap();
        graph.active_links.insert(
            2,
            SidechainLinkRust {
                level: f32::NAN,
                ..link(3, 4, 0)
            },
        );
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn audit_rejects_imported_cycles() {
        let mut graph = SidechainOrchestrator::new();
        graph.active_links.insert(1, link(1, 2, 0));
        graph.active_links.insert(2, link(2, 1, 0));
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn dynamic_plugin_ports_are_enumerated_and_removed() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(20, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        assert_eq!(
            graph
                .links_for_destination(9, 1)
                .iter()
                .map(|entry| entry.0)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(graph.remove_plugin_links(9, 1), 2);
        assert!(graph.links_for_destination(9, 1).is_empty());
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn supports_multiple_sources_and_multiple_plugin_inputs() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(30, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        let mut second_input = link(5, 9, 1);
        second_input.input_bus = 1;
        graph.register_link(20, second_input).unwrap();
        assert_eq!(
            graph.sources_for_input(9, 1, 0),
            vec![(10, 4, 1.0), (30, 3, 1.0)]
        );
        assert_eq!(graph.sources_for_input(9, 1, 1), vec![(20, 5, 1.0)]);
        assert_eq!(graph.resolve_source_for(9, 1), Some(4));
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn rejects_an_exact_duplicate_route_but_allows_shared_destination() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(3, 9, 1)).unwrap();
        assert!(graph.register_link(2, link(4, 9, 1)).is_ok());
        assert_eq!(
            graph.register_link(3, link(3, 9, 1)),
            Err(SidechainLinkError::DuplicateRoute)
        );
    }

    // Ported from tests/sidechain_snapshot_contract.cpp: this behaviour is
    // owned by the Rust engine handle, so the contract is asserted here.
    fn register(
        engine: *const c_void,
        destination: u32,
        plugin: u32,
        left: *const f32,
        right: *const f32,
        frames: u32,
        sample_rate: u32,
        generation: u64,
        level: f32,
    ) -> bool {
        unsafe {
            hirari_sidechain_engine_register_link(
                engine,
                destination,
                plugin,
                3,
                left,
                right,
                frames,
                sample_rate,
                generation,
                level,
                1,
            )
        }
    }

    fn copy_block(
        engine: *const c_void,
        destination: u32,
        plugin: u32,
        left: *mut f32,
        right: *mut f32,
        capacity: u32,
    ) -> (u32, [u64; 6]) {
        let mut metadata = [0u64; 6];
        let frames = unsafe {
            hirari_sidechain_engine_copy_link(
                engine,
                destination,
                plugin,
                left,
                right,
                capacity,
                metadata.as_mut_ptr(),
            )
        };
        (frames, metadata)
    }

    #[test]
    fn engine_handle_matches_the_native_snapshot_contract() {
        let engine = hirari_sidechain_engine_create();
        assert!(!false);
        let left_a = [1.0f32, 2.0, 3.0, 4.0];
        let right_a = [-1.0f32, -2.0, -3.0, -4.0];
        let left_b = [5.0f32, 6.0, 7.0, 8.0];
        let right_b = [-5.0f32, -6.0, -7.0, -8.0];

        assert!(!register(engine, 10, 0, std::ptr::null(), std::ptr::null(), 0, 0, 0, 1.0));
        assert!(!unsafe { hirari_sidechain_engine_resolve_links(engine) });
        assert!(unsafe { hirari_sidechain_engine_take_legacy_misuse(engine) });
        assert!(!unsafe { hirari_sidechain_engine_take_legacy_misuse(engine) });
        assert!(!register(engine, 10, 0, left_a.as_ptr(), right_a.as_ptr(), 4, 48000, 0, 0.5));
        assert!(!register(engine, 10, 0, left_a.as_ptr(), right_a.as_ptr(), 4, 0, 1, 0.5));
        assert!(register(engine, 10, 0, left_a.as_ptr(), right_a.as_ptr(), 4, 48000, 1, 0.5));

        let mut snapshot_left = [0.0f32; 4];
        let mut snapshot_right = [0.0f32; 4];
        let (frames, metadata) = copy_block(
            engine,
            10,
            0,
            snapshot_left.as_mut_ptr(),
            snapshot_right.as_mut_ptr(),
            4,
        );
        assert_eq!(frames, 4);
        assert_eq!(snapshot_left[0], left_a[0]);
        assert_eq!(snapshot_right[3], right_a[3]);
        assert_eq!(metadata[0], 3);
        assert_eq!(f32::from_bits(metadata[1] as u32), 0.5);
        assert_eq!(metadata[2], 1);
        assert_eq!(metadata[3], 48000);
        let first_generation = metadata[5];

        // Refresh publishes both channels under one versioned slot update.
        assert!(unsafe {
            hirari_sidechain_engine_refresh_source(
                engine,
                3,
                left_b.as_ptr(),
                right_b.as_ptr(),
                4,
                48000,
                1,
            )
        });
        let (frames, metadata) = copy_block(
            engine,
            10,
            0,
            snapshot_left.as_mut_ptr(),
            snapshot_right.as_mut_ptr(),
            4,
        );
        assert_eq!(frames, 4);
        assert_eq!(snapshot_left[0], left_b[0]);
        assert_eq!(snapshot_right[3], right_b[3]);
        assert!(metadata[5] > first_generation);

        assert!(register(engine, 11, 1, left_a.as_ptr(), right_a.as_ptr(), 4, 48000, 1, -4.0));
        let mut clamped_left = [0.0f32; 4];
        let mut clamped_right = [0.0f32; 4];
        let (frames, metadata) = copy_block(
            engine,
            11,
            1,
            clamped_left.as_mut_ptr(),
            clamped_right.as_mut_ptr(),
            4,
        );
        assert_eq!(frames, 4);
        assert_eq!(f32::from_bits(metadata[1] as u32), 0.0);
        assert!(!register(
            engine,
            12,
            1,
            left_a.as_ptr(),
            right_a.as_ptr(),
            4,
            48000,
            1,
            f32::NAN
        ));

        assert!(unsafe { hirari_sidechain_engine_has_link(engine, 10, 0, 3) });
        assert!(unsafe { hirari_sidechain_engine_remove_link(engine, 10, 0) });
        assert_eq!(
            copy_block(
                engine,
                10,
                0,
                snapshot_left.as_mut_ptr(),
                snapshot_right.as_mut_ptr(),
                4
            )
            .0,
            0
        );
        assert!(!unsafe { hirari_sidechain_engine_has_link(engine, 10, 0, 3) });

        // Capture/restore hands the snapshot back as an opaque handle and
        // re-registers every captured link.
        let captured = unsafe { hirari_sidechain_engine_capture_links(engine, 3) };
        assert!(!captured.is_null());
        assert!(unsafe { hirari_sidechain_engine_remove_link(engine, 11, 1) });
        assert!(!unsafe { hirari_sidechain_engine_has_link(engine, 11, 1, 3) });
        assert!(unsafe { hirari_sidechain_engine_restore_links(engine, captured) });
        assert!(unsafe { hirari_sidechain_engine_has_link(engine, 11, 1, 3) });
        unsafe { hirari_sidechain_engine_free_captured_links(captured) };

        // Owned publication copies into fixed storage, so the published
        // pointers do not depend on the caller's temporary arrays.
        let owned = hirari_sidechain_engine_create();
        assert!(!owned.is_null());
        let owned_left = [9.0f32, 8.0, 7.0, 6.0];
        let owned_right = [-9.0f32, -8.0, -7.0, -6.0];
        assert!(register(
            owned,
            20,
            0,
            owned_left.as_ptr(),
            owned_right.as_ptr(),
            4,
            48000,
            1,
            1.0
        ));
        let mut owned_copy_left = [0.0f32; 4];
        let mut owned_copy_right = [0.0f32; 4];
        assert_eq!(
            copy_block(
                owned,
                20,
                0,
                owned_copy_left.as_mut_ptr(),
                owned_copy_right.as_mut_ptr(),
                4
            )
            .0,
            4
        );
        assert_eq!(owned_copy_left[0], 9.0);
        assert_eq!(owned_copy_right[3], -6.0);

        let next_left = [1.0f32; 4];
        let next_right = [-1.0f32; 4];
        assert!(unsafe {
            hirari_sidechain_engine_refresh_source(
                owned,
                3,
                next_left.as_ptr(),
                next_right.as_ptr(),
                4,
                48000,
                1,
            )
        });
        let (_, _) = copy_block(
            owned,
            20,
            0,
            owned_copy_left.as_mut_ptr(),
            owned_copy_right.as_mut_ptr(),
            4,
        );
        assert_eq!(owned_copy_left[0], 1.0);
        assert_eq!(owned_copy_right[0], -1.0);

        // Seqlock publication is single-writer by contract; the control
        // mutex makes that hold when two control clients race.
        let writer_a_left = [10.0f32; 8];
        let writer_a_right = [-10.0f32; 8];
        let writer_b_left = [20.0f32; 16];
        let writer_b_right = [-20.0f32; 16];
        let address = owned as usize;
        let coherent = std::sync::Arc::new(AtomicBool::new(true));
        let reader_flag = coherent.clone();
        let reader = std::thread::spawn(move || {
            let engine = address as *const c_void;
            let mut left = [0.0f32; 16];
            let mut right = [0.0f32; 16];
            for _ in 0..2000 {
                let (frames, _) = copy_block(
                    engine,
                    30,
                    0,
                    left.as_mut_ptr(),
                    right.as_mut_ptr(),
                    16,
                );
                if frames == 0 {
                    continue;
                }
                let expected = if frames == 8 { 10.0 } else { 20.0 };
                for frame in 0..frames as usize {
                    if left[frame] != expected || right[frame] != -expected {
                        reader_flag.store(false, Ordering::Release);
                        return;
                    }
                }
            }
        });
        let writer_a = std::thread::spawn(move || {
            let engine = address as *const c_void;
            for _ in 0..100 {
                assert!(register(
                    engine,
                    30,
                    0,
                    writer_a_left.as_ptr(),
                    writer_a_right.as_ptr(),
                    8,
                    48000,
                    1,
                    1.0
                ));
            }
        });
        let writer_b = std::thread::spawn(move || {
            let engine = address as *const c_void;
            for _ in 0..100 {
                assert!(register(
                    engine,
                    30,
                    0,
                    writer_b_left.as_ptr(),
                    writer_b_right.as_ptr(),
                    16,
                    48000,
                    1,
                    0.75
                ));
            }
        });
        writer_a.join().unwrap();
        writer_b.join().unwrap();
        reader.join().unwrap();
        assert!(coherent.load(Ordering::Acquire));
        let mut raced_left = [0.0f32; 16];
        let mut raced_right = [0.0f32; 16];
        let (frames, metadata) = copy_block(
            owned,
            30,
            0,
            raced_left.as_mut_ptr(),
            raced_right.as_mut_ptr(),
            16,
        );
        assert!(frames == 8 || frames == 16);
        assert!(metadata[5] > 0);

        unsafe { hirari_sidechain_engine_destroy(owned) };
        unsafe { hirari_sidechain_engine_destroy(engine) };
    }
