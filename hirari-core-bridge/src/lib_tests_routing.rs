// The routing edit host reads and writes track state through the real
// TrackSession, so these tests drive a live engine: the session table, the
// tracks' own Rust stores and the engine's undo history are the ones under
// test. Host-op tables are process-global, which is why nothing here swaps
// them for fakes.
struct RoutingEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: Box<crate::AudioEngine>,
    audio_track: u32,
    second_audio: u32,
    bus_track: u32,
    missing_track: u32,
}

impl RoutingEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::new_audio_engine();
        assert!(!false);
        let reference = &*engine;
        let audio_track = reference.add_track(0);
        let second_audio = reference.add_track(0);
        let bus_track = reference.add_track(3);
        assert_ne!(audio_track, 0);
        assert_ne!(second_audio, 0);
        assert_ne!(bus_track, 0);
        // Track creation is itself reversible, so the history is emptied after
        // the fixture's own edits to give every test an absolute count.
        reference.clear_undo_history();
        assert_eq!(reference.get_undo_count(), 0);
        let missing_track = bus_track + 40;
        assert!(missing_track < 128);
        Self {
            _guard: guard,
            engine,
            audio_track,
            second_audio,
            bus_track,
            missing_track,
        }
    }

    fn engine(&self) -> &crate::AudioEngine {
        &*self.engine
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.engine())
    }

    /// Reads one track's routing state through the same session path the
    /// edit host writes it through.
    fn state(&self, track_id: u32) -> crate::routing_edit_host::HirariRoutingTrackState {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        let (strip, runtime) = (session.strip(), session.runtime());
        crate::routing_edit_host::HirariRoutingTrackState {
            armed: unsafe {
                crate::channel_strip::hirari_channel_strip_is_record_armed(strip)
            },
            phase_inverted: unsafe {
                crate::track_runtime::hirari_track_runtime_is_phase_inverted(runtime)
            },
            is_bus: session.is_bus(),
            delay_samples: unsafe {
                crate::track_runtime::hirari_track_runtime_track_delay_samples(runtime)
            },
            volume: unsafe { crate::channel_strip::hirari_channel_strip_gain(strip) },
            pan: unsafe { crate::channel_strip::hirari_channel_strip_pan(strip) },
            muted: unsafe { crate::track_runtime::hirari_track_runtime_is_muted(runtime) },
            solo: unsafe { crate::channel_strip::hirari_channel_strip_is_solo(strip) },
        }
    }

    fn set_arm(&self, track_id: u32, armed: bool) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_track_armed(self.host(), track_id, armed) }
    }

    fn set_phase(&self, track_id: u32, inverted: bool) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_phase_invert(self.host(), track_id, inverted) }
    }

    fn set_delay(&self, track_id: u32, samples: u32) -> bool {
        unsafe {
            crate::routing_edit_host::hirari_routing_edit_set_track_delay_samples(
                self.host(),
                track_id,
                samples,
            )
        }
    }

    fn set_volume(&self, track_id: u32, value: f32) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_track_volume(self.host(), track_id, value) }
    }

    fn set_pan(&self, track_id: u32, value: f32) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_track_pan(self.host(), track_id, value) }
    }

    fn set_mute(&self, track_id: u32, muted: bool) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_track_mute(self.host(), track_id, muted) }
    }

    fn set_solo(&self, track_id: u32, solo: bool) -> bool {
        unsafe { crate::routing_edit_host::hirari_routing_edit_set_track_solo(self.host(), track_id, solo) }
    }

    fn set_send(&self, source: u32, dest: u32, gain: f32, pre_fader: bool) -> bool {
        unsafe {
            crate::routing_edit_host::hirari_routing_edit_set_send_route(
                self.host(),
                source,
                dest,
                gain,
                pre_fader,
                true,
            )
        }
    }

    fn set_sidechain(&self, source: u32, dest: u32, plugin: u32, tap: u32, enabled: bool) -> bool {
        unsafe {
            crate::routing_edit_host::hirari_routing_edit_set_sidechain_link(
                self.host(),
                source,
                dest,
                plugin,
                tap,
                enabled,
            )
        }
    }
}

#[test]
fn record_arm_and_phase_record_only_real_changes() {
    let fixture = RoutingEngine::new();
    let track = fixture.audio_track;
    // Already disarmed and un-inverted: valid no-ops without an undo entry.
    assert!(fixture.set_arm(track, false));
    assert!(fixture.set_phase(track, false));
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(fixture.set_arm(track, true));
    assert_eq!(fixture.engine().get_undo_count(), 1);
    assert!(fixture.set_arm(track, true));
    assert_eq!(fixture.engine().get_undo_count(), 1);
    assert!(fixture.set_phase(track, true));
    assert_eq!(fixture.engine().get_undo_count(), 2);
    let state = fixture.state(track);
    assert!(state.armed);
    assert!(state.phase_inverted);
    // The engine history replays both entries through the same Rust policy.
    fixture.engine().undo();
    assert!(fixture.state(track).armed);
    assert!(!fixture.state(track).phase_inverted);
    fixture.engine().undo();
    assert!(!fixture.state(track).armed);
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert_eq!(fixture.engine().get_redo_count(), 2);
    fixture.engine().redo();
    assert!(fixture.state(track).armed);
    fixture.engine().redo();
    assert!(fixture.state(track).phase_inverted);
}

#[test]
fn send_route_requires_bus_destination_and_existing_tracks() {
    let fixture = RoutingEngine::new();
    // Track-to-track edges are inserts, not sends.
    assert!(!fixture.set_send(fixture.audio_track, fixture.second_audio, 1.0, false));
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(fixture.set_send(fixture.audio_track, fixture.bus_track, 1.0, false));
    assert_eq!(fixture.engine().get_undo_count(), 1);
    // A missing source is rejected even now that the destination is a bus.
    assert!(!fixture.set_send(fixture.missing_track, fixture.bus_track, 1.0, false));
    assert_eq!(fixture.engine().get_undo_count(), 1);
    fixture.engine().undo();
    assert_eq!(fixture.engine().get_undo_count(), 0);
}

#[test]
fn mix_edits_apply_and_restore_through_the_engine_history() {
    let fixture = RoutingEngine::new();
    let track = fixture.audio_track;
    let before = fixture.state(track);
    assert!(fixture.set_volume(track, 0.5));
    assert!(fixture.set_pan(track, 0.75));
    let after = fixture.state(track);
    assert_eq!(after.volume, 0.5);
    assert_eq!(after.pan, 0.75);
    assert_eq!(fixture.engine().get_undo_count(), 2);
    fixture.engine().undo();
    fixture.engine().undo();
    let restored = fixture.state(track);
    assert_eq!(restored.volume, before.volume);
    assert_eq!(restored.pan, before.pan);
    assert_eq!(fixture.engine().get_undo_count(), 0);
}

#[test]
fn mix_ranges_reject_out_of_domain_values() {
    let fixture = RoutingEngine::new();
    let track = fixture.audio_track;
    assert!(!fixture.set_volume(track, 2.5));
    assert!(!fixture.set_volume(track, -0.1));
    assert!(!fixture.set_volume(track, f32::NAN));
    assert!(fixture.set_volume(track, 2.0));
    assert!(!fixture.set_pan(track, 1.5));
    assert!(!fixture.set_pan(track, f32::INFINITY));
    assert!(fixture.set_pan(track, -1.0));
    // An unknown track resolves to no session, so nothing applies.
    assert!(!fixture.set_mute(fixture.missing_track, true));
    assert_eq!(fixture.engine().get_undo_count(), 2);
    assert_eq!(fixture.state(track).volume, 2.0);
    assert_eq!(fixture.state(track).pan, -1.0);
}

#[test]
fn mute_and_solo_record_unchanged_values_like_the_previous_engine() {
    let fixture = RoutingEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.set_mute(track, false));
    assert!(fixture.set_solo(track, true));
    assert!(fixture.set_solo(track, true));
    assert_eq!(fixture.engine().get_undo_count(), 3);
    let state = fixture.state(track);
    assert!(!state.muted);
    assert!(state.solo);
    // The second solo entry recorded the already-solo value, so undoing it
    // keeps the track soloed; the first entry is the real transition back.
    fixture.engine().undo();
    assert!(fixture.state(track).solo);
    fixture.engine().undo();
    assert!(!fixture.state(track).solo);
}

#[test]
fn delay_edits_bound_to_the_shared_delay_line_and_undo() {
    let fixture = RoutingEngine::new();
    let track = fixture.audio_track;
    assert!(fixture.set_delay(track, 128));
    assert_eq!(fixture.state(track).delay_samples, 128);
    assert_eq!(fixture.engine().get_undo_count(), 1);
    assert!(!fixture.set_delay(track, 8193));
    assert_eq!(fixture.state(track).delay_samples, 128);
    assert_eq!(fixture.engine().get_undo_count(), 1);
    fixture.engine().undo();
    assert_eq!(fixture.state(track).delay_samples, 0);
}

#[test]
fn sidechain_links_register_and_remove_through_the_rust_fallback() {
    let fixture = RoutingEngine::new();
    // The engine installs no sidechain context or registration callback, so
    // every path below takes the Rust fallback: the device format from the
    // engine block state and the source's work buffer through the session.
    assert!(!fixture.set_sidechain(fixture.missing_track, fixture.second_audio, 0, 0, true));
    assert!(!fixture.set_sidechain(fixture.audio_track, fixture.missing_track, 0, 0, true));
    assert!(!fixture.set_sidechain(fixture.audio_track, fixture.audio_track, 0, 0, true));
    assert!(!fixture.set_sidechain(fixture.audio_track, fixture.second_audio, 0, 3, true));
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(fixture.set_sidechain(fixture.audio_track, fixture.second_audio, 0, 0, true));
    assert_eq!(fixture.engine().get_undo_count(), 1);
    // Undo replays the disable payload; the link is gone from both stores,
    // so a second removal has nothing left to delete.
    fixture.engine().undo();
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(!fixture.set_sidechain(fixture.audio_track, fixture.second_audio, 0, 0, false));
    assert_eq!(fixture.engine().get_undo_count(), 0);
    // Redo re-registers through the same fallback, and now removal succeeds.
    fixture.engine().redo();
    assert!(fixture.set_sidechain(fixture.audio_track, fixture.second_audio, 0, 0, false));
}
