impl AuraCore {
    pub fn set_preview_synth_engine(&self, engine: u32) {
        if let Some(audio) = self.engine.as_ref() {
            audio.set_preview_synth_engine(engine.min(2));
        }
    }

    pub fn set_preview_synth_engine_diagnostic_json(&self, engine: u32) -> String {
        if engine > 2 {
            let error = crate::bridge_error::BridgeError::new(
                "unsupported_preview_synth",
                "preview synth engine must be 0, 1, or 2",
            );
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(audio) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        audio.set_preview_synth_engine(engine);
        format!("{{\"ok\":true,\"preview_synth_engine\":{engine}}}")
    }

    pub fn set_route(&self, source_id: u32, dest_id: u32, enabled: bool) -> bool {
        if source_id == dest_id {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_route(source_id, dest_id, enabled))
    }

    pub fn set_route_gain(&self, source_id: u32, dest_id: u32, gain: f32, enabled: bool) -> bool {
        if source_id == dest_id
            || !gain.is_finite()
            || !(0.0..=2.0).contains(&gain)
            || (enabled && gain <= 0.0)
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_route_gain(source_id, dest_id, gain, enabled))
    }

    pub fn set_route_gain_diagnostic_json(
        &self,
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    ) -> String {
        if source_id == dest_id {
            return "{\"code\":\"route_self_loop\",\"retryable\":false}".to_owned();
        }
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) || (enabled && gain <= 0.0) {
            return "{\"code\":\"invalid_route_gain\",\"retryable\":false}".to_owned();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.set_route_gain(source_id, dest_id, gain, enabled) {
            return format!(
                "{{\"ok\":true,\"source_id\":{source_id},\"dest_id\":{dest_id},\"gain\":{gain},\"enabled\":{enabled}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "route_gain_rejected",
                "audio route gain was rejected by the graph",
            )
            .object(format!("route:{source_id}->{dest_id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_route_diagnostic_json(&self, source_id: u32, dest_id: u32, enabled: bool) -> String {
        if source_id == dest_id {
            return "{\"code\":\"route_self_loop\",\"retryable\":false}".to_owned();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.set_route(source_id, dest_id, enabled) {
            return format!("{{\"ok\":true,\"source_id\":{source_id},\"dest_id\":{dest_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "route_rejected",
                "route was rejected by the graph",
            )
            .object(format!("route:{source_id}->{dest_id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_feedback_route(
        &self,
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    ) -> bool {
        if source_id == dest_id || !gain.is_finite() {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_feedback_route(source_id, dest_id, gain.clamp(0.0, 2.0), enabled)
        })
    }

    pub fn set_feedback_route_diagnostic_json(
        &self,
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    ) -> String {
        if source_id == dest_id {
            return "{\"code\":\"route_self_loop\",\"retryable\":false}".to_owned();
        }
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return "{\"code\":\"invalid_route_gain\",\"retryable\":false}".to_owned();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.set_feedback_route(source_id, dest_id, gain, enabled) {
            return format!("{{\"ok\":true,\"source_id\":{source_id},\"dest_id\":{dest_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "feedback_route_rejected",
                "feedback route was rejected",
            )
            .object(format!("route:{source_id}->{dest_id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_sidechain_link(
        &self,
        source_id: u32,
        dest_id: u32,
        plugin_index: u32,
        tap_point: u32,
        enabled: bool,
    ) -> bool {
        if source_id == dest_id || tap_point > 2 {
            return false;
        }
        self.engine
            .as_ref()
            .map(|engine| {
                engine.set_sidechain_link(source_id, dest_id, plugin_index, tap_point, enabled)
            })
            .unwrap_or(false)
    }

    pub fn set_sidechain_link_diagnostic_json(
        &self,
        source_id: u32,
        dest_id: u32,
        plugin_index: u32,
        tap_point: u32,
        enabled: bool,
    ) -> String {
        if source_id == dest_id {
            return "{\"code\":\"sidechain_self_loop\",\"retryable\":false}".to_owned();
        }
        if tap_point > 2 {
            return "{\"code\":\"invalid_sidechain_tap\",\"retryable\":false}".to_owned();
        }
        if self.set_sidechain_link(source_id, dest_id, plugin_index, tap_point, enabled) {
            return format!("{{\"ok\":true,\"source_id\":{source_id},\"dest_id\":{dest_id},\"plugin_index\":{plugin_index}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "sidechain_rejected",
                "sidechain link was rejected",
            )
            .object(format!(
                "sidechain:{source_id}->{dest_id}/plugin:{plugin_index}"
            ))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn has_sidechain_link(&self, source_id: u32, dest_id: u32, plugin_index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.has_sidechain_link(source_id, dest_id, plugin_index))
    }
    pub fn is_audio_device_ready(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.is_audio_device_ready())
    }
    pub fn get_audio_output_peak(&self) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.get_audio_output_peak())
    }
    pub fn get_audio_callback_count(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.get_audio_callback_count())
    }
    pub fn get_dropped_input_blocks(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.get_dropped_input_blocks())
    }
    /// Returns `(device_ready, silent_fallback, dropped_input_blocks)` from
    /// one control-side snapshot for recording diagnostics and telemetry.
    pub fn audio_input_health(&self) -> (bool, bool, u64) {
        (
            self.is_audio_device_ready(),
            self.is_silent_audio_fallback(),
            self.get_dropped_input_blocks(),
        )
    }
    pub fn try_reconnect_audio_device(&self) {
        if let Some(e) = self.engine.as_ref() {
            e.try_reconnect_audio_device();
        }
    }
    pub fn try_reconnect_audio_device_diagnostic_json(&self) -> String {
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        self.try_reconnect_audio_device();
        self.audio_driver_diagnostic_json()
    }
    pub fn get_sample_rate(&self) -> f64 {
        self.engine.as_ref().map_or(0.0, |e| e.get_sample_rate())
    }
    /// Returns the native CoreAudio device catalog. The empty catalog is a
    /// valid result on offline/non-macOS backends.
    pub fn list_audio_devices_json(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.list_audio_devices_json().to_string())
            .unwrap_or_else(|| "[]".to_owned())
    }
    /// Selects a CoreAudio device and rebuilds the callback/engine boundary
    /// atomically at the requested format.
    pub fn select_audio_device(&self, device_id: u32, sample_rate: u32, buffer_size: u32) -> bool {
        if !matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000 | 192_000)
            || !matches!(buffer_size, 32 | 64 | 128 | 256 | 512 | 1024 | 2048)
            || self.recording_preview_active()
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.select_audio_device(device_id, sample_rate as f64, buffer_size)
        })
    }
    pub fn apply_audio_config(&self, sample_rate: u32, buffer_size: u32) -> bool {
        if !matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000 | 192_000)
            || !matches!(buffer_size, 32 | 64 | 128 | 256 | 512 | 1024 | 2048)
        {
            return false;
        }
        // Reconfiguration changes the native audio generation and capture
        // format. Do not invalidate an active recording spool mid-session.
        if self.recording_preview_active() {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            if engine.is_playing() {
                engine.set_playing(false);
            }
            engine.try_apply_config(engine.get_tempo(), sample_rate, buffer_size)
        })
    }
    pub fn apply_audio_config_diagnostic_json(&self, sample_rate: u32, buffer_size: u32) -> String {
        if !matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000 | 192_000) {
            return "{\"code\":\"unsupported_sample_rate\",\"retryable\":false}".to_owned();
        }
        if !matches!(buffer_size, 32 | 64 | 128 | 256 | 512 | 1024 | 2048) {
            return "{\"code\":\"unsupported_buffer_size\",\"retryable\":false}".to_owned();
        }
        if self.recording_preview_active() {
            return "{\"code\":\"recording_active\",\"retryable\":false}".to_owned();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.apply_audio_config(sample_rate, buffer_size) {
            return serde_json::json!({
                "ok": true,
                "sample_rate": sample_rate,
                "buffer_size": buffer_size,
                "audio_generation": self.audio_config_generation(),
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "audio_config_rejected",
            "retryable": true,
            "driver": serde_json::from_str::<serde_json::Value>(&self.audio_driver_diagnostic_json())
                .unwrap_or_else(|_| serde_json::json!({"status": "unknown", "error_code": 0})),
            "project_generation": self.project_generation(),
            "audio_generation": self.audio_config_generation(),
        }).to_string()
    }
}
