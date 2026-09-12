impl AuraCore {
    pub fn new() -> anyhow::Result<Self> {
        // Unit tests must never race the machine's physical audio device.
        // Relying only on an environment variable is order-dependent: a test
        // can construct AuraCore before the shared test guard has a chance to
        // set it.  Compile-time test isolation makes bare `cargo test` safe as
        // well as the CMake/CI entry points that set the variable explicitly.
        let isolated =
            cfg!(test) || std::env::var("AURA_NATIVE_TEST_ISOLATION").as_deref() == Ok("1");
        Self::new_with_mode(isolated)
    }

    /// Construct the Core without opening a physical audio device.
    ///
    /// CLI, server, test, and automation clients should use this entry point
    /// so headless operation never depends on machine audio configuration.
    pub fn new_offline() -> anyhow::Result<Self> {
        Self::new_with_mode(true)
    }

    fn new_with_mode(isolated: bool) -> anyhow::Result<Self> {
        let engine = if isolated {
            ffi::new_audio_engine_offline()
        } else {
            ffi::new_audio_engine()
        };
        if engine.is_null() {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: Failed to initialize AudioEngine (nullptr)"
            ));
        }

        // Explicitly initialize GPU before start (Point 2)
        let gpu_initialized = ffi::initialize_gpu_with_status();
        if !gpu_initialized {
            report_aura_log(
                1,
                "GPU initialization failed; continuing with CPU fallbacks.",
            );
        }

        let Some(engine_ref) = engine.as_ref() else {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: AudioEngine handle is invalid"
            ));
        };
        if isolated {
            engine_ref.new_project();
            let _ = engine_ref.remove_track(0);
        }
        // Engine bootstrap/template setup is not a user edit and must not
        // appear as the first undoable action of a new Core instance.
        engine_ref.clear_undo_history();
        let analysis = ffi::new_analysis_hub(engine_ref);
        if analysis.is_null() {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: Failed to initialize AnalysisHub (nullptr)"
            ));
        }

        Ok(Self {
            engine,
            analysis,
            gpu_initialized,
            preview_audio: std::sync::Mutex::new(preview_audio_runtime::PreviewAudioRuntime::new()),
            recording_session: std::sync::Mutex::new(None),
            comping: std::sync::Mutex::new(comping::CompingOrchestrator::new()),
            aux_track_ids: std::sync::Mutex::new(HashSet::new()),
            midi_events: std::sync::Mutex::new(Vec::new()),
            scheduled_midi_notes: std::sync::Mutex::new(Vec::new()),
            midi_vibrato_rates: std::sync::Mutex::new(HashMap::new()),
            midi_monitor: std::sync::Mutex::new(crate::midi_monitor::MidiMonitor::new()),
            review_notes: std::sync::Mutex::new(crate::review_notes::ReviewNoteStore::new()),
            control_room: std::sync::Mutex::new(crate::control_room::ControlRoomState::default()),
            chord_track: std::sync::Mutex::new(Vec::new()),
            midi_lyric_history: std::sync::Mutex::new(Vec::new()),
            chord_history: std::sync::Mutex::new(Vec::new()),
            chord_redo_history: std::sync::Mutex::new(Vec::new()),
            comping_history: std::sync::Mutex::new(Vec::new()),
            comping_redo_history: std::sync::Mutex::new(Vec::new()),
            plugin_parameter_events: std::sync::Mutex::new(Vec::with_capacity(64)),
            project_transaction: std::sync::Mutex::new(()),
            openutau_vocals: std::sync::Mutex::new(Vec::new()),
            track_stacks: std::sync::Mutex::new(Vec::new()),
            track_stack_base_volumes: std::sync::Mutex::new(HashMap::new()),
            markers: std::sync::Mutex::new(vec![
                MarkerContract {
                    id: 1,
                    label: "START".to_owned(),
                    beat: 0.0,
                    color: "#646496".to_owned(),
                },
                MarkerContract {
                    id: 2,
                    label: "DEVELOPMENT".to_owned(),
                    beat: 32.0,
                    color: "#966464".to_owned(),
                },
            ]),
            macro_mappings: std::sync::Mutex::new(Vec::new()),
            midi_learn_mappings: std::sync::Mutex::new(Vec::new()),
            mix_snapshots: std::sync::Mutex::new(crate::snapshots::SnapshotOrchestrator::new()),
            midi_pickup_acquired: std::sync::Mutex::new(HashSet::new()),
            production_events: std::sync::Arc::new(std::sync::Mutex::new(
                crate::production_events::EventHub::default(),
            )),
            external_sync: std::sync::Mutex::new(
                crate::sync_transport::ExternalSyncController::default(),
            ),
            export_queue: std::sync::Mutex::new(crate::export::ExportOrchestrator::new()),
            advanced_export: std::sync::Mutex::new(crate::advanced_export_engine::ExportOrchestrator::new()),
            ara2_protocol: std::sync::Mutex::new(crate::ara2_protocol::Ara2ProtocolEndpoint::default()),
        })
    }

    pub(crate) fn production_event_hub(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::production_events::EventHub>> {
        self.production_events.clone()
    }

    pub(crate) fn publish_production_event(
        &self,
        event: crate::production_events::ProductionEvent,
    ) {
        if let Ok(mut hub) = self.production_events.lock() {
            let _ = hub.publish(self.project_generation(), event);
        }
    }

    /// Returns a lifetime-bound handle to the analysis hub.
    /// This enforces that the hub cannot be used if the core is mutated or dropped.
    pub fn analysis(&self) -> Option<&ffi::AnalysisHub> {
        self.analysis.as_ref()
    }

    pub fn gpu_initialized(&self) -> bool {
        self.gpu_initialized
    }

    pub fn audio_config_generation(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_audio_config_generation())
    }

    /// Stable diagnostic envelope for UI, CLI, and future FFI callers.
    /// This keeps driver failure details and generation context together.
    pub fn audio_driver_diagnostic_json(&self) -> String {
        let reported_status = self.audio_driver_status();
        // The host's lifecycle state can briefly remain `running` while the
        // underlying callback has already stopped. Diagnostics must describe
        // the observable device state, not only the control-side state flag.
        let callback_ready = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.is_audio_device_ready());
        let status = if reported_status == "running" && !callback_ready {
            "stopped".to_owned()
        } else {
            reported_status
        };
        let error = match status.as_str() {
            "initialized" | "running" => None,
            "start-failed" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_start_failed",
                    "Audio device failed to start",
                )
                .retryable(true),
            ),
            "silent-fallback" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_silent_fallback",
                    "Audio is running in silent fallback mode",
                )
                .retryable(true),
            ),
            "stopped" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_stopped",
                    "Audio device is stopped",
                )
                .retryable(true),
            ),
            _ => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_unavailable",
                    "Audio device is unavailable",
                )
                .retryable(true),
            ),
        };
        serde_json::json!({
            "ok": error.is_none() && callback_ready,
            "status": status,
            "error_code": self
                .engine
                .as_ref()
                .map_or(0, |engine| engine.audio_driver_error_code()),
            "error": error,
            "project_generation": self.project_generation(),
            "audio_generation": self.audio_config_generation(),
        })
        .to_string()
    }
}
