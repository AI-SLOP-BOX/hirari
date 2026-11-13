//! Tests split out of layout_gather.rs.
    use super::*;
    use crate::channel_strip::{
        hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_set_gain,
        hirari_channel_strip_set_pan, hirari_channel_strip_set_record_armed,
        hirari_channel_strip_set_solo,
    };
    use crate::effect_chain_host::{
        hirari_effect_chain_add_builtin, hirari_effect_chain_create, hirari_effect_chain_destroy,
        hirari_effect_chain_parameter, hirari_effect_chain_parameter_count,
        hirari_effect_chain_set_bypass,
    };
    use crate::track_automation_store::{
        hirari_track_automation_store_create, hirari_track_automation_store_destroy,
        hirari_track_automation_store_set_plugin_lanes, hirari_track_automation_store_set_points,
        AutomationPoint, PluginLaneView,
    };
    use crate::track_frozen_store::{
        hirari_track_frozen_store_create, hirari_track_frozen_store_destroy,
        hirari_track_frozen_store_publish,
    };
    use crate::track_midi_articulation::{
        hirari_track_midi_articulation_create, hirari_track_midi_articulation_destroy,
        hirari_track_midi_expression_map_set,
    };
    use crate::track_runtime::{
        hirari_track_runtime_create, hirari_track_runtime_destroy, hirari_track_runtime_set_delays,
        hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
        hirari_track_runtime_set_spatial,
    };
    use std::sync::Mutex;

    /// A track assembled from the same Rust storages the engine hands over, so
    /// the gather is tested against the reads it really performs.
    struct MockTrack {
        id: u32,
        track_type: u32,
        name: *mut c_void,
        strip: *mut c_void,
        chain: *mut c_void,
        automation: *mut c_void,
        frozen: *mut c_void,
        runtime: *mut c_void,
        articulation: *mut c_void,
    }

    impl Drop for MockTrack {
        fn drop(&mut self) {
            unsafe {
                hirari_track_runtime_destroy(self.runtime);
                hirari_track_midi_articulation_destroy(self.articulation);
                hirari_track_frozen_store_destroy(self.frozen);
                hirari_track_automation_store_destroy(self.automation);
                hirari_effect_chain_destroy(self.chain);
                hirari_channel_strip_destroy(self.strip);
                crate::track_name_store::hirari_track_name_store_destroy(self.name);
            }
        }
    }

    struct MockHost {
        tracks: Mutex<Vec<MockTrack>>,
        routes: *mut c_void,
        routing: *mut c_void,
        gauge: Mutex<(u32, u32)>,
    }

    impl Drop for MockHost {
        fn drop(&mut self) {
            unsafe {
                crate::sidechain_route_store::hirari_sidechain_routes_destroy(self.routes);
                crate::routing_engine_core::hirari_routing_engine_destroy(self.routing);
            }
        }
    }

    fn host_opaque(host: *mut c_void) -> &'static MockHost {
        unsafe { &*host.cast::<MockHost>() }
    }

    fn mock_track(track: *mut c_void) -> &'static MockTrack {
        unsafe { &*track.cast::<MockTrack>() }
    }

    unsafe extern "C" fn begin(host: *mut c_void) {
        host_opaque(host).gauge.lock().unwrap().0 += 1;
    }
    unsafe extern "C" fn end(host: *mut c_void) {
        host_opaque(host).gauge.lock().unwrap().1 += 1;
    }
    unsafe extern "C" fn track_count(host: *mut c_void) -> usize {
        host_opaque(host).tracks.lock().unwrap().len()
    }
    unsafe extern "C" fn track_handle(host: *mut c_void, index: usize) -> *mut c_void {
        let tracks = host_opaque(host).tracks.lock().unwrap();
        tracks
            .get(index)
            .map_or(std::ptr::null_mut(), |track| track as *const MockTrack as *mut c_void)
    }
    unsafe extern "C" fn track_id(track: *mut c_void) -> u32 {
        if track.is_null() {
            return 0;
        }
        mock_track(track).id
    }
    unsafe extern "C" fn track_type(track: *mut c_void) -> u32 {
        if track.is_null() {
            return 0;
        }
        mock_track(track).track_type
    }
    unsafe extern "C" fn track_storage(track: *mut c_void, kind: u32) -> *mut c_void {
        if track.is_null() {
            return std::ptr::null_mut();
        }
        let track = mock_track(track);
        match kind {
            STRIP => track.strip,
            CHAIN => track.chain,
            AUTOMATION => track.automation,
            FROZEN => track.frozen,
            RUNTIME => track.runtime,
            ARTICULATION => track.articulation,
            _ => track.name,
        }
    }
    unsafe extern "C" fn sidechain_routes_store(host: *mut c_void) -> *mut c_void {
        host_opaque(host).routes
    }
    unsafe extern "C" fn routing_store(host: *mut c_void) -> *mut c_void {
        host_opaque(host).routing
    }
    unsafe extern "C" fn sample_rate(_: *mut c_void) -> f64 {
        48_000.0
    }

    fn ops() -> HirariLayoutHostOps {
        HirariLayoutHostOps {
            begin,
            end,
            track_count,
            track_handle,
            track_id,
            track_type,
            track_storage,
            sidechain_routes_store,
            routing_store,
            sample_rate,
        }
    }

    fn fixture() -> Box<MockHost> {
        let routes = crate::sidechain_route_store::hirari_sidechain_routes_create();
        let routing = crate::routing_engine_core::hirari_routing_engine_create();
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(routes, 1, 2, 3, 1);
            crate::routing_engine_core::hirari_routing_engine_add_feedback(routing, 1, 2, 0.5);
        }
        let strip = hirari_channel_strip_create();
        let name = unsafe { crate::track_name_store::hirari_track_name_store_create() };
        let chain = hirari_effect_chain_create();
        let automation = hirari_track_automation_store_create();
        let frozen = { hirari_track_frozen_store_create() };
        let articulation = hirari_track_midi_articulation_create();
        let null = std::ptr::null_mut();
        let runtime = unsafe {
            hirari_track_runtime_create(
                7, 0, chain, strip, automation, frozen, null, null, null, null, null, null, null,
                null, articulation, null,
            )
        };
        let volume_points = [
            AutomationPoint {
                time: 1.0,
                value: 0.25,
                curve: 0.0,
            },
            AutomationPoint {
                time: 2.0,
                value: 0.75,
                curve: 0.5,
            },
        ];
        let lane_points = [AutomationPoint {
            time: 3.0,
            value: 0.5,
            curve: 0.0,
        }];
        let lanes = [PluginLaneView {
            processor_index: 1,
            parameter_id: 2,
            points: lane_points.as_ptr(),
            point_count: lane_points.len(),
        }];
        let frozen_path = b"/tmp/hirari-frozen.wav";
        // The store owns a published render, so the fixture hands it a real
        // audio storage rather than an invented handle.
        let frozen_audio = crate::audio_buffer_ops::hirari_audio_buffer_storage_create();
        unsafe {
            hirari_channel_strip_set_gain(strip, 0.5);
            assert!(crate::track_name_store::hirari_track_name_store_set(
                name,
                b"Bass".as_ptr(),
                b"Bass".len()
            ));
            hirari_channel_strip_set_pan(strip, -0.25);
            hirari_channel_strip_set_solo(strip, true);
            hirari_channel_strip_set_record_armed(strip, true);
            hirari_track_runtime_set_muted(runtime, 1);
            hirari_track_runtime_set_phase_invert(runtime, 1);
            hirari_track_runtime_set_delays(runtime, 12, 34);
            hirari_track_runtime_set_spatial(runtime, 0.1, 0.2, 0.3);
            hirari_track_automation_store_set_points(
                automation,
                0,
                volume_points.as_ptr(),
                volume_points.len(),
            );
            hirari_track_automation_store_set_plugin_lanes(automation, lanes.as_ptr(), lanes.len());
            assert!(hirari_effect_chain_add_builtin(chain, 1, false, false));
            assert!(hirari_effect_chain_add_builtin(chain, 4, false, false));
            assert!(hirari_effect_chain_set_bypass(chain, 0, true));
            assert!(hirari_track_frozen_store_publish(
                frozen,
                frozen_audio,
                2,
                4,
                4,
                48_000,
                7,
                3,
                frozen_path.as_ptr(),
                frozen_path.len(),
            ));
            assert!(hirari_track_midi_expression_map_set(
                articulation,
                0,
                b"{}".as_ptr(),
                2
            ));
        }
        Box::new(MockHost {
            tracks: Mutex::new(vec![MockTrack {
                id: 7,
                track_type: 1,
                name,
                strip,
                chain,
                automation,
                frozen,
                runtime,
                articulation,
            }]),
            routes,
            routing,
            gauge: Mutex::new((0, 0)),
        })
    }

    unsafe fn snapshot_of(host: &MockHost) -> (HirariProjectLayoutSnapshot, *mut c_void) {
        let ops = ops();
        let storage = hirari_layout_gather(
            (&*host as *const MockHost).cast_mut().cast(),
            &ops,
            false,
            None,
            std::ptr::null(),
        );
        if storage.is_null() {
            return (HirariProjectLayoutSnapshot::default(), storage);
        }
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        (snapshot, storage)
    }

    fn text(value: &HirariLayoutString) -> &[u8] {
        if value.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(value.data, value.len) }
    }

    #[test]
    fn gather_reads_the_tracks_own_rust_storages() {
        let host = fixture();
        let chain = host.tracks.lock().unwrap()[0].chain;
        let (snapshot, storage) = unsafe { snapshot_of(&host) };
        assert_eq!(snapshot.track_count, 1);
        assert_eq!(snapshot.sidechain_route_count, 1);
        assert_eq!(snapshot.feedback_route_count, 1);
        assert_eq!(snapshot.timeline_sample_rate, 48_000.0);
        let track = unsafe { &*snapshot.tracks };
        assert_eq!(track.id, 7);
        assert_eq!(track.track_type, 1);
        assert_eq!(track.volume, 0.5);
        assert_eq!(track.pan, -0.25);
        assert!(track.solo);
        assert!(track.record_armed);
        // Mute, phase and timing are the runtime's; freeze is the store's.
        assert!(track.muted);
        assert!(track.phase_inverted);
        assert_eq!(track.track_delay_samples, 34);
        assert_eq!(
            (track.spatial_x, track.spatial_y, track.spatial_z),
            (0.1, 0.2, 0.3)
        );
        assert!(track.frozen);
        assert_eq!(track.frozen_total_samples, 4);
        assert_eq!(track.frozen_sample_rate, 48_000);
        assert_eq!(text(&track.name), b"Bass");
        assert_eq!(text(&track.frozen_path), b"/tmp/hirari-frozen.wav");
        assert_eq!(text(&track.expression_map_json), b"{}");
        assert_eq!(text(&track.expression_map_pro_json), b"null");
        let volume =
            unsafe { std::slice::from_raw_parts(track.volume_automation, track.volume_automation_count) };
        assert_eq!(
            volume
                .iter()
                .map(|point| (point.time, point.value, point.curve))
                .collect::<Vec<_>>(),
            vec![(1.0, 0.25, 0.0), (2.0, 0.75, 0.5)]
        );
        assert_eq!(track.pan_automation_count, 0);
        assert_eq!(track.plugin_automation_count, 1);
        let lane = unsafe { &*track.plugin_automation };
        assert_eq!((lane.plugin_index, lane.parameter_id, lane.point_count), (1, 2, 1));
        let lane_point = unsafe { &*lane.points };
        assert_eq!((lane_point.time, lane_point.value), (3.0, 0.5));
        assert_eq!(track.plugin_type_count, 2);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(track.plugin_types, track.plugin_type_count) },
            &[1, 4]
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(track.plugin_bypass, track.plugin_type_count) },
            &[1, 0]
        );
        assert_eq!(track.plugin_state_count, 2);
        assert_eq!(track.sandbox_plugin_path_count, 0);
        assert!(track.plugin_gui_states.is_null());
        for plugin in 0..2 {
            let state = unsafe { &*track.plugin_states.add(plugin) };
            assert_eq!(
                unsafe { std::slice::from_raw_parts(state.bytes, state.count) },
                chain_bytes(chain, plugin as u32, false)
            );
            // The JSON path reports the plug-in's parameters in id order.
            let block = unsafe { &*track.plugin_parameters.add(plugin) };
            let values =
                unsafe { std::slice::from_raw_parts(block.values, block.count) };
            assert_eq!(
                values
                    .iter()
                    .enumerate()
                    .map(|(id, value)| (id as u32, *value))
                    .collect::<Vec<_>>(),
                (0..unsafe { hirari_effect_chain_parameter_count(chain, plugin as u32) })
                    .map(|id| (id, unsafe {
                        hirari_effect_chain_parameter(chain, plugin as u32, id)
                    }))
                    .collect::<Vec<_>>()
            );
        }
        unsafe { hirari_layout_release(storage) };
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn save_only_gather_fills_gui_states_instead_of_parameters() {
        let host = fixture();
        let chain = host.tracks.lock().unwrap()[0].chain;
        let ops = ops();
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &ops,
                true,
                None,
                std::ptr::null(),
            )
        };
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        let track = unsafe { &*snapshot.tracks };
        assert!(track.plugin_parameters.is_null());
        assert_eq!(track.plugin_gui_state_count, 2);
        for plugin in 0..2 {
            let gui = unsafe { &*track.plugin_gui_states.add(plugin) };
            assert_eq!(
                unsafe { std::slice::from_raw_parts(gui.bytes, gui.count) },
                chain_bytes(chain, plugin as u32, true)
            );
        }
        unsafe { hirari_layout_release(storage) };
    }

    #[test]
    fn cancellation_releases_the_host_lock() {
        unsafe extern "C" fn cancelled(_: *const c_void) -> bool {
            true
        }
        let host = fixture();
        let ops = ops();
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &ops,
                false,
                Some(cancelled),
                std::ptr::null(),
            )
        };
        assert!(storage.is_null());
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn null_inputs_are_rejected() {
        let ops = ops();
        assert!(unsafe {
            hirari_layout_gather(std::ptr::null_mut(), &ops, false, None, std::ptr::null())
        }
        .is_null());
        assert!(!unsafe { hirari_layout_snapshot(std::ptr::null(), std::ptr::null_mut()) });
        unsafe { hirari_layout_release(std::ptr::null_mut()) };
    }

    #[test]
    fn retired_track_slots_are_skipped() {
        unsafe extern "C" fn retired_handle(_: *mut c_void, _: usize) -> *mut c_void {
            std::ptr::null_mut()
        }
        let host = fixture();
        let mut table = ops();
        table.track_handle = retired_handle;
        let storage = unsafe {
            hirari_layout_gather(
                (&*host as *const MockHost).cast_mut().cast(),
                &table,
                false,
                None,
                std::ptr::null(),
            )
        };
        assert!(!storage.is_null());
        let mut snapshot = HirariProjectLayoutSnapshot::default();
        assert!(unsafe { hirari_layout_snapshot(storage, &mut snapshot) });
        assert_eq!(snapshot.track_count, 0);
        unsafe { hirari_layout_release(storage) };
        assert_eq!(*host.gauge.lock().unwrap(), (1, 1));
    }

    #[test]
    fn source_rate_is_clamped_to_the_supported_range() {
        assert_eq!(clamped_source_rate(96_000.0, 48_000.0), 96_000);
        assert_eq!(clamped_source_rate(0.0, 44_100.0), 44_100);
        assert_eq!(clamped_source_rate(1.0, 48_000.0), 8_000);
        assert_eq!(clamped_source_rate(1_000_000.0, 48_000.0), 384_000);
    }
