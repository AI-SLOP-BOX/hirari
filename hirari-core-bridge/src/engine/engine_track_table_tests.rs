//! Tests split out of engine_track_table.rs.

    use super::*;
    use crate::engine_track::{
        hirari_track_delay_samples, hirari_track_id, hirari_track_new, hirari_track_owner_object,
        hirari_track_owner_release, hirari_track_pdc_requested, hirari_track_resolve_bus,
        hirari_track_set_delay_samples, TRACK_TYPE_AUDIO,
    };
    use std::sync::Weak;

    /// Builds a track and hands the reference it owns to the graph, together
    /// with the object pointer every entry point addresses it by and a
    /// non-owning witness for the track's lifetime.
    struct Made {
        owner: *mut c_void,
        object: *mut c_void,
        witness: Weak<Track>,
    }

    fn build(id: u32, kind: u32) -> Made {
        let name = b"t";
        let mut object = std::ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                id,
                name.as_ptr(),
                name.len(),
                kind,
                std::ptr::null_mut(),
                &mut object,
            )
        };
        Made {
            witness: unsafe { Arc::downgrade(&*owner.cast::<Arc<Track>>()) },
            owner,
            object,
        }
    }

    struct Table(*mut c_void);

    impl Table {
        fn new() -> Self {
            Table(hirari_engine_track_table_create())
        }
        fn get(&self) -> *mut c_void {
            self.0
        }
        /// Appends one track and returns it, so a test keeps only the pointers
        /// that stay valid while the graph holds the reference.
        fn add(&self, id: u32, kind: u32) -> (*mut c_void, Weak<Track>) {
            let made = self.commit(id, kind);
            (made.object, made.witness)
        }
        fn commit(&self, id: u32, kind: u32) -> Made {
            let made = build(id, kind);
            assert!(unsafe { hirari_engine_track_insert(self.get(), usize::MAX, made.owner) });
            made
        }
        fn publish(&self, generation: u64, stamp: bool) {
            unsafe { hirari_engine_track_publish(self.get(), generation, stamp) };
        }
        fn locked(&self) -> Vec<*mut c_void> {
            let count = unsafe { hirari_engine_track_count(self.get()) };
            unsafe { std::slice::from_raw_parts(hirari_engine_track_ptrs(self.get()), count) }
                .to_vec()
        }
        fn snapshot(&self) -> HirariTrackSnapshot {
            let mut out = HirariTrackSnapshot::default();
            unsafe { hirari_engine_track_snapshot(self.get(), &mut out) };
            assert!(!out.active_tracks.is_null() && !out.track_map.is_null());
            out
        }
    }

    impl Drop for Table {
        fn drop(&mut self) {
            unsafe { hirari_engine_track_table_destroy(self.get()) };
        }
    }

    fn ids(tracks: &[*mut c_void]) -> Vec<u32> {
        tracks
            .iter()
            .map(|&track| unsafe { hirari_track_id(track) })
            .collect()
    }

    fn published_ids(snapshot: &HirariTrackSnapshot) -> Vec<u32> {
        ids(&unsafe {
            std::slice::from_raw_parts(snapshot.active_tracks, snapshot.track_count as usize)
        })
    }

    fn mapped(snapshot: &HirariTrackSnapshot, id: u32) -> bool {
        !unsafe { *snapshot.track_map.add(id as usize) }.is_null()
    }

    fn retained(witness: &Weak<Track>) -> bool {
        witness.upgrade().is_some()
    }

    #[test]
    fn graph_keeps_push_order_and_resolves_ids() {
        let table = Table::new();
        let (first, _) = table.add(7, TRACK_TYPE_AUDIO);
        let (second, _) = table.add(3, TRACK_TYPE_AUDIO);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 2);
        assert_eq!(table.locked(), vec![first, second]);
        assert_eq!(unsafe { hirari_engine_track_find(table.get(), 3) }, second);
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 1);
        assert!(unsafe { hirari_engine_track_find(table.get(), 9) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(table.get(), 9) },
            usize::MAX
        );
    }

    #[test]
    fn insert_clamps_the_index_and_rejects_a_null_owner() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        table.add(2, TRACK_TYPE_AUDIO);
        assert!(!unsafe {
            hirari_engine_track_insert(table.get(), usize::MAX, std::ptr::null_mut())
        });
        let tail = build(3, TRACK_TYPE_AUDIO);
        assert!(unsafe { hirari_engine_track_insert(table.get(), 99, tail.owner) });
        let head = build(4, TRACK_TYPE_AUDIO);
        assert!(unsafe { hirari_engine_track_insert(table.get(), 0, head.owner) });
        assert_eq!(ids(&table.locked()), vec![4, 1, 2, 3]);
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 3);
    }

    #[test]
    fn detach_returns_a_second_reference_and_defers_the_track() {
        let table = Table::new();
        table.add(11, TRACK_TYPE_AUDIO);
        let kept = table.commit(12, TRACK_TYPE_AUDIO);
        let detached = unsafe { hirari_engine_track_detach(table.get(), 11) };
        assert!(!detached.is_null());
        assert_eq!(unsafe { hirari_track_id(hirari_track_owner_object(detached)) }, 11);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 1);
        // The caller's reference is gone but the deferred list still holds the
        // track, so no audio-thread pointer can dangle.
        unsafe { hirari_track_owner_release(detached) };
        assert!(unsafe { hirari_engine_track_detach(table.get(), 99) }.is_null());
        // Dropping the table releases every track the callback may have seen.
        assert!(retained(&kept.witness));
        drop(table);
        assert!(!retained(&kept.witness));
    }

    #[test]
    fn publish_stamps_the_generation_and_detach_clears_the_id_map() {
        let table = Table::new();
        table.add(21, TRACK_TYPE_AUDIO);
        table.add(22, TRACK_TYPE_AUDIO);
        table.publish(7, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![21, 22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(mapped(&snapshot, 21) && mapped(&snapshot, 22));
        unsafe { hirari_track_owner_release(hirari_engine_track_detach(table.get(), 21)) };
        table.publish(0, false);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(!mapped(&snapshot, 21));
    }

    #[test]
    fn publish_without_a_stamp_keeps_the_previous_generation() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        table.publish(3, true);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        table.add(2, TRACK_TYPE_AUDIO);
        table.publish(99, false);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        assert_eq!(table.snapshot().track_count, 2);
        table.publish(4, true);
        assert_eq!(table.snapshot().audio_config_generation, 4);
    }

    #[test]
    fn publication_writes_the_buffer_the_callback_is_not_reading() {
        let table = Table::new();
        table.add(31, TRACK_TYPE_AUDIO);
        table.add(32, TRACK_TYPE_AUDIO);
        table.publish(1, true);
        let held = table.snapshot();
        assert_eq!(published_ids(&held), vec![31, 32]);
        unsafe {
            hirari_track_owner_release(hirari_engine_track_detach(table.get(), 31));
            hirari_track_owner_release(hirari_engine_track_detach(table.get(), 32));
        }
        table.publish(2, true);
        assert_eq!(published_ids(&table.snapshot()), Vec::<u32>::new());
        // The realtime callback may still be traversing the buffer it read.
        assert_eq!(published_ids(&held), vec![31, 32]);
    }

    #[test]
    fn publication_clamps_to_the_realtime_capacity() {
        let table = Table::new();
        for id in 1..=(MAX_ACTIVE_TRACKS as u32 + 2) {
            table.add(id, TRACK_TYPE_AUDIO);
        }
        table.publish(1, true);
        let snapshot = table.snapshot();
        assert_eq!(snapshot.track_count as usize, MAX_ACTIVE_TRACKS);
        assert_eq!(published_ids(&snapshot).len(), MAX_ACTIVE_TRACKS);
        assert!(mapped(&snapshot, MAX_ACTIVE_TRACKS as u32));
        assert!(!mapped(&snapshot, MAX_ACTIVE_TRACKS as u32 + 1));
        assert!(!mapped(&snapshot, MAX_ACTIVE_TRACKS as u32 + 2));
    }

    #[test]
    fn ids_beyond_the_map_capacity_are_published_but_not_mapped() {
        let table = Table::new();
        let id = TRACK_MAP_LEN as u32 + 5;
        table.add(id, TRACK_TYPE_AUDIO);
        table.add(9, TRACK_TYPE_AUDIO);
        table.publish(1, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![id, 9]);
        assert!(mapped(&snapshot, 9));
    }

    #[test]
    fn retain_answers_a_second_reference_for_the_graph() {
        let table = Table::new();
        table.add(41, TRACK_TYPE_AUDIO);
        let kept = table.commit(42, TRACK_TYPE_AUDIO);
        let one = unsafe { hirari_engine_track_retain(table.get(), 42) };
        assert_eq!(unsafe { hirari_track_owner_object(one) }, kept.object);
        unsafe { hirari_track_owner_release(one) };
        assert!(unsafe { hirari_engine_track_retain(table.get(), 99) }.is_null());
        assert!(retained(&kept.witness));
    }

    #[test]
    fn defer_all_holds_the_graph_until_the_table_dies() {
        let table = Table::new();
        let first = table.commit(51, TRACK_TYPE_AUDIO);
        let second = table.commit(52, TRACK_TYPE_AUDIO);
        unsafe { hirari_engine_track_defer_all(table.get()) };
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert!(table.locked().is_empty());
        assert!(retained(&first.witness) && retained(&second.witness));
        drop(table);
        assert!(!retained(&first.witness) && !retained(&second.witness));
    }

    #[test]
    fn reset_project_releases_the_graph_and_resets_the_managers() {
        let table = Table::new();
        let bus = table.commit(2, TRACK_TYPE_BUS);
        unsafe { hirari_track_resolve_bus(bus.object, std::ptr::null_mut(), true) };
        let buses = crate::bus_audio::hirari_bus_system_create();
        let null = std::ptr::null_mut();
        unsafe { hirari_engine_sync_structure(table.get(), buses, null, null, 1) };
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 2) });
        unsafe {
            hirari_engine_reset_project(table.get(), null, null, null, null, buses, null, null)
        };
        // The outgoing graph is released here rather than deferred to shutdown.
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert!(!retained(&bus.witness));
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 2) });
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn structural_sync_reconciles_buses_publishes_then_compensates() {
        let table = Table::new();
        let audio = table.commit(1, TRACK_TYPE_AUDIO);
        let bus = table.commit(5, TRACK_TYPE_BUS);
        unsafe { hirari_track_resolve_bus(bus.object, std::ptr::null_mut(), true) };
        // The user delay is the part of the delay line a graph rebuild must
        // never erase, so seed it before the solve runs.
        unsafe { hirari_track_set_delay_samples(audio.object, 64) };
        let buses = crate::bus_audio::hirari_bus_system_create();
        let pdc = crate::native_pdc::hirari_native_pdc_create();
        assert!(unsafe { crate::native_pdc::hirari_native_pdc_bind_control_thread(pdc) });
        unsafe { hirari_engine_sync_structure(table.get(), buses, std::ptr::null_mut(), pdc, 9) };
        // The walk publishes the graph the callback traverses.
        assert_eq!(table.snapshot().audio_config_generation, 9);
        // Only the bus actually in the graph is presented to the audio path.
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 5) });
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 1) });
        // A chain without inserts reports no latency, so the solver owes every
        // track a zero offset — and the manual delay still reads back.
        assert_eq!(unsafe { hirari_track_pdc_requested(audio.object) }, 0);
        assert_eq!(unsafe { hirari_track_pdc_requested(bus.object) }, 0);
        assert_eq!(unsafe { hirari_track_delay_samples(audio.object) }, 64);
        unsafe { crate::native_pdc::hirari_native_pdc_destroy(pdc) };
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn null_handles_answer_defaults() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        assert_eq!(unsafe { hirari_engine_track_count(std::ptr::null()) }, 0);
        assert!(unsafe { hirari_engine_track_ptrs(std::ptr::null()) }.is_null());
        assert!(unsafe { hirari_engine_track_find(std::ptr::null(), 1) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(std::ptr::null(), 1) },
            usize::MAX
        );
        assert!(unsafe { hirari_engine_track_retain(std::ptr::null(), 1) }.is_null());
        let first = build(2, TRACK_TYPE_AUDIO);
        let second = build(3, TRACK_TYPE_AUDIO);
        assert!(!unsafe {
            hirari_engine_track_insert(std::ptr::null_mut(), usize::MAX, first.owner)
        });
        assert!(!unsafe { hirari_engine_track_insert(std::ptr::null_mut(), 0, second.owner) });
        for owner in [first.owner, second.owner] {
            unsafe { hirari_track_owner_release(owner) };
        }
        assert!(unsafe { hirari_engine_track_detach(std::ptr::null_mut(), 1) }.is_null());
        unsafe { hirari_engine_track_table_destroy(std::ptr::null_mut()) };
        unsafe {
            hirari_engine_track_defer_all(std::ptr::null_mut());
            hirari_engine_track_publish(std::ptr::null_mut(), 1, true);
            hirari_engine_track_snapshot(std::ptr::null(), std::ptr::null_mut());
            hirari_engine_sync_structure(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
            );
            hirari_engine_reset_project(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        };
        let buffers = [table.locked()[0], std::ptr::null_mut()];
        let mut snapshot = HirariTrackSnapshot {
            active_tracks: buffers.as_ptr(),
            track_map: unsafe { buffers.as_ptr().add(1) },
            track_count: 7,
            audio_config_generation: 7,
        };
        unsafe { hirari_engine_track_snapshot(std::ptr::null(), &mut snapshot) };
        assert_eq!(snapshot, HirariTrackSnapshot::default());
        // The live graph is untouched by every null-handle call above.
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 1);
    }

    #[test]
    fn dropping_the_table_releases_every_handle() {
        let table = Table::new();
        let first = table.commit(71, TRACK_TYPE_AUDIO);
        let second = table.commit(72, TRACK_TYPE_AUDIO);
        let detached = unsafe { hirari_engine_track_detach(table.get(), 72) };
        unsafe { hirari_track_owner_release(detached) };
        table.publish(1, true);
        assert!(retained(&first.witness) && retained(&second.witness));
        drop(table);
        assert!(!retained(&first.witness) && !retained(&second.witness));
    }

    #[test]
    fn a_retained_handle_keeps_the_object_readable_after_the_graph_goes() {
        let table = Table::new();
        table.add(61, TRACK_TYPE_AUDIO);
        let retained = unsafe { hirari_engine_track_retain(table.get(), 61) };
        let object = unsafe { hirari_track_owner_object(retained) };
        drop(table);
        // A control walk that took the track out of the graph still reads a
        // consistent identity from it, because its own reference holds.
        assert_eq!(unsafe { hirari_track_id(object) }, 61);
        unsafe { hirari_track_owner_release(retained) };
    }
