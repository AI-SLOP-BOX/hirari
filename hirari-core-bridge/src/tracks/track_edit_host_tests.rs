//! Tests split out of track_edit_host.rs.

    use super::*;

    use crate::channel_strip::{
        hirari_channel_strip_create, hirari_channel_strip_destroy, hirari_channel_strip_gain,
        hirari_channel_strip_pan, hirari_channel_strip_set_gain, hirari_channel_strip_set_pan,
    };
    use crate::effect_chain_host::{
        hirari_effect_chain_add_builtin, hirari_effect_chain_bypass, hirari_effect_chain_count,
        hirari_effect_chain_create, hirari_effect_chain_destroy, hirari_effect_chain_node_kind,
        hirari_effect_chain_set_bypass,
    };
    use crate::track_automation_store::{
        hirari_track_automation_store_create, hirari_track_automation_store_destroy,
        hirari_track_automation_store_points, hirari_track_automation_store_plugin_lane_views,
        hirari_track_automation_store_set_plugin_lanes, hirari_track_automation_store_set_points,
        AutomationPoint,
    };
    use crate::track_runtime::{
        hirari_track_runtime_create, hirari_track_runtime_destroy,
        hirari_track_runtime_is_muted, hirari_track_runtime_is_phase_inverted,
        hirari_track_runtime_set_muted, hirari_track_runtime_set_phase_invert,
    };

    /// One track's Rust storages, destroyed in the order the runtime releases
    /// the handles it borrows.
    struct Stores {
        strip: *mut c_void,
        chain: *mut c_void,
        automation: *mut c_void,
        runtime: *mut c_void,
        name: *mut c_void,
    }

    impl Stores {
        fn new(id: u32) -> Self {
            let strip = unsafe { hirari_channel_strip_create() };
            let chain = unsafe { hirari_effect_chain_create() };
            let automation = unsafe { hirari_track_automation_store_create() };
            let name = unsafe { crate::track_name_store::hirari_track_name_store_create() };
            let runtime = unsafe {
                hirari_track_runtime_create(
                    id, 0, chain, strip, automation, std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                )
            };
            Self { strip, chain, automation, runtime, name }
        }
        fn set_name(&self, name: &[u8]) {
            assert!(unsafe {
                crate::track_name_store::hirari_track_name_store_set(
                    self.name,
                    name.as_ptr(),
                    name.len(),
                )
            });
        }
        fn name(&self) -> Vec<u8> {
            crate::track_name_store::read(self.name as *const c_void)
        }
    }

    impl Drop for Stores {
        fn drop(&mut self) {
            unsafe {
                hirari_track_runtime_destroy(self.runtime);
                hirari_track_automation_store_destroy(self.automation);
                hirari_effect_chain_destroy(self.chain);
                hirari_channel_strip_destroy(self.strip);
                crate::track_name_store::hirari_track_name_store_destroy(self.name);
            }
        }
    }

    fn points(store: *mut c_void, kind: u32) -> Vec<AutomationPoint> {
        let mut count = 0usize;
        let raw = unsafe { hirari_track_automation_store_points(store, kind, &mut count) };
        if raw.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(raw, count) }.to_vec()
    }

    #[test]
    fn cloning_state_copies_stores_onto_a_fresh_track() {
        let source = Stores::new(1);
        let dest = Stores::new(2);
        unsafe {
            hirari_channel_strip_set_gain(source.strip, 0.5);
            hirari_channel_strip_set_pan(source.strip, -0.25);
            hirari_channel_strip_set_gain(dest.strip, 1.0);
            hirari_track_runtime_set_muted(source.runtime, 1);
            hirari_track_runtime_set_phase_invert(source.runtime, 1);
            let volume = [
                AutomationPoint { time: 4.0, value: 0.2, curve: 0.0 },
                AutomationPoint { time: 1.0, value: 0.8, curve: 0.5 },
            ];
            hirari_track_automation_store_set_points(source.automation, 0, volume.as_ptr(), 2);
            let delay = [AutomationPoint { time: 2.0, value: 0.5, curve: 0.0 }];
            hirari_track_automation_store_set_points(
                source.automation,
                2,
                delay.as_ptr(),
                delay.len(),
            );
            let lane = [AutomationPoint { time: 3.0, value: 0.75, curve: 0.25 }];
            let lanes = [crate::track_automation_store::PluginLaneView {
                processor_index: 1,
                parameter_id: 7,
                points: lane.as_ptr(),
                point_count: 1,
            }];
            hirari_track_automation_store_set_plugin_lanes(
                source.automation,
                lanes.as_ptr(),
                lanes.len(),
            );
            assert!(hirari_effect_chain_add_builtin(source.chain, 1, false, false));
            assert!(hirari_effect_chain_add_builtin(source.chain, 4, false, false));
            assert!(hirari_effect_chain_set_bypass(source.chain, 1, true));
            source.set_name(b"Guitar");
        }
        // The chain bypass and processor state follow the source node, so the
        // duplicate reports the same insert list and the same parameter values.
        unsafe {
            assert!(hirari_track_clone_state(
                source.strip,
                source.runtime,
                source.automation,
                source.chain,
                source.name,
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            ));
        }
        assert_eq!(dest.name(), b"Guitar Copy".to_vec());
        assert_eq!(source.name(), b"Guitar".to_vec());
        assert_eq!(unsafe { hirari_channel_strip_gain(dest.strip) }, 0.5);
        assert_eq!(unsafe { hirari_channel_strip_pan(dest.strip) }, -0.25);
        assert!(unsafe { hirari_track_runtime_is_muted(dest.runtime) });
        assert!(unsafe { hirari_track_runtime_is_phase_inverted(dest.runtime) });
        // The automation store sorts by time as it publishes.
        assert_eq!(
            points(dest.automation, 0),
            vec![
                AutomationPoint { time: 1.0, value: 0.8, curve: 0.5 },
                AutomationPoint { time: 4.0, value: 0.2, curve: 0.0 },
            ]
        );
        assert_eq!(points(dest.automation, 1), Vec::new());
        assert_eq!(
            points(dest.automation, 2),
            vec![AutomationPoint { time: 2.0, value: 0.5, curve: 0.0 }]
        );
        let mut lane_count = 0usize;
        let views =
            unsafe { hirari_track_automation_store_plugin_lane_views(dest.automation, &mut lane_count) };
        assert_eq!(lane_count, 1);
        let lane = unsafe { &*views };
        assert_eq!((lane.processor_index, lane.parameter_id, lane.point_count), (1, 7, 1));
        assert_eq!(unsafe { hirari_effect_chain_count(dest.chain) }, 2);
        assert_eq!(
            unsafe {
                (
                    hirari_effect_chain_node_kind(dest.chain, 0),
                    hirari_effect_chain_node_kind(dest.chain, 1),
                )
            },
            (1, 4)
        );
        assert_eq!(chain_state(dest.chain, 1), chain_state(source.chain, 1));
        assert!(!unsafe { hirari_effect_chain_bypass(dest.chain, 0) });
        assert!(unsafe { hirari_effect_chain_bypass(dest.chain, 1) });
    }

    #[test]
    fn cloning_state_without_a_source_chain_copies_nothing() {
        let dest = Stores::new(2);
        assert!(!unsafe {
            hirari_track_clone_state(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            )
        });
    }

    #[test]
    fn a_name_that_cannot_take_the_suffix_aborts_the_clone() {
        let source = Stores::new(1);
        let dest = Stores::new(2);
        source.set_name(&vec![b'a'; crate::track_name_store::MAX_TRACK_NAME_BYTES]);
        dest.set_name(b"keep");
        unsafe { hirari_channel_strip_set_gain(source.strip, 0.5) };
        assert!(!unsafe {
            hirari_track_clone_state(
                source.strip,
                source.runtime,
                source.automation,
                source.chain,
                source.name,
                dest.strip,
                dest.runtime,
                dest.automation,
                dest.chain,
                dest.name,
                false,
            )
        });
        // Nothing crossed, because the name is settled before the stores are.
        assert_eq!(unsafe { hirari_channel_strip_gain(dest.strip) }, 1.0);
        assert_eq!(dest.name(), b"keep".to_vec());
    }
