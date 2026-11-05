// The automation edit host reads and writes the curve stores a track owns, so
// these tests drive it through a real engine: the tracks lock, the store behind
// it and the engine's undo history are the things under test. Every one of them
// is a slot of the engine's own session bag, which is why nothing here installs
// a fake host.
struct AutomationEditEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: cxx::UniquePtr<crate::ffi::AudioEngine>,
    audio_track: u32,
}

impl AutomationEditEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let reference = engine.as_ref().unwrap();
        let audio_track = reference.add_track(0);
        assert_ne!(audio_track, 0);
        // Track creation is itself reversible, so the history is emptied after
        // the fixture's own edits to give every test an absolute count.
        reference.clear_undo_history();
        assert_eq!(reference.get_undo_count(), 0);
        Self { _guard: guard, engine, audio_track }
    }

    fn engine(&self) -> &crate::ffi::AudioEngine {
        self.engine.as_ref().unwrap()
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.engine())
    }

    fn edit_track(&self, track_id: u32, kind: i32, parameter_id: u32, curve: &[(f64, f64, f64)]) -> bool {
        let packed: Vec<f64> = curve.iter().flat_map(|&(t, v, c)| [t, v, c]).collect();
        self.edit_track_packed(track_id, kind, parameter_id, &packed)
    }

    /// The entry point as the UI calls it: a flat time/value/curve array whose
    /// length the host has to validate.
    fn edit_track_packed(&self, track_id: u32, kind: i32, parameter_id: u32, packed: &[f64]) -> bool {
        unsafe {
            crate::automation_edit_host::hirari_automation_edit_track(
                self.host(),
                track_id,
                kind,
                parameter_id,
                packed.as_ptr(),
                packed.len(),
            )
        }
    }

    fn edit_plugin(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        curve: &[(f64, f64, f64)],
    ) -> bool {
        let packed: Vec<f64> = curve.iter().flat_map(|&(t, v, c)| [t, v, c]).collect();
        unsafe {
            crate::automation_edit_host::hirari_automation_edit_plugin(
                self.host(),
                track_id,
                plugin_index,
                parameter_id,
                packed.as_ptr(),
                packed.len(),
            )
        }
    }

    /// The track's own curve, read back from its store with the engine lock
    /// held the way every edit entry point does.
    fn track_points(&self, track_id: u32, kind: u32) -> Vec<crate::automation_edit_host::HirariAutomationPoint>
    {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        let mut count = 0usize;
        let raw = unsafe {
            crate::track_automation_store::hirari_track_automation_store_points(
                session.automation(),
                kind,
                &mut count,
            )
        };
        if raw.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(raw, count) }.to_vec()
    }

    fn lanes(&self, track_id: u32) -> Vec<(u32, u32)> {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        let mut count = 0usize;
        let views = unsafe {
            crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(
                session.automation(),
                &mut count,
            )
        };
        if views.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(views, count) }
            .iter()
            .map(|lane| (lane.processor_index, lane.parameter_id))
            .collect()
    }

    fn lane_points(&self, track_id: u32, plugin_index: u32, parameter_id: u32) -> Vec<crate::automation_edit_host::HirariAutomationPoint> {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture's track must resolve");
        let mut count = 0usize;
        let views = unsafe {
            crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(
                session.automation(),
                &mut count,
            )
        };
        if views.is_null() {
            return Vec::new();
        }
        let lane = unsafe { std::slice::from_raw_parts(views, count) }
            .iter()
            .find(|lane| lane.processor_index == plugin_index && lane.parameter_id == parameter_id);
        let Some(lane) = lane else { return Vec::new() };
        if lane.points.is_null() || lane.point_count == 0 {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(lane.points, lane.point_count) }.to_vec()
    }

    fn add_plugin(&self, track_id: u32, plugin_type: u32) {
        assert!(unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_add(self.host(), track_id, plugin_type)
        });
    }

    /// The label of the entry on top of the undo stack.
    fn undo_label(&self) -> String {
        let state = self.engine().undo_state() as *const std::ffi::c_void;
        assert!(!state.is_null());
        let length = unsafe {
            crate::undo_transaction::hirari_undo_manager_top_name(state, false, std::ptr::null_mut(), 0)
        };
        let mut buffer = vec![0u8; length];
        let written = unsafe {
            crate::undo_transaction::hirari_undo_manager_top_name(
                state,
                false,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        buffer.truncate(written);
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn curve(&self, index: usize) -> Vec<(f64, f64, f64)> {
        vec![(index as f64 * 10.0 + 20.0, 0.75, 0.5), (index as f64 * 10.0 + 30.0, 1.0, -0.25)]
    }

    /// A curve as the store holds it after the host's f64-to-f32 narrowing.
    fn points(curve: &[(f64, f64, f64)]) -> Vec<crate::automation_edit_host::HirariAutomationPoint>
    {
        curve
            .iter()
            .map(|&(time, value, curve)| crate::automation_edit_host::HirariAutomationPoint {
                time,
                value: value as f32,
                curve: curve as f32,
            })
            .collect()
    }
}

#[test]
fn a_volume_edit_replaces_the_curve_and_undoes_back_to_the_previous_one() {
    let fixture = AutomationEditEngine::new();
    assert!(fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &fixture.curve(0)));
    assert_eq!(fixture.undo_label(), format!("Volume Automation Track {}", fixture.audio_track));
    assert!(fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &fixture.curve(1)));
    assert_eq!(fixture.engine().get_undo_count(), 2);
    assert_eq!(
        fixture.track_points(fixture.audio_track, 0),
        AutomationEditEngine::points(&fixture.curve(1))
    );
    // Undo walks the two captures back: the first edit's curve, then the empty
    // store the track started with.
    fixture.engine().undo();
    assert_eq!(
        fixture.track_points(fixture.audio_track, 0),
        AutomationEditEngine::points(&fixture.curve(0))
    );
    fixture.engine().undo();
    assert!(fixture.track_points(fixture.audio_track, 0).is_empty());
    fixture.engine().redo();
    fixture.engine().redo();
    assert_eq!(
        fixture.track_points(fixture.audio_track, 0),
        AutomationEditEngine::points(&fixture.curve(1))
    );
}

#[test]
fn pan_and_delay_edits_label_their_own_history_entry() {
    let fixture = AutomationEditEngine::new();
    for (kind, store_kind, label) in [
        (crate::automation_edit_host::HIRARI_AUTOMATION_PAN, 1, "Pan Automation"),
        (crate::automation_edit_host::HIRARI_AUTOMATION_TRACK_DELAY, 2, "Track Delay Automation"),
    ] {
        assert!(fixture.edit_track(fixture.audio_track, kind, 0, &fixture.curve(0)));
        assert_eq!(
            fixture.undo_label(),
            if kind == crate::automation_edit_host::HIRARI_AUTOMATION_TRACK_DELAY {
                format!("{label} {}", fixture.audio_track)
            } else {
                format!("{label} Track {}", fixture.audio_track)
            }
        );
        // The curve lands in the store the audio callback reads for that kind.
        assert_eq!(fixture.track_points(fixture.audio_track, store_kind).len(), 2);
    }
    // A parameter id outside the volume and pan range is not a track curve.
    assert!(!fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_PAN, 7, &fixture.curve(0)));
}

#[test]
fn a_plugin_edit_replaces_only_the_matching_lane() {
    let fixture = AutomationEditEngine::new();
    fixture.add_plugin(fixture.audio_track, 1);
    assert!(fixture.edit_plugin(fixture.audio_track, 0, 0, &fixture.curve(0)));
    assert!(fixture.edit_plugin(fixture.audio_track, 0, 1, &fixture.curve(1)));
    assert_eq!(fixture.lanes(fixture.audio_track), vec![(0, 0), (0, 1)]);
    assert!(fixture.edit_plugin(fixture.audio_track, 0, 0, &[(90.0, 0.5, 0.0)]));
    assert_eq!(fixture.lanes(fixture.audio_track), vec![(0, 1), (0, 0)]);
    assert_eq!(fixture.lane_points(fixture.audio_track, 0, 0).len(), 1);
    assert_eq!(fixture.lane_points(fixture.audio_track, 0, 1).len(), 2);
    assert_eq!(
        fixture.undo_label(),
        format!("Plugin Parameter Automation Track {}", fixture.audio_track)
    );
}

#[test]
fn an_empty_plugin_curve_removes_the_lane() {
    let fixture = AutomationEditEngine::new();
    fixture.add_plugin(fixture.audio_track, 1);
    assert!(fixture.edit_plugin(fixture.audio_track, 0, 0, &fixture.curve(0)));
    assert_eq!(fixture.lanes(fixture.audio_track), vec![(0, 0)]);
    assert!(fixture.edit_plugin(fixture.audio_track, 0, 0, &[]));
    assert!(fixture.lanes(fixture.audio_track).is_empty());
}

#[test]
fn an_unknown_track_or_plugin_slot_is_rejected_without_an_undo_entry() {
    let fixture = AutomationEditEngine::new();
    let curve = fixture.curve(0);
    assert!(!fixture.edit_track(u32::MAX, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &curve));
    assert!(!fixture.edit_plugin(u32::MAX, 0, 0, &curve));
    // The track exists but has no insert at index zero.
    assert!(!fixture.edit_plugin(fixture.audio_track, 0, 0, &curve));
    assert_eq!(fixture.engine().get_undo_count(), 0);
    assert!(fixture.track_points(fixture.audio_track, 0).is_empty());
}

#[test]
fn a_malformed_or_oversized_curve_is_refused() {
    let fixture = AutomationEditEngine::new();
    for curve in [
        vec![(f64::NAN, 0.5, 0.0)],
        vec![(10.0, 2.0, 0.0)],
        vec![(10.0, 0.5, 4.0)],
        vec![(20.0, 0.5, 0.0), (10.0, 0.5, 0.0)],
        vec![(10.0, 0.5, 0.0), (10.0, 0.6, 0.0)],
    ] {
        assert!(!fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &curve));
        assert!(fixture.track_points(fixture.audio_track, 0).is_empty());
    }
    // A flat array that is not a whole number of points cannot be a curve.
    assert!(!fixture.edit_track_packed(
        fixture.audio_track,
        crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME,
        0,
        &[10.0, 0.5, 0.0, 20.0]
    ));
    assert!(fixture.track_points(fixture.audio_track, 0).is_empty());
    // One more delay point than the engine ever accepted (8192) is refused
    // outright.
    let oversized = vec![(1.0, 0.5, 0.0); 8192 * 3 + 1];
    assert!(!fixture.edit_track(
        fixture.audio_track,
        crate::automation_edit_host::HIRARI_AUTOMATION_TRACK_DELAY,
        0,
        &oversized
    ));
    // A plugin lane sample index past exact integer precision is refused too.
    let unbounded = vec![(f64::MAX, 0.5, 0.0)];
    fixture.add_plugin(fixture.audio_track, 1);
    let after_add = fixture.engine().get_undo_count();
    assert!(!fixture.edit_plugin(fixture.audio_track, 0, 0, &unbounded));
    assert_eq!(fixture.engine().get_undo_count(), after_add);
}

#[test]
fn one_edit_records_exactly_one_history_entry() {
    let fixture = AutomationEditEngine::new();
    let before = fixture.engine().get_undo_count();
    assert!(fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &fixture.curve(0)));
    assert_eq!(fixture.engine().get_undo_count(), before + 1);
    assert!(fixture.edit_track(fixture.audio_track, crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME, 0, &fixture.curve(0)));
    assert_eq!(fixture.engine().get_undo_count(), before + 2);
    fixture.engine().undo();
    assert_eq!(
        fixture.track_points(fixture.audio_track, 0),
        AutomationEditEngine::points(&fixture.curve(0))
    );
}
