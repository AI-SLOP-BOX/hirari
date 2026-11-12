//! Tests split out of routing_edit_host.rs.
use super::*;


    struct UndoRecord {
        name: String,
        undo: Vec<HirariRoutingEditPayload>,
    }

    /// The routing graph and the sidechain route table are the real Rust
    /// stores, so the host only fakes the C++ pieces: the sidechain manager
    /// registration and the undo recorder. Track-state edits are covered by
    /// the engine-backed tests in lib_tests_routing.rs, because the session
    /// table those paths use is process-global and can't be swapped here.
    struct TestHost {
        routing: *mut c_void,
        routes: *mut c_void,
        gate: *mut c_void,
        // (destination, plugin, source, tap)
        links: Vec<(u32, u32, u32, u32)>,
        undo: Vec<UndoRecord>,
        layouts: u32,
        publishes: u32,
        source_found: bool,
        destination_found: bool,
        source_prepared: bool,
    }

    impl TestHost {
        fn new() -> Self {
            TestHost {
                routing: crate::routing_engine_core::hirari_routing_engine_create(),
                routes: crate::sidechain_route_store::hirari_sidechain_routes_create(),
                gate: crate::sidechain_manager::hirari_sidechain_engine_create(),
                links: Vec::new(),
                undo: Vec::new(),
                layouts: 0,
                publishes: 0,
                source_found: false,
                destination_found: false,
                source_prepared: false,
            }
        }
    }

    impl Drop for TestHost {
        fn drop(&mut self) {
            unsafe {
                crate::routing_engine_core::hirari_routing_engine_destroy(self.routing);
                crate::sidechain_route_store::hirari_sidechain_routes_destroy(self.routes);
                crate::sidechain_manager::hirari_sidechain_engine_destroy(self.gate);
            }
        }
    }

    unsafe extern "C" fn test_routing_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.routing
    }

    unsafe extern "C" fn test_sidechain_routes_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.routes
    }

    unsafe extern "C" fn test_sidechain_context(
        host: *mut c_void,
        _source: u32,
        _dest: u32,
        out: *mut HirariRoutingSidechainContext,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        *out = HirariRoutingSidechainContext {
            block_size: 512,
            sample_rate: 48_000,
            generation: 1,
            source_found: host.source_found,
            destination_found: host.destination_found,
            source_prepared: host.source_prepared,
        };
        true
    }

    unsafe extern "C" fn test_register_sidechain(
        host: *mut c_void,
        dest: u32,
        plugin: u32,
        source: u32,
        _context: *const HirariRoutingSidechainContext,
        tap: u32,
        _silent: bool,
    ) -> bool {
        let host = &mut *(host as *mut TestHost);
        host.links
            .retain(|link| !(link.0 == dest && link.1 == plugin));
        host.links.push((dest, plugin, source, tap));
        true
    }

    unsafe extern "C" fn test_sidechain_engine_store(host: *mut c_void) -> *mut c_void {
        unsafe { &*(host as *const TestHost) }.gate
    }

    unsafe extern "C" fn test_mark_layout_changed(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.layouts += 1;
    }

    unsafe extern "C" fn test_sync_structural(host: *mut c_void) {
        let host = &mut *(host as *mut TestHost);
        host.publishes += 1;
    }

    unsafe extern "C" fn test_record_undo(
        host: *mut c_void,
        name: *const c_char,
        undo: *const HirariRoutingEditPayload,
        undo_count: usize,
        redo: *const HirariRoutingEditPayload,
        redo_count: usize,
        perform: bool,
        _coalesce: bool,
    ) {
        let host = &mut *(host as *mut TestHost);
        let name = if name.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(name)
                .to_string_lossy()
                .into_owned()
        };
        let undo = if undo_count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(undo, undo_count).to_vec()
        };
        let redo = if redo_count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(redo, redo_count).to_vec()
        };
        if perform {
            apply_payloads_with(&test_ops(), host as *mut TestHost as *mut c_void, &redo);
        }
        host.undo.push(UndoRecord { name, undo });
    }

    fn test_ops() -> HirariRoutingHostOps {
        HirariRoutingHostOps {
            routing_store: Some(test_routing_store),
            sidechain_routes_store: Some(test_sidechain_routes_store),
            sidechain_engine_store: Some(test_sidechain_engine_store),
            sidechain_context: Some(test_sidechain_context),
            register_sidechain: Some(test_register_sidechain),
            mark_layout_changed: Some(test_mark_layout_changed),
            sync_structural: Some(test_sync_structural),
            record_undo: Some(test_record_undo),
            undo_state: None,
            engine_block_store: None,
        }
    }

    fn host_ptr(host: &TestHost) -> *mut c_void {
        host as *const TestHost as *mut c_void
    }

    /// Reads the live routing graph through the same path the policy uses.
    fn edge(host: &TestHost, source: u32, dest: u32) -> HirariRoutingEdgeState {
        let ops = test_ops();
        query_edge(&ops, host_ptr(host), source, dest).expect("routing store")
    }

    fn route_table(host: &TestHost) -> Vec<HirariRoutingSidechainEntry> {
        let ops = test_ops();
        list_sidechain(&ops, host_ptr(host)).expect("sidechain route store")
    }

    fn seed_connection(host: &TestHost, source: u32, dest: u32, gain: f32, send: bool) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_connection(
                host.routing, source, dest, gain, send, false,
            )
        };
    }

    fn seed_route(host: &TestHost, source: u32, dest: u32, plugin: u32, tap: u32) {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(
                host.routes, source, dest, plugin, tap,
            )
        };
    }

    fn seed_dependency(host: &TestHost, source: u32, dest: u32) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_dependency(
                host.routing, source, dest,
            )
        };
    }

    fn has_dependency(host: &TestHost, source: u32, dest: u32) -> bool {
        unsafe { crate::routing_engine_core::hirari_routing_engine_has_dependency(host.routing, source, dest) }
    }

    fn sidechain(source: u32, dest: u32, plugin: u32, tap: u32) -> HirariRoutingSidechainEntry {
        HirariRoutingSidechainEntry {
            source_id: source,
            dest_id: dest,
            plugin_index: plugin,
            tap_point: tap,
        }
    }

    #[test]
    fn sidechain_rejects_feedback_cycle_without_applying() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        // Existing edge 1 -> 2. Adding 2 -> 1 would close the loop.
        seed_route(&host, 1, 2, 0, 2);
        let host_ptr = host_ptr(&host);
        assert!(!set_sidechain_with(&ops, host_ptr, 2, 1, 0, 2, true, true));
        assert_eq!(route_table(&host), vec![sidechain(1, 2, 0, 2)]);
        assert!(host.links.is_empty());
        assert!(host.undo.is_empty());
    }

    #[test]
    fn sidechain_replacing_source_repoints_the_existing_route() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        seed_route(&host, 1, 3, 0, 2);
        seed_dependency(&host, 1, 3);
        let host_ptr = host_ptr(&host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        assert_eq!(route_table(&host), vec![sidechain(2, 3, 0, 1)]);
        assert_eq!(host.links, vec![(3, 0, 2, 1)]);
        assert!(has_dependency(&host, 2, 3));
        assert!(!has_dependency(&host, 1, 3));
        assert_eq!(host.undo.len(), 1);
        assert_eq!(host.undo[0].name, "Sidechain Connect");
    }

    #[test]
    fn undo_payload_restores_the_previous_sidechain_link() {
        let ops = test_ops();
        let mut host = TestHost::new();
        host.source_found = true;
        host.destination_found = true;
        host.source_prepared = true;
        seed_route(&host, 1, 3, 0, 2);
        seed_dependency(&host, 1, 3);
        let host_ptr = host_ptr(&host);
        assert!(set_sidechain_with(&ops, host_ptr, 2, 3, 0, 1, true, true));
        let undo = host.undo[0].undo.clone();
        assert_eq!(undo.len(), 1);
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(route_table(&host), vec![sidechain(1, 3, 0, 2)]);
        assert_eq!(host.links, vec![(3, 0, 1, 2)]);
        assert!(has_dependency(&host, 1, 3));
        assert!(!has_dependency(&host, 2, 3));
    }

    #[test]
    fn route_gain_undo_restores_the_previous_gain() {
        let ops = test_ops();
        let host = TestHost::new();
        seed_connection(&host, 1, 2, 0.75, false);
        let host_ptr = host_ptr(&host);
        assert!(set_route_gain_with(&ops, host_ptr, 1, 2, 0.5, true));
        assert_eq!(edge(&host, 1, 2).route_gain, 0.5);
        let undo = host.undo[0].undo.clone();
        apply_payloads_with(&ops, host_ptr, &undo);
        assert_eq!(edge(&host, 1, 2).route_gain, 0.75);
    }

    #[test]
    fn feedback_route_rejects_non_finite_and_missing_remove() {
        let ops = test_ops();
        let host = TestHost::new();
        let host_ptr = host_ptr(&host);
        assert!(!set_feedback_route_with(&ops, host_ptr, 1, 2, f32::NAN, true));
        assert!(!set_feedback_route_with(&ops, host_ptr, 1, 2, 1.0, false));
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 1.5, true));
        assert_eq!(edge(&host, 1, 2).feedback_gain, 1.5);
        assert!(set_feedback_route_with(&ops, host_ptr, 1, 2, 0.0, false));
        assert!(edge(&host, 1, 2).feedback_gain < 0.0);
    }

    static FAKE_UNDO: std::sync::atomic::AtomicPtr<c_void> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        FAKE_UNDO.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn route_edit_without_native_recorder_uses_the_rust_history() {
        let host = TestHost::new();
        let host_ptr = host_ptr(&host);
        let state = unsafe { crate::undo_transaction::hirari_undo_manager_create(8, 300) };
        FAKE_UNDO.store(state, std::sync::atomic::Ordering::SeqCst);
        let mut ops = test_ops();
        ops.record_undo = None;
        ops.undo_state = Some(fake_undo_state);
        assert!(set_route_with(&ops, host_ptr, 0, 1, true));
        assert!(edge(&host, 0, 1).route_enabled);
        assert_eq!(
            unsafe { crate::undo_transaction::hirari_undo_manager_undo_count(state) },
            1
        );
        assert!(unsafe { crate::undo_transaction::hirari_undo_manager_apply(state, false) });
        assert!(!edge(&host, 0, 1).route_enabled);
        unsafe { crate::undo_transaction::hirari_undo_manager_destroy(state) };
        FAKE_UNDO.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
