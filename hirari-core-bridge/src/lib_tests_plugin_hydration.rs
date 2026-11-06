// Project-load plugin hydration. The document stores a track's inserts
// positionally and its curves per kind, so these tests hold the two shapes the
// loader has to rebuild: a real save/load round trip through the engine, and a
// direct call with a hand-decoded document onto a live track's own stores. The
// rejection rules, sandbox-slot handling and positional bounds are properties of
// the real chain and automation stores, not of a fake.
static HYDRATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct PluginHydrationEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: Box<crate::AudioEngine>,
    path: String,
    audio_track: u32,
}

impl PluginHydrationEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::new_audio_engine();
        assert!(!false);
        let reference = &*engine;
        let audio_track = reference.add_track(0);
        assert_ne!(audio_track, 0);
        reference.clear_undo_history();
        let unique = HYDRATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!(
                "hirari-plugin-hydration-{}-{}.hirari",
                std::process::id(),
                unique
            ))
            .to_string_lossy()
            .into_owned();
        Self { _guard: guard, engine, path, audio_track }
    }

    fn engine(&self) -> &crate::AudioEngine {
        &*self.engine
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.engine())
    }

    fn add_plugin(&self, plugin_type: u32) -> bool {
        unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_add(self.host(), self.audio_track, plugin_type)
        }
    }

    fn edit_volume(&self, curve: &[(f64, f64, f64)]) -> bool {
        let packed: Vec<f64> = curve.iter().flat_map(|&(t, v, c)| [t, v, c]).collect();
        unsafe {
            crate::automation_edit_host::hirari_automation_edit_track(
                self.host(),
                self.audio_track,
                crate::automation_edit_host::HIRARI_AUTOMATION_VOLUME,
                0,
                packed.as_ptr(),
                packed.len(),
            )
        }
    }

    fn edit_plugin_lane(&self, plugin_index: u32, parameter_id: u32, curve: &[(f64, f64, f64)]) -> bool {
        let packed: Vec<f64> = curve.iter().flat_map(|&(t, v, c)| [t, v, c]).collect();
        unsafe {
            crate::automation_edit_host::hirari_automation_edit_plugin(
                self.host(),
                self.audio_track,
                plugin_index,
                parameter_id,
                packed.as_ptr(),
                packed.len(),
            )
        }
    }

    /// A document entry for the fixture's own track: the loader sees the id and
    /// type the saved file carries, everything else is what a test sets.
    fn document(&self) -> crate::ffi::NativeTrackState {
        crate::ffi::NativeTrackState {
            id: self.audio_track,
            ..Default::default()
        }
    }

    /// The track's own store handles, read while the engine lock is held and kept
    /// after the session drops — the same handles the loader hands Rust.
    fn storages(&self, track_id: u32) -> crate::ffi::NativeTrackStorages {
        let session = crate::track_session::TrackSession::open(self.host(), track_id)
            .expect("the fixture track must resolve");
        crate::ffi::NativeTrackStorages {
            id: track_id,
            trackType: if session.is_bus() { 3 } else { 0 },
            chain: session.chain() as usize,
            automation: session.automation() as usize,
        }
    }

    fn hydrate(
        &self,
        documents: Vec<crate::ffi::NativeTrackState>,
        storages: Vec<crate::ffi::NativeTrackStorages>,
    ) -> bool {
        let state = crate::ffi::NativeProjectState {
            tracks: documents,
            ..Default::default()
        };
        crate::project_plugin_hydration::native_project_hydrate_plugins(&state, &storages)
    }

    fn track_json(&self) -> serde_json::Value {
        let layout: serde_json::Value = serde_json::from_str(&self.engine().project_layout_json())
            .expect("native layout must be valid JSON");
        layout
            .as_array()
            .expect("layout must be a track array")
            .iter()
            .find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(self.audio_track as u64)
            })
            .cloned()
            .expect("the fixture track must be listed")
    }

    fn plugin_types(&self) -> Vec<u64> {
        self.numbers("plugin_types")
    }

    fn numbers(&self, field: &str) -> Vec<u64> {
        self.track_json()
            .get(field)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(serde_json::Value::as_u64)
            .collect()
    }

    fn plugin_bypass(&self) -> Vec<bool> {
        self.track_json()
            .get("plugin_bypass")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(serde_json::Value::as_bool)
            .collect()
    }

    fn volume_points(&self) -> Vec<(f64, f64, f64)> {
        self.track_json()
            .get("volume_automation")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|point| {
                (
                    point.get("time").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    point.get("value").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    point.get("curve").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                )
            })
            .collect()
    }

    fn plugin_lanes(&self) -> Vec<(u64, u64, usize)> {
        self.track_json()
            .get("plugin_automation")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|lane| {
                (
                    lane.get("plugin_index").and_then(serde_json::Value::as_u64).unwrap_or(0),
                    lane.get("parameter_id").and_then(serde_json::Value::as_u64).unwrap_or(0),
                    lane.get("points")
                        .and_then(serde_json::Value::as_array)
                        .map(|points| points.len())
                        .unwrap_or(0),
                )
            })
            .collect()
    }

    fn sandbox_paths(&self) -> Vec<String> {
        self.track_json()
            .get("sandbox_plugin_paths")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(String::from)
            .collect()
    }
}

impl Drop for PluginHydrationEngine {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn curve_points(curve: &[(f64, f64, f64)]) -> Vec<crate::ffi::NativeAutomationPoint> {
    curve
        .iter()
        .map(|&(time, value, shape)| crate::ffi::NativeAutomationPoint {
            time,
            value: value as f32,
            curve: shape as f32,
        })
        .collect()
}

fn blob(bytes: &[u8]) -> crate::ffi::NativeBlob {
    crate::ffi::NativeBlob { bytes: bytes.to_vec() }
}

#[test]
fn a_saved_chain_curves_and_bypass_reload_through_the_engine() {
    let fixture = PluginHydrationEngine::new();
    assert!(fixture.add_plugin(1));
    assert!(fixture.add_plugin(2));
    assert!(fixture.engine().set_plugin_bypass(fixture.audio_track, 1, true));
    assert!(fixture.edit_volume(&[(4.0, 0.25, 0.5), (8.0, 1.0, -0.25)]));
    assert!(fixture.edit_plugin_lane(0, 0, &[(2.0, 0.5, 0.0), (6.0, 0.75, 0.25)]));
    assert!(fixture.engine().save_project(&fixture.path));

    // Loading tears the graph down and installs rebuilt tracks, so everything
    // asserted below comes out of the hydration walk rather than the live edits.
    assert!(fixture.engine().load_project(&fixture.path));
    assert_eq!(fixture.plugin_types(), vec![1, 2]);
    assert_eq!(fixture.plugin_bypass(), vec![false, true]);
    assert_eq!(
        fixture.volume_points(),
        vec![(4.0, 0.25, 0.5), (8.0, 1.0, -0.25)]
    );
    assert_eq!(fixture.plugin_lanes(), vec![(0, 0, 2)]);
}

#[test]
fn a_builtin_the_engine_cannot_create_stops_the_hydration() {
    let fixture = PluginHydrationEngine::new();
    let storages = fixture.storages(fixture.audio_track);
    let mut document = fixture.document();
    document.pluginTypes = vec![1, 9_999];
    assert!(!fixture.hydrate(vec![document], vec![storages]));
    // The walk aborts at the unknown type, so the loader discards the whole
    // replacement graph. The inserts already built stay on the provisional track
    // but never reach the published one.
    assert_eq!(fixture.plugin_types(), vec![1]);
}

#[test]
fn a_sandbox_marker_consumes_a_path_and_keeps_the_walk_going() {
    let fixture = PluginHydrationEngine::new();
    let storages = fixture.storages(fixture.audio_track);
    let mut document = fixture.document();
    document.pluginTypes = vec![1, u32::MAX, 2];
    document.sandboxedPluginPaths = vec!["/nonexistent/hirari-missing-plugin.clap".to_string()];
    document.pluginBypass = vec![0, 1, 0];
    assert!(fixture.hydrate(vec![document], vec![storages]));
    // A binary the engine cannot open publishes no slot, so only the two
    // builtins reach the chain. The marker still consumed exactly one path,
    // which is why the insert after it keeps its own saved type.
    assert_eq!(fixture.plugin_types(), vec![1, 2]);
    assert!(fixture.sandbox_paths().is_empty());
    // The positional arrays address live slots, so the sandbox slot's bypass
    // entry lands on the next builtin. The saved state stays in the document and
    // only becomes reachable once the plugin is relinked.
    assert_eq!(fixture.plugin_bypass(), vec![false, true]);
}

#[test]
fn a_sandbox_path_without_a_marker_is_appended_after_the_document_chain() {
    let fixture = PluginHydrationEngine::new();
    let storages = fixture.storages(fixture.audio_track);
    let mut document = fixture.document();
    document.pluginTypes = vec![1, u32::MAX];
    document.sandboxedPluginPaths = vec![
        "/nonexistent/hirari-marker-plugin.clap".to_string(),
        "/nonexistent/hirari-orphan-plugin.clap".to_string(),
    ];
    assert!(fixture.hydrate(vec![document], vec![storages]));
    // Files written before the marker existed can list more paths than the
    // plugin array has markers; the marker takes the first and the remainder is
    // still offered to the chain instead of being dropped. Neither binary is
    // present, so the live chain keeps only the builtin.
    assert_eq!(fixture.plugin_types(), vec![1]);
    assert!(fixture.sandbox_paths().is_empty());
}

#[test]
fn an_empty_document_curve_leaves_the_stored_curve_alone() {
    let fixture = PluginHydrationEngine::new();
    assert!(fixture.edit_volume(&[(1.0, 0.5, 0.0)]));
    let storages = fixture.storages(fixture.audio_track);
    assert!(fixture.hydrate(vec![fixture.document()], vec![storages]));
    assert_eq!(fixture.volume_points(), vec![(1.0, 0.5, 0.0)]);

    let mut document = fixture.document();
    document.volumeAutomation = curve_points(&[(2.0, 0.75, 0.25)]);
    assert!(fixture.hydrate(vec![document], vec![fixture.storages(fixture.audio_track)]));
    assert_eq!(fixture.volume_points(), vec![(2.0, 0.75, 0.25)]);
}

#[test]
fn per_insert_state_stops_at_the_live_chain_length() {
    let fixture = PluginHydrationEngine::new();
    let storages = fixture.storages(fixture.audio_track);
    let mut document = fixture.document();
    document.pluginTypes = vec![1];
    // The document carries state for inserts the engine did not rebuild: entries
    // past the live chain are skipped rather than reaching the one real slot, and
    // a state blob the plugin cannot read leaves it running.
    document.pluginStates = vec![blob(b"not-a-state"), blob(b""), blob(b"also-not")];
    document.pluginGuiStates = vec![blob(b"not-a-gui-state"); 3];
    document.pluginBypass = vec![0, 1, 0];
    assert!(fixture.hydrate(vec![document], vec![storages]));
    assert_eq!(fixture.plugin_types(), vec![1]);
    assert_eq!(fixture.plugin_bypass(), vec![false]);
}

#[test]
fn a_document_track_without_a_provisional_storage_is_skipped() {
    let fixture = PluginHydrationEngine::new();
    let storages = fixture.storages(fixture.audio_track);
    let mut document = fixture.document();
    document.id = 999_999;
    document.pluginTypes = vec![1];
    assert!(fixture.hydrate(vec![document], vec![storages]));
    assert!(fixture.plugin_types().is_empty());
}

#[test]
fn a_track_without_its_stores_fails_the_hydration() {
    let fixture = PluginHydrationEngine::new();
    let document = fixture.document();
    let storages = vec![crate::ffi::NativeTrackStorages {
        id: document.id,
        trackType: 0,
        chain: 0,
        automation: 0,
    }];
    assert!(!fixture.hydrate(vec![document], storages));
}
