impl HirariCore {
    /// Address of the Control Room runtime the native audio callback renders with.
    ///
    /// The object is implemented in Rust (`control_room_runtime.rs`) but is
    /// created and destroyed by the engine's `ControlRoom` member, so its
    /// lifetime is exactly `engine`'s and `&self` keeps it alive for every call
    /// below. A zero address means the engine handle was never created; each
    /// `hirari_control_room_*` entry point treats a null handle the way the
    /// deleted null-`m_engine` C++ branches did (false for mutators, `-20.0` /
    /// `0` / `0.0` for the getter defaults), so behaviour matches either way.
    ///
    /// Everything below calls the very functions the C++ forwarding shims used
    /// to invoke -- only the route is shorter. Two operations still consult the
    /// engine for facts Rust does not own: `bus_track_exists`, which answers
    /// against the native track list where a cue's bus target must resolve.
    #[inline]
    fn control_room_handle(&self) -> *mut std::ffi::c_void {
        self.control_room_state as *mut std::ffi::c_void
    }

    pub fn audio_output_channel_count(&self) -> u32 {
        self.engine.as_ref().map_or(0, |engine| engine.get_audio_output_channel_count())
    }

    pub fn add_control_room_speaker(&self, name: &str, gain: f32) -> bool {
        !name.trim().is_empty()
            && name.len() <= 128
            && gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && unsafe {
                crate::control_room_runtime::hirari_control_room_add_speaker(
                    self.control_room_handle(),
                    name.as_ptr(),
                    name.len(),
                    gain,
                )
            }
            && self.control_room.lock().map(|mut state| {
                let added = state.add_monitor_output(name);
                let index = state.monitor_outputs.len().saturating_sub(1);
                if added { let _ = state.set_output_gain(index, gain); }
                added
            }).unwrap_or(false)
    }
    pub fn select_control_room_speaker(&self, index: u32) -> bool {
        let native = unsafe {
            crate::control_room_runtime::hirari_control_room_select_speaker(
                self.control_room_handle(),
                index as usize,
            )
        };
        let model = self.control_room.lock().map(|mut state| state.select_output(index as usize)).unwrap_or(false);
        native && model
    }
    pub fn remove_control_room_speaker(&self, index: u32) -> bool {
        let native = unsafe {
            crate::control_room_runtime::hirari_control_room_remove_speaker(
                self.control_room_handle(),
                index as usize,
            )
        };
        let model = self.control_room.lock()
            .map(|mut state| state.remove_monitor_output(index as usize))
            .unwrap_or(false);
        native && model
    }
    pub fn set_control_room_speaker_gain(&self, index: u32, gain: f32) -> bool {
        gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && unsafe {
                crate::control_room_runtime::hirari_control_room_set_speaker_gain(
                    self.control_room_handle(),
                    index as usize,
                    gain,
                )
            }
            && self.control_room.lock().map(|mut state| state.set_output_gain(index as usize, gain)).unwrap_or(false)
    }
    pub fn set_control_room_speaker_enabled(&self, index: u32, enabled: bool) -> bool {
        let native = unsafe {
            crate::control_room_runtime::hirari_control_room_set_speaker_enabled(
                self.control_room_handle(),
                index as usize,
                enabled,
            )
        };
        let model = self.control_room.lock().map(|mut state| state.set_output_enabled(index as usize, enabled)).unwrap_or(false);
        native && model
    }
    pub fn upsert_control_room_cue(&self, id: u32, gain: f32, enabled: bool) -> bool {
        id != 0
            && gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && unsafe {
                crate::control_room_runtime::hirari_control_room_upsert_cue(
                    self.control_room_handle(),
                    id,
                    gain,
                    enabled,
                )
            }
            && self.control_room.lock().map(|mut state| state.upsert_cue(id, gain, enabled)).unwrap_or(false)
    }
    pub fn remove_control_room_cue(&self, id: u32) -> bool {
        let native = id != 0 && unsafe {
            crate::control_room_runtime::hirari_control_room_remove_cue(self.control_room_handle(), id)
        };
        let model = id != 0 && self.control_room.lock()
            .map(|mut state| state.remove_cue(id))
            .unwrap_or(false);
        native && model
    }
    pub fn set_control_room_cue_enabled(&self, id: u32, enabled: bool) -> bool {
        let native = id != 0 && unsafe {
            crate::control_room_runtime::hirari_control_room_set_cue_enabled(
                self.control_room_handle(),
                id,
                enabled,
            )
        };
        let model = id != 0 && self.control_room.lock()
            .map(|mut state| state.set_cue_enabled(id, enabled))
            .unwrap_or(false);
        native && model
    }
    pub fn set_control_room_cue_bus_track(&self, cue_id: u32, bus_track_id: Option<u32>) -> bool {
        if bus_track_id == Some(0) { return false; }
        let bus = bus_track_id.unwrap_or(0);
        // A non-zero bus target must resolve to a real BusTrack in the native
        // track list. HirariUnifiedEngine used to enforce that inline before
        // forwarding; the list is C++-owned, so it now answers through the one
        // accessor added for it while the write goes straight to the runtime.
        let native = (bus == 0
            || self.engine.as_ref().is_some_and(|engine| engine.bus_track_exists(bus)))
            && unsafe {
                crate::control_room_runtime::hirari_control_room_set_cue_bus(
                    self.control_room_handle(),
                    cue_id,
                    bus,
                )
            };
        native && self.control_room.lock()
            .map(|mut state| state.set_cue_bus_track(cue_id, bus_track_id))
            .unwrap_or(false)
    }
    pub fn set_control_room_cue_output_channel(
        &self,
        cue_id: u32,
        output_channel: Option<u32>,
    ) -> bool {
        if output_channel.is_some_and(|channel| {
            channel > 30 || (channel != 0 && channel & 1 != 0)
        }) {
            return false;
        }
        let native = unsafe {
            crate::control_room_runtime::hirari_control_room_set_cue_output(
                self.control_room_handle(),
                cue_id,
                output_channel.unwrap_or(0),
            )
        };
        native && self.control_room.lock()
            .map(|mut state| state.set_cue_output_channel(cue_id, output_channel))
            .unwrap_or(false)
    }
    pub fn set_control_room_cue_click_enabled(&self, cue_id: u32, enabled: bool) -> bool {
        let native = unsafe {
            crate::control_room_runtime::hirari_control_room_set_cue_click(
                self.control_room_handle(),
                cue_id,
                enabled,
            )
        };
        native && self.control_room.lock()
            .map(|mut state| state.set_cue_click_enabled(cue_id, enabled))
            .unwrap_or(false)
    }
    pub fn control_room_cue_gain(&self, id: u32) -> f32 {
        unsafe { crate::control_room_runtime::hirari_control_room_cue_gain(self.control_room_handle(), id) }
    }
    pub fn select_control_room_cue(&self, id: Option<u32>) -> bool {
        let native_id = id.unwrap_or(0);
        // Selecting a cue is only legal when its bus target resolves to a real
        // BusTrack. The engine used to look that up itself before committing;
        // the lookup order below mirrors it exactly -- read the cue's bus from
        // the runtime, reject a zero or missing target, then commit.
        if native_id != 0 {
            let bus_track_id = unsafe {
                crate::control_room_runtime::hirari_control_room_cue_bus(
                    self.control_room_handle(),
                    native_id,
                )
            };
            if bus_track_id == 0
                || !self.engine.as_ref().is_some_and(|engine| engine.bus_track_exists(bus_track_id))
            {
                return false;
            }
        }
        let committed = unsafe {
            crate::control_room_runtime::hirari_control_room_select_cue(
                self.control_room_handle(),
                native_id,
            )
        };
        if !committed {
            return false;
        }
        self.control_room.lock()
            .map(|mut state| state.select_cue(id))
            .unwrap_or(false)
    }
    pub fn selected_control_room_cue(&self) -> Option<u32> {
        self.control_room.lock().ok().and_then(|state| state.active_cue)
    }
    pub fn control_room_validate(&self) -> bool {
        unsafe { crate::control_room_runtime::hirari_control_room_validate(self.control_room_handle()) }
    }
    pub fn set_control_room_dim(&self, enabled: bool) {
        unsafe { crate::control_room_runtime::hirari_control_room_set_dim(self.control_room_handle(), enabled) };
        if let Ok(mut state) = self.control_room.lock() { state.dim = enabled; }
    }
    pub fn set_control_room_dim_db(&self, db: f32) -> bool {
        if !db.is_finite() || !(-60.0..=0.0).contains(&db) {
            return false;
        }
        let applied = unsafe {
            crate::control_room_runtime::hirari_control_room_set_dim_db(self.control_room_handle(), db)
        };
        if !applied {
            return false;
        }
        self.control_room.lock().map(|mut state| state.set_dim_db(db)).unwrap_or(false)
    }
    pub fn control_room_dim_db(&self) -> f32 {
        self.control_room
            .lock()
            .map(|state| state.dim_db)
            .unwrap_or(-20.0)
    }
    pub fn set_control_room_talkback(&self, enabled: bool, gain: f32) {
        if gain.is_finite() {
            let gain = gain.clamp(0.0, 4.0);
            unsafe {
                crate::control_room_runtime::hirari_control_room_set_talkback(
                    self.control_room_handle(),
                    enabled,
                    gain,
                )
            };
            if let Ok(mut state) = self.control_room.lock() {
                state.talkback = enabled;
                state.talkback_gain = gain;
            }
        }
    }
    pub fn set_control_room_talkback_input_channel(&self, channel: u32) -> bool {
        channel < 32
            && unsafe {
                crate::control_room_runtime::hirari_control_room_set_talkback_channel(
                    self.control_room_handle(),
                    channel,
                )
            }
            && self.control_room.lock().map(|mut state| {
                state.talkback_input_channel = channel;
                true
            }).unwrap_or(false)
    }
    pub fn control_room_talkback_input_channel(&self) -> u32 {
        unsafe {
            crate::control_room_runtime::hirari_control_room_talkback_channel(self.control_room_handle())
        }
    }
    pub fn control_room_dimmed(&self) -> bool {
        unsafe { crate::control_room_runtime::hirari_control_room_is_dimmed(self.control_room_handle()) }
    }
    pub fn control_room_talkback_enabled(&self) -> bool {
        unsafe {
            crate::control_room_runtime::hirari_control_room_talkback_enabled(self.control_room_handle())
        }
    }
    pub fn control_room_monitor_gain(&self) -> f32 {
        unsafe { crate::control_room_runtime::hirari_control_room_monitor_gain(self.control_room_handle()) }
    }

    pub fn set_automation_data(
        &self,
        track_id: u32,
        param_id: u32,
        packed_points: Vec<f64>,
    ) -> bool {
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.set_automation_data(track_id, param_id, packed_points));
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::AutomationChanged {
                    target: format!("audio.track.{track_id}.parameter.{param_id}"),
                },
            );
        }
        changed
    }

    pub fn set_automation_data_diagnostic_json(
        &self,
        track_id: u32,
        param_id: u32,
        packed_points: Vec<f64>,
    ) -> String {
        if !packed_points.len().is_multiple_of(3) {
            return "{\"code\":\"invalid_automation_points\",\"retryable\":false}".to_owned();
        }
        if packed_points.len() > 24_576 || packed_points.iter().any(|value| !value.is_finite()) {
            return "{\"code\":\"non_finite_automation_points\",\"retryable\":false}".to_owned();
        }
        if param_id > 1 {
            return "{\"code\":\"unsupported_automation_parameter\",\"retryable\":false}".to_owned();
        }
        let (min_value, max_value) = if param_id == 0 {
            (0.0, 2.0)
        } else {
            (-1.0, 1.0)
        };
        let mut previous_time = f64::NEG_INFINITY;
        for triple in packed_points.as_chunks::<3>().0 {
            if triple[0] < 0.0
                || triple[0].fract() != 0.0
                || triple[0] <= previous_time
                || !(min_value..=max_value).contains(&triple[1])
                || !(-1.0..=1.0).contains(&triple[2])
            {
                return "{\"code\":\"invalid_automation_range\",\"retryable\":false}".to_owned();
            }
            previous_time = triple[0];
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.set_automation_data(track_id, param_id, packed_points) {
            return format!("{{\"ok\":true,\"track_id\":{track_id},\"parameter_id\":{param_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "automation_rejected",
                "automation data was rejected by the native graph",
            )
            .object(format!("track:{track_id}/parameter:{param_id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_track_delay_automation_diagnostic_json(
        &self,
        track_id: u32,
        packed_points: Vec<f64>,
    ) -> String {
        if !packed_points.len().is_multiple_of(3)
            || packed_points.len() > 24_576
            || packed_points.iter().any(|value| !value.is_finite())
        {
            return "{\"code\":\"invalid_track_delay_automation\",\"retryable\":false}".to_owned();
        }
        let mut previous_time = f64::NEG_INFINITY;
        for triple in packed_points.as_chunks::<3>().0 {
            if triple[0] < 0.0
                || triple[0].fract() != 0.0
                || triple[0] <= previous_time
                || triple[1] < 0.0
                || triple[1] > 1.0
                || triple[2] < -1.0
                || triple[2] > 1.0
            {
                return "{\"code\":\"invalid_track_delay_automation_range\",\"retryable\":false}"
                    .to_owned();
            }
            previous_time = triple[0];
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.set_track_delay_automation(track_id, packed_points) {
            return format!("{{\"ok\":true,\"track_id\":{track_id},\"operation\":\"set_track_delay_automation\"}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_delay_automation_rejected",
                "track delay automation was rejected",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_plugin_parameter(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        value: f32,
    ) -> bool {
        if !value.is_finite() {
            return false;
        }
        let normalized = value.clamp(0.0, 1.0);
        let applied = self.engine.as_ref().is_some_and(|engine| {
            engine.set_plugin_parameter(track_id, plugin_index, parameter_id, normalized)
        });
        if applied {
            if let Ok(mut events) = self.plugin_parameter_events.lock() {
                if events.len() >= 256 {
                    events.drain(..64);
                }
                events.push(PluginParameterEvent {
                    track_id,
                    plugin_index,
                    parameter_id,
                    value: normalized,
                });
            }
        }
        applied
    }

    pub fn set_plugin_automation(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        packed_points: Vec<f64>,
    ) -> bool {
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_plugin_automation(track_id, plugin_index, parameter_id, packed_points)
        })
    }

    pub fn set_plugin_parameter_diagnostic_json(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        value: f32,
    ) -> String {
        if !value.is_finite() {
            return "{\"code\":\"non_finite_parameter\",\"retryable\":false}".to_owned();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        let normalized = value.clamp(0.0, 1.0);
        if self.set_plugin_parameter(track_id, plugin_index, parameter_id, normalized) {
            return format!("{{\"ok\":true,\"track_id\":{track_id},\"plugin_index\":{plugin_index},\"parameter_id\":{parameter_id},\"value\":{normalized}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "plugin_parameter_rejected",
                "plugin parameter update was rejected",
            )
            .object(format!(
                "track:{track_id}/plugin:{plugin_index}/parameter:{parameter_id}"
            ))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Analyze a bounded audio window and apply the resulting compressor
    /// settings as one user-facing undo transaction. The target plugin is
    /// expected to expose the internal Compressor parameter contract
    /// (0=threshold, 1=ratio, 2=attack, 3=release).
    pub fn apply_dynamics_suggestion_diagnostic_json(
        &self,
        track_id: u32,
        plugin_index: u32,
        samples: Vec<f32>,
    ) -> String {
        if track_id == 0
            || samples.len() > 262_144
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return serde_json::json!({
                "code": "invalid_dynamics_target",
                "retryable": false,
            })
            .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let params = crate::dynamics::DynamicsOrchestrator.suggest_parameters(&samples);
        let normalized = [
            ((params.threshold_db + 60.0) / 60.0).clamp(0.0, 1.0),
            ((params.ratio - 1.0) / 19.0).clamp(0.0, 1.0),
            ((params.attack_ms - 0.1) / 99.9).clamp(0.0, 1.0),
            ((params.release_ms - 5.0) / 995.0).clamp(0.0, 1.0),
        ];
        engine.begin_undo_transaction("Dynamics Assistant");
        for (parameter_id, value) in normalized.iter().copied().enumerate() {
            if !self.set_plugin_parameter(track_id, plugin_index, parameter_id as u32, value) {
                let _ = engine.abort_undo_transaction();
                return serde_json::json!({
                    "code": "dynamics_apply_rejected",
                    "retryable": false,
                    "track_id": track_id,
                    "plugin_index": plugin_index,
                    "parameter_id": parameter_id,
                })
                .to_string();
            }
        }
        if !engine.end_undo_transaction() {
            let _ = engine.abort_undo_transaction();
            return "{\"code\":\"dynamics_transaction_failed\",\"retryable\":true}".to_owned();
        }
        serde_json::json!({
            "ok": true,
            "operation": "apply_dynamics_suggestion",
            "track_id": track_id,
            "plugin_index": plugin_index,
            "threshold_db": params.threshold_db,
            "ratio": params.ratio,
            "attack_ms": params.attack_ms,
            "release_ms": params.release_ms,
            "sample_count": samples.len(),
        })
        .to_string()
    }

    pub fn drain_plugin_parameter_events(&self) -> Vec<PluginParameterEvent> {
        self.plugin_parameter_events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }

    pub fn get_plugin_parameter(&self, track_id: u32, plugin_index: u32, parameter_id: u32) -> f32 {
        self.engine.as_ref().map_or(0.0, |engine| {
            engine.get_plugin_parameter(track_id, plugin_index, parameter_id)
        })
    }
    pub fn get_plugin_parameter_count(&self, track_id: u32, plugin_index: u32) -> u32 {
        self.engine.as_ref().map_or(0, |engine| {
            engine.get_plugin_parameter_count(track_id, plugin_index)
        })
    }
    pub fn get_plugin_parameter_name(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
    ) -> String {
        self.engine.as_ref().map_or_else(String::new, |engine| {
            engine.get_plugin_parameter_name(track_id, plugin_index, parameter_id)
        })
    }

    pub fn save_plugin_preset(&self, track_id: u32, plugin_index: u32, path: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.save_plugin_preset(track_id, plugin_index, path))
    }

    pub fn save_plugin_preset_diagnostic_json(
        &self,
        track_id: u32,
        plugin_index: u32,
        path: &str,
    ) -> String {
        if track_id == 0 || path.trim().is_empty() {
            return serde_json::json!({"code":"invalid_plugin_preset_target","retryable":false})
                .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.save_plugin_preset(track_id, plugin_index, path) {
            return serde_json::json!({"ok":true,"operation":"save_plugin_preset","track_id":track_id,"plugin_index":plugin_index,"path":path}).to_string();
        }
        serde_json::json!({"code":"plugin_preset_save_failed","retryable":false,"track_id":track_id,"plugin_index":plugin_index}).to_string()
    }

    pub fn load_plugin_preset(&self, track_id: u32, plugin_index: u32, path: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.load_plugin_preset(track_id, plugin_index, path))
    }

    pub fn load_plugin_preset_diagnostic_json(
        &self,
        track_id: u32,
        plugin_index: u32,
        path: &str,
    ) -> String {
        if track_id == 0 || path.trim().is_empty() {
            return serde_json::json!({"code":"invalid_plugin_preset_target","retryable":false})
                .to_string();
        }
        if !std::path::Path::new(path).is_file() {
            return serde_json::json!({"code":"plugin_preset_not_found","retryable":false,"path":path}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.load_plugin_preset(track_id, plugin_index, path) {
            return serde_json::json!({"ok":true,"operation":"load_plugin_preset","track_id":track_id,"plugin_index":plugin_index,"path":path}).to_string();
        }
        serde_json::json!({"code":"plugin_preset_load_failed","retryable":false,"track_id":track_id,"plugin_index":plugin_index}).to_string()
    }
}
