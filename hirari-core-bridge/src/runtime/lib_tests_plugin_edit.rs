// The plugin edit host owns the chain, freeze and layout protocol, so these
// tests drive it through a real engine: the tracks lock, the track's own Rust
// stores and the engine's undo history are the ones under test. Host-op tables
// are process-global, which is why nothing here swaps them for fakes.
struct PluginEditEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: Box<crate::AudioEngine>,
    audio_track: u32,
    bus_track: u32,
}

impl PluginEditEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::new_audio_engine();
        assert!(!false);
        let reference = &*engine;
        let audio_track = reference.add_track(0);
        let bus_track = reference.add_track(3);
        assert_ne!(audio_track, 0);
        assert_ne!(bus_track, 0);
        // Track creation is itself reversible, so the history is emptied after
        // the fixture's own edits to give every test an absolute count.
        reference.clear_undo_history();
        assert_eq!(reference.get_undo_count(), 0);
        Self { _guard: guard, engine, audio_track, bus_track }
    }

    fn engine(&self) -> &crate::AudioEngine {
        &*self.engine
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.engine())
    }

    /// Reads the track's chain with the engine lock held, the way every edit
    /// entry point does.
    fn chain_read<T>(&self, track_id: u32, read: impl FnOnce(*mut std::ffi::c_void) -> T) -> T {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        read(session.chain())
    }

    fn kinds(&self, track_id: u32) -> Vec<u32> {
        self.chain_read(track_id, |chain| {
            let count = unsafe { crate::effect_chain_host::hirari_effect_chain_count(chain as *const std::ffi::c_void) };
            (0..count)
                .map(|index| {
                    unsafe {
                        crate::effect_chain_host::hirari_effect_chain_node_kind(
                            chain as *const std::ffi::c_void,
                            index,
                        )
                    }
                })
                .collect()
        })
    }

    fn bypassed(&self, track_id: u32, index: u32) -> bool {
        self.chain_read(track_id, |chain| unsafe {
            crate::effect_chain_host::hirari_effect_chain_bypass(chain as *const std::ffi::c_void, index)
        })
    }

    fn parameter(&self, track_id: u32, index: u32, parameter_id: u32) -> f32 {
        self.chain_read(track_id, |chain| unsafe {
            crate::effect_chain_host::hirari_effect_chain_parameter(
                chain as *const std::ffi::c_void,
                index,
                parameter_id,
            )
        })
    }

    fn frozen(&self, track_id: u32) -> bool {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_is_frozen(session.frozen()) }
    }

    /// The track's name, read from its own Rust store with the lock held.
    fn name(&self, track_id: u32) -> Vec<u8> {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        session.read_name()
    }

    fn edit_rename(&self, track_id: u32, name: &[u8]) -> bool {
        unsafe {
            crate::control_edit_host::hirari_control_edit_rename_track(
                self.host(),
                track_id,
                name.as_ptr(),
                name.len(),
            )
        }
    }

    // The entry points take the raw engine host; these wrappers keep that pointer
    // handoff in one place so the assertions stay about the policy.
    fn edit_bypass(&self, track_id: u32, index: u32, bypassed: bool) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_set_bypass(
                self.host(),
                track_id,
                index,
                bypassed,
            )
        }
    }

    fn edit_move(&self, track_id: u32, from: u32, to: u32) -> bool {
        unsafe { crate::plugin_edit_host::hirari_plugin_edit_move(self.host(), track_id, from, to) }
    }

    fn edit_remove(&self, track_id: u32, index: u32) -> bool {
        unsafe { crate::plugin_edit_host::hirari_plugin_edit_remove(self.host(), track_id, index) }
    }

    fn edit_parameter(&self, track_id: u32, index: u32, parameter_id: u32, value: f32) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_set_parameter(
                self.host(),
                track_id,
                index,
                parameter_id,
                value,
            )
        }
    }

    fn edit_parameter_without_undo(
        &self,
        track_id: u32,
        index: u32,
        parameter_id: u32,
        value: f32,
    ) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_set_parameter_without_undo(
                self.host(),
                track_id,
                index,
                parameter_id,
                value,
            )
        }
    }

    fn edit_add(&self, track_id: u32, plugin_type: u32) -> bool {
        unsafe { crate::plugin_edit_host::hirari_plugin_edit_add(self.host(), track_id, plugin_type) }
    }

    fn edit_get_state(&self, track_id: u32, index: u32, out: *mut u8, capacity: usize) -> usize {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_get_state(
                self.host(),
                track_id,
                index,
                out,
                capacity,
            )
        }
    }

    fn edit_set_state(&self, track_id: u32, index: u32, state: &[u8]) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_set_state(
                self.host(),
                track_id,
                index,
                state.as_ptr(),
                state.len(),
            )
        }
    }

    fn edit_load_preset(&self, track_id: u32, index: u32, path: &[u8]) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_load_preset(
                self.host(),
                track_id,
                index,
                path.as_ptr(),
                path.len(),
            )
        }
    }

    fn edit_save_preset(&self, track_id: u32, index: u32, path: &[u8]) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_save_preset(
                self.host(),
                track_id,
                index,
                path.as_ptr(),
                path.len(),
            )
        }
    }
}

#[test]
fn the_session_resolves_the_tracks_own_stores_and_type() {
    let fixture = PluginEditEngine::new();
    let host = fixture.host();

    let session = crate::track_session::TrackSession::open(host, fixture.audio_track)
        .expect("audio track must open a session");
    assert!(!session.chain().is_null());
    assert!(!session.frozen().is_null());
    assert!(!session.is_bus());
    drop(session);

    let bus = crate::track_session::TrackSession::open(host, fixture.bus_track)
        .expect("bus track must open a session");
    assert!(bus.is_bus());
    drop(bus);

    // A missing track is released and refused, so no edit can mutate a stale
    // handle and the engine lock never stays held.
    assert!(crate::track_session::TrackSession::open(host, u32::MAX).is_none());
    assert!(crate::track_session::TrackSession::open(std::ptr::null_mut(), fixture.audio_track)
        .is_none());
    assert_eq!(fixture.engine().get_undo_count(), 0);
}

#[test]
fn adding_a_plugin_records_one_entry_the_history_replays_both_ways() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;

    assert!(fixture.engine().add_plugin(track, 2));
    assert_eq!(fixture.kinds(track), vec![2]);
    assert_eq!(fixture.engine().get_undo_count(), 1);
    assert_eq!(fixture.engine().get_redo_count(), 0);

    fixture.engine().undo();
    assert_eq!(fixture.kinds(track), Vec::<u32>::new());
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert_eq!(fixture.engine().get_redo_count(), 1);

    fixture.engine().redo();
    assert_eq!(fixture.kinds(track), vec![2]);
    assert_eq!(fixture.engine().get_undo_count(), 1);
    assert_eq!(fixture.engine().get_redo_count(), 0);
    assert!(!fixture.frozen(track));
}

#[test]
fn removing_a_plugin_restores_the_same_processor_in_the_same_slot() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    assert!(fixture.engine().add_plugin(track, 3));
    assert!(fixture.edit_bypass(track, 1, true));
    let history = fixture.engine().get_undo_count();

    assert!(fixture.edit_remove(track, 1));
    assert_eq!(fixture.kinds(track), vec![2]);
    assert_eq!(fixture.engine().get_undo_count(), history + 1);

    // Undo re-inserts the detached node by its token, so the bypass the user had
    // set comes back with it.
    fixture.engine().undo();
    assert_eq!(fixture.kinds(track), vec![2, 3]);
    assert!(fixture.bypassed(track, 1));

    fixture.engine().redo();
    assert_eq!(fixture.kinds(track), vec![2]);
}

#[test]
fn a_bypass_edit_records_even_when_the_value_does_not_change() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    let history = fixture.engine().get_undo_count();

    assert!(fixture.edit_bypass(track, 0, true));
    assert!(fixture.bypassed(track, 0));
    assert_eq!(fixture.engine().get_undo_count(), history + 1);

    assert!(fixture.edit_bypass(track, 0, true));
    assert_eq!(fixture.engine().get_undo_count(), history + 2);

    fixture.engine().undo();
    assert!(fixture.bypassed(track, 0));
    fixture.engine().undo();
    assert!(!fixture.bypassed(track, 0));
    assert_eq!(fixture.engine().get_undo_count(), history);
}

#[test]
fn rejected_plugin_edits_leave_the_chain_and_the_history_untouched() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    let history = fixture.engine().get_undo_count();

    // Out-of-range plugin, the same-index move and a missing track are all
    // refused before anything is recorded.
    assert!(!fixture.edit_bypass(track, 5, true));
    assert!(!fixture.edit_move(track, 0, 0));
    assert!(!fixture.edit_move(track, 0, 1));
    assert!(!fixture.edit_remove(track, 7));
    assert!(!fixture.edit_parameter(track, 0, 0, f32::NAN));
    assert!(!fixture.edit_parameter_without_undo(track, 0, 0, f32::INFINITY));
    assert!(!fixture.edit_add(u32::MAX, 2));
    assert!(!fixture.edit_bypass(u32::MAX, 0, true));
    assert_eq!(fixture.kinds(track), vec![2]);
    assert_eq!(fixture.engine().get_undo_count(), history);

    // Reads never record either: the probe reports the same length twice, and a
    // plugin or track that is not there reports zero.
    assert_eq!(
        fixture.edit_get_state(track, 0, std::ptr::null_mut(), 0),
        fixture.edit_get_state(track, 0, std::ptr::null_mut(), 0)
    );
    assert_eq!(fixture.edit_get_state(track, 99, std::ptr::null_mut(), 0), 0);
    assert_eq!(fixture.edit_get_state(u32::MAX, 0, std::ptr::null_mut(), 0), 0);
    assert_eq!(fixture.engine().get_undo_count(), history);
}

#[test]
fn moving_a_plugin_reorders_the_chain_and_undo_reverses_it() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    assert!(fixture.engine().add_plugin(track, 3));
    assert_eq!(fixture.kinds(track), vec![2, 3]);

    assert!(fixture.engine().move_plugin(track, 0, 1));
    assert_eq!(fixture.kinds(track), vec![3, 2]);
    assert_eq!(fixture.engine().get_undo_count(), 3);

    fixture.engine().undo();
    assert_eq!(fixture.kinds(track), vec![2, 3]);
    fixture.engine().redo();
    assert_eq!(fixture.kinds(track), vec![3, 2]);
}

#[test]
fn a_parameter_edit_keeps_the_exact_value_in_history() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    let before = fixture.parameter(track, 0, 0);
    let history = fixture.engine().get_undo_count();

    assert!(fixture.engine().set_plugin_parameter(track, 0, 0, 0.25));
    assert_eq!(fixture.parameter(track, 0, 0), 0.25);
    assert_eq!(fixture.engine().get_undo_count(), history + 1);

    // Hydration writes the value without owning the edit log.
    assert!(fixture.engine().set_plugin_parameter_without_undo(track, 0, 0, 0.5));
    assert_eq!(fixture.parameter(track, 0, 0), 0.5);
    assert_eq!(fixture.engine().get_undo_count(), history + 1);

    fixture.engine().undo();
    assert_eq!(fixture.parameter(track, 0, 0), before);
    fixture.engine().redo();
    assert_eq!(fixture.parameter(track, 0, 0), 0.25);
}

#[test]
fn unknown_builtin_types_are_refused_before_anything_is_recorded() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;

    // The catalog ends at type 10; an unknown type must never insert an inert
    // processor.
    assert!(!fixture.engine().add_plugin(track, 11));
    assert!(!fixture.engine().add_plugin(track, u32::MAX));
    assert_eq!(fixture.kinds(track), Vec::<u32>::new());
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(!fixture.frozen(track));
}

#[test]
fn a_bus_track_gets_bus_admission_from_its_own_session() {
    let fixture = PluginEditEngine::new();

    // Type 0 resolves to Hirari/BusLimiter on a bus, which no DSP implements yet,
    // so the bus insert is refused; the same type is admitted on an audio track.
    assert!(!fixture.engine().add_plugin(fixture.bus_track, 0));
    assert_eq!(fixture.kinds(fixture.bus_track), Vec::<u32>::new());
    assert!(fixture.engine().add_plugin(fixture.audio_track, 0));
    assert_eq!(fixture.kinds(fixture.audio_track), vec![0]);

    // Later slots are shared by buses and tracks.
    assert!(fixture.engine().add_plugin(fixture.bus_track, 2));
    assert_eq!(fixture.kinds(fixture.bus_track), vec![2]);
    assert_eq!(fixture.engine().get_undo_count(), 2);
}

#[test]
fn sandbox_paths_are_checked_before_the_track_is_touched() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;

    assert!(!fixture.engine().add_sandboxed_plugin(track, ""));
    assert!(!fixture.engine().add_sandboxed_plugin(track, "/tmp/not-a-bundle.txt"));
    assert!(!fixture.engine().add_sandboxed_plugin(
        track,
        "/definitely/missing/hirari-plugin.clap"
    ));
    assert!(!fixture.engine().add_sandboxed_plugin(u32::MAX, "/definitely/missing/hirari-plugin.clap"));
    assert_eq!(fixture.kinds(track), Vec::<u32>::new());
    assert_eq!(fixture.engine().get_undo_count(), 0);
}

#[test]
fn state_writes_reject_bad_input_without_touching_the_history() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    let history = fixture.engine().get_undo_count();

    // A non-empty length with no buffer, and a plugin the chain does not have.
    assert!(!unsafe {
        crate::plugin_edit_host::hirari_plugin_edit_set_state(
            fixture.host(),
            track,
            0,
            std::ptr::null(),
            8,
        )
    });
    assert!(!fixture.edit_set_state(track, 9, &[0_u8; 4]));
    assert!(!fixture.edit_set_state(u32::MAX, 0, &[0_u8; 4]));
    assert_eq!(fixture.engine().get_undo_count(), history);
    assert_eq!(fixture.kinds(track), vec![2]);
}

#[test]
fn preset_requests_reject_before_the_chain_is_read() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.engine().add_plugin(track, 2));
    let history = fixture.engine().get_undo_count();

    // An empty path, an unreadable file and a missing track all fail without a
    // history entry.
    assert!(!fixture.edit_load_preset(track, 0, b""));
    assert!(!fixture.edit_load_preset(track, 0, b"/definitely/missing/hirari-preset.hiraripreset"));
    assert!(!fixture.edit_load_preset(u32::MAX, 0, b"/definitely/missing/hirari-preset.hiraripreset"));
    assert!(!fixture.edit_save_preset(track, 0, b""));
    assert!(!fixture.edit_save_preset(u32::MAX, 0, b"/tmp/hirari-preset-test.hiraripreset"));
    assert_eq!(fixture.engine().get_undo_count(), history);
}

#[test]
fn renaming_a_track_records_one_entry_the_history_replays_it() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    let before = fixture.name(track);
    assert!(!before.is_empty());
    let history = fixture.engine().get_undo_count();

    assert!(fixture.edit_rename(track, b"Drums"));
    assert_eq!(fixture.name(track), b"Drums".to_vec());
    assert_eq!(fixture.engine().get_undo_count(), history + 1);

    fixture.engine().undo();
    assert_eq!(fixture.name(track), before);
    fixture.engine().redo();
    assert_eq!(fixture.name(track), b"Drums".to_vec());
}

#[test]
fn an_identical_name_is_a_no_op_and_a_bad_name_touches_nothing() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    let before = fixture.name(track);
    let history = fixture.engine().get_undo_count();

    // The name already matches, so the store is left alone and no entry lands.
    assert!(fixture.edit_rename(track, &before));
    assert_eq!(fixture.engine().get_undo_count(), history);

    // Empty, NUL-bearing and oversized names are refused, and so is a track the
    // engine does not have.
    assert!(!fixture.edit_rename(track, b""));
    assert!(!fixture.edit_rename(track, b"bad\0name"));
    assert!(!fixture.edit_rename(track, &vec![b'a'; 257]));
    assert!(!fixture.edit_rename(u32::MAX, b"Ghost"));
    assert_eq!(fixture.name(track), before);
    assert_eq!(fixture.engine().get_undo_count(), history);
}

#[test]
fn the_layout_snapshot_reports_the_renamed_track_from_the_rust_store() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.edit_rename(track, b"Lead Vocal"));

    let json = fixture.engine().project_layout_json();
    assert!(json.contains("Lead Vocal"), "the gathered layout must carry the name");
}

#[test]
fn duplicating_a_track_appends_the_suffix_in_the_copy_own_store() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.edit_rename(track, b"Guitar"));

    let copy = fixture.engine().duplicate_track(track);
    assert_ne!(copy, 0);
    assert_eq!(fixture.name(copy), b"Guitar Copy".to_vec());
    assert_eq!(fixture.name(track), b"Guitar".to_vec());
}

/// The monitor focus decision is Rust's own now: the track has to exist to take
/// the focus, the channel pair has to stay inside the engine's addressing limit,
/// and releasing a monitor is always accepted.
#[test]
fn input_monitor_focus_needs_a_track_that_exists() {
    let fixture = PluginEditEngine::new();
    let track = fixture.audio_track;
    let host = fixture.host();
    let monitor = |id, enabled, left, right| unsafe {
        crate::control_edit_host::hirari_control_edit_set_input_monitor(
            host, id, enabled, left, right,
        )
    };

    assert!(monitor(track, true, 0, 1), "a real track can take the focus");
    assert!(
        !monitor(u32::MAX, true, 0, 1),
        "a track the engine does not have cannot"
    );
    assert!(
        !monitor(track, true, 0, 32),
        "both channels stay inside the engine's limit"
    );
    assert!(
        !monitor(track, true, 1, 1),
        "one input pair means two different channels"
    );
    assert!(monitor(track, false, 0, 1), "releasing the monitor is accepted");
    assert!(monitor(u32::MAX, false, 0, 1), "even for a track that is gone");
}
