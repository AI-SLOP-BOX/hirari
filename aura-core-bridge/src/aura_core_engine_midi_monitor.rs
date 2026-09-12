impl AuraCore {
    pub fn midi_monitor_filtered_json(
        &self,
        status_mask: u8,
        status_value: u8,
        start: u64,
        end: u64,
    ) -> String {
        self.midi_monitor
            .lock()
            .ok()
            .and_then(|m| {
                serde_json::to_string(&m.filter(status_mask, status_value, start, end)).ok()
            })
            .unwrap_or_else(|| "[]".to_owned())
    }
    pub fn control_room_json(&self) -> String {
        self.control_room
            .lock()
            .ok()
            .and_then(|s| serde_json::to_string(&*s).ok())
            .unwrap_or_else(|| "{}".into())
    }
    pub fn control_room_monitor_snapshot_json(&self) -> String {
        self.control_room
            .lock()
            .ok()
            .map(|s| {
                serde_json::json!({
                    "active_output": s.monitor_outputs.get(s.active_output).cloned(),
                    "active_output_gain": s.monitor_output_gains.get(s.active_output).copied().unwrap_or(1.0),
                    "active_output_enabled": s.monitor_output_enabled.get(s.active_output).copied().unwrap_or(true),
                    "monitor_gain": s.effective_monitor_gain(),
                    "dim": s.dim,
                    "talkback_gain": s.effective_talkback_gain(),
                    "cue_gain": s.effective_cue_gain(),
                    "reference_track": s.reference_track,
                    "reference_enabled": s.reference_enabled
                })
                .to_string()
            })
            .unwrap_or_else(|| "{}".into())
    }
    pub fn set_control_room_json(&self, state_json: &str) -> bool {
        let Ok(state) = serde_json::from_str::<crate::control_room::ControlRoomState>(state_json)
        else {
            return false;
        };
        if !state.validate() {
            return false;
        }
        self.control_room
            .lock()
            .map(|mut current| {
                *current = state;
                true
            })
            .unwrap_or(false)
    }
    pub fn expression_map_validate_json(&self, map_json: &str) -> bool {
        serde_json::from_str::<crate::expression_map::ExpressionMap>(map_json)
            .map(|m| m.validate())
            .unwrap_or(false)
    }
    pub fn accessibility_state_validate_json(&self, state_json: &str) -> bool {
        serde_json::from_str::<crate::accessibility::AccessibilityState>(state_json)
            .map(|s| s.validate())
            .unwrap_or(false)
    }
    pub fn help_search_json(&self, catalog_json: &str, locale: &str, query: &str) -> String {
        let Ok(c) = serde_json::from_str::<crate::help_catalog::HelpCatalog>(catalog_json) else {
            return "[]".to_owned();
        };
        serde_json::to_string(&c.search(locale, query)).unwrap_or_else(|_| "[]".to_owned())
    }
    pub fn translate_ui_json(&self, catalog_json: &str, locale: &str, key: &str) -> String {
        let Ok(c) = serde_json::from_str::<crate::localization::TranslationCatalog>(catalog_json)
        else {
            return "null".to_owned();
        };
        serde_json::to_string(&c.translate(locale, key)).unwrap_or_else(|_| "null".to_owned())
    }
    pub fn sidechain_port_validate_json(&self, port_json: &str) -> bool {
        serde_json::from_str::<crate::sidechain::SidechainPort>(port_json)
            .map(|p| p.validate())
            .unwrap_or(false)
    }
    pub fn plugin_processing_state_validate_json(&self, state_json: &str) -> bool {
        serde_json::from_str::<crate::plugin_processing::PluginProcessingState>(state_json)
            .map(|s| s.validate())
            .unwrap_or(false)
    }
    pub fn plugin_latency_registry_validate_json(&self, registry_json: &str) -> bool {
        serde_json::from_str::<crate::plugin_processing::PluginLatencyRegistry>(registry_json)
            .map(|r| r.validate())
            .unwrap_or(false)
    }
    pub fn plugin_latency_registry_snapshot_json(&self, registry_json: &str) -> String {
        let Ok(registry) =
            serde_json::from_str::<crate::plugin_processing::PluginLatencyRegistry>(registry_json)
        else {
            return "{}".into();
        };
        serde_json::json!({"ok": registry.validate(), "total_samples": registry.total_samples(), "max": registry.max_latency(), "count": registry.samples.len(), "entries": registry.samples}).to_string()
    }
    pub fn plugin_registry_validate_json(&self, registry_json: &str) -> bool {
        serde_json::from_str::<crate::plugin_registry::PluginRegistry>(registry_json)
            .map(|r| r.validate())
            .unwrap_or(false)
    }
    pub fn plugin_registry_allowed_json(&self, registry_json: &str, plugin_id: &str) -> bool {
        serde_json::from_str::<crate::plugin_registry::PluginRegistry>(registry_json)
            .map(|r| r.is_allowed(plugin_id))
            .unwrap_or(false)
    }
    pub fn plugin_registry_regressions_json(
        &self,
        current_json: &str,
        baseline_json: &str,
    ) -> String {
        let (Ok(current), Ok(baseline)) = (
            serde_json::from_str::<crate::plugin_registry::PluginRegistry>(current_json),
            serde_json::from_str::<crate::plugin_registry::PluginRegistry>(baseline_json),
        ) else {
            return "[]".into();
        };
        serde_json::to_string(&current.regression_ids(&baseline)).unwrap_or_else(|_| "[]".into())
    }
    pub fn plugin_preset_search_json(
        &self,
        browser_json: &str,
        plugin_id: &str,
        query: &str,
        favorites_only: bool,
        compatible_only: bool,
    ) -> String {
        let Ok(browser) =
            serde_json::from_str::<crate::plugin_presets::PluginPresetBrowser>(browser_json)
        else {
            return "[]".to_owned();
        };
        serde_json::to_string(&browser.search(
            (!plugin_id.trim().is_empty()).then_some(plugin_id),
            query,
            favorites_only,
            compatible_only,
        ))
        .unwrap_or_else(|_| "[]".to_owned())
    }
    pub fn midi_device_profile_validate_json(&self, profile_json: &str) -> bool {
        serde_json::from_str::<crate::midi_device_profiles::MidiDeviceProfile>(profile_json)
            .map(|p| p.validate())
            .unwrap_or(false)
    }
    pub fn midi_device_patch_search_json(&self, profile_json: &str, query: &str) -> String {
        let Ok(profile) =
            serde_json::from_str::<crate::midi_device_profiles::MidiDeviceProfile>(profile_json)
        else {
            return "[]".into();
        };
        serde_json::to_string(&profile.search_patches(query)).unwrap_or_else(|_| "[]".into())
    }
    pub fn remote_transport_parse_json(&self, command: &str) -> String {
        serde_json::to_string(
            &crate::hardware::parse_remote_transport(command)
                .map(|value| format!("{value:?}"))
                .unwrap_or_default(),
        )
        .unwrap_or_else(|_| "null".into())
    }
    pub fn command_macro_validate_json(&self, macro_json: &str) -> bool {
        serde_json::from_str::<crate::command_macros::CommandMacro>(macro_json)
            .map(|m| m.validate())
            .unwrap_or(false)
    }
    pub fn midi_device_patch_json(
        &self,
        profile_json: &str,
        bank_msb: u8,
        bank_lsb: u8,
        program: u8,
    ) -> String {
        let Ok(profile) =
            serde_json::from_str::<crate::midi_device_profiles::MidiDeviceProfile>(profile_json)
        else {
            return "null".to_owned();
        };
        serde_json::to_string(&profile.find_patch(bank_msb, bank_lsb, program))
            .unwrap_or_else(|_| "null".to_owned())
    }
    pub fn workspace_layout_validate_json(&self, layout_json: &str) -> bool {
        serde_json::from_str::<crate::workspace::WorkspaceLayout>(layout_json)
            .map(|l| l.validate())
            .unwrap_or(false)
    }
    pub fn media_tag_search_json(
        &self,
        index_json: &str,
        query: &str,
        favorites_only: bool,
    ) -> String {
        let Ok(index) = serde_json::from_str::<crate::asset::MediaTagIndex>(index_json) else {
            return "[]".to_owned();
        };
        serde_json::to_string(&index.search(query, favorites_only))
            .unwrap_or_else(|_| "[]".to_owned())
    }
    pub fn tempo_sync_json(&self, source_bpm: f64, project_bpm: f64, samples: u64) -> String {
        match crate::asset::tempo_sync_ratio(source_bpm, project_bpm) {
            Some(ratio) => serde_json::json!({"ok":true,"ratio":ratio,"synced_samples":crate::asset::tempo_sync_length(samples, source_bpm, project_bpm)}).to_string(),
            None => serde_json::json!({"ok":false}).to_string(),
        }
    }

    pub fn add_review_note(&self, author: &str, text: &str, timestamp_ms: u64) -> u64 {
        self.review_notes
            .lock()
            .ok()
            .and_then(|mut s| s.add(author, text, timestamp_ms))
            .unwrap_or(0)
    }

    pub fn set_review_note_status(&self, id: u64, status: &str) -> bool {
        let status = match status {
            "open" => crate::review_notes::ReviewStatus::Open,
            "resolved" => crate::review_notes::ReviewStatus::Resolved,
            "rejected" => crate::review_notes::ReviewStatus::Rejected,
            _ => return false,
        };
        self.review_notes
            .lock()
            .map(|mut s| s.set_status(id, status))
            .unwrap_or(false)
    }

    pub fn review_notes_json(&self) -> String {
        self.review_notes
            .lock()
            .ok()
            .and_then(|s| serde_json::to_string(s.snapshot()).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn clear_review_notes(&self) {
        if let Ok(mut s) = self.review_notes.lock() {
            s.clear();
        }
    }

    pub fn plugin_compatibility_snapshot_json(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.plugin_compatibility_snapshot_json())
            .unwrap_or_else(|| "[]".to_owned())
    }
    pub fn media_usage_diagnostic_json(&self, timeline_json: &str, media_dir: &str) -> String {
        let assets = crate::asset::AssetOrchestrator::new();
        assets
            .analyze_media_usage(timeline_json.as_bytes(), media_dir)
            .unwrap_or_else(|| serde_json::json!({"ok":false,"code":"invalid_media_timeline"}))
            .to_string()
    }
    /// Sends an outbound MIDI packet and mirrors it into the monitor so UI
    /// feedback reflects the exact bytes handed to the device adapter.
    pub fn send_midi_message_monitored(&self, unique_id: u32, data: &[u8]) -> bool {
        if data.is_empty() || data.len() > 4096 {
            return false;
        }
        let sent = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.send_midi_message(unique_id, data));
        if sent && data.len() >= 3 {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let _ = self.push_midi_monitor_event(timestamp, data[0], data[1], data[2]);
        }
        sent
    }

    pub fn list_midi_devices_json(&self) -> String {
        self.engine.as_ref().map(|engine| engine.list_midi_devices_json().to_string()).unwrap_or_else(|| "[]".to_owned())
    }

    pub fn start_midi_input(&self) -> bool {
        self.engine.as_ref().is_some_and(|engine| engine.start_midi_input())
    }

    pub fn stop_midi_input(&self) {
        if let Some(engine) = self.engine.as_ref() { engine.stop_midi_input(); }
    }

    pub fn poll_midi_input_json(&self) -> String {
        self.engine.as_ref().map(|engine| engine.poll_midi_input_json().to_string()).unwrap_or_else(|| "[]".to_owned())
    }

    /// Invoke a project-local extension through the same bounded process
    /// runner used by the command API.  Keeping this small adapter here lets
    /// the native UI command palette share the CLI security boundary.
    pub fn invoke_extension_json(
        &self,
        root: &str,
        extension_id: &str,
        command_id: &str,
        payload_json: &str,
        timeout_ms: u64,
    ) -> String {
        let payload = match serde_json::from_str::<serde_json::Value>(payload_json) {
            Ok(value) => value,
            Err(error) => return serde_json::json!({"ok": false, "code": "extension_payload_invalid", "message": error.to_string()}).to_string(),
        };
        match crate::extensions::invoke_command(root, extension_id, command_id, &payload, timeout_ms) {
            Ok(value) => serde_json::json!({"ok": true, "operation": "extension_invoke", "extension_id": extension_id, "command_id": command_id, "result": value}).to_string(),
            Err(error) => serde_json::json!({"ok": false, "code": "extension_invocation_failed", "message": error}).to_string(),
        }
    }

    /// Returns the manifest-only extension catalog consumed by the UI,
    /// command palette, and external automation clients. Discovery never
    /// executes extension code; command and declarative UI registrations are
    /// returned together so all clients see the same surface.
    pub fn extension_catalog_json(&self, root: &str) -> String {
        if root.trim().is_empty() || root.len() > 4096 || root.contains('\0') {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_extension_root",
                "retryable": false,
            })
            .to_string();
        }
        let (extensions, discovery_errors) = crate::extensions::discover(root);
        let (commands, mut errors) = crate::extensions::command_catalog(root);
        let (ui, ui_errors) = crate::extensions::ui_registry(root);
        errors.extend(discovery_errors);
        errors.extend(ui_errors);
        errors.sort();
        errors.dedup();
        serde_json::json!({
            "ok": true,
            "root": root,
            "extensions": extensions,
            "commands": commands,
            "ui": ui,
            "glossary": crate::glossary::entries(),
            "marketplace": {
                "status": "catalog-only",
                "network_install": false,
                "listings": crate::extensions::marketplace_catalog(),
            },
            "recent_run_history": crate::extensions::recent_run_history(root),
            "errors": errors,
        })
        .to_string()
    }

    /// Toggle a project-local extension without executing it.  This gives
    /// advanced users a reversible switch while keeping disabled extensions
    /// out of both the command palette and declarative UI registry.
    pub fn set_extension_enabled_json(
        &self,
        root: &str,
        extension_id: &str,
        enabled: bool,
    ) -> String {
        if root.trim().is_empty() || extension_id.trim().is_empty() {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_extension_activation_request",
            })
            .to_string();
        }
        match crate::extensions::set_enabled(root, extension_id, enabled) {
            Ok(()) => serde_json::json!({
                "ok": true,
                "operation": "set_extension_enabled",
                "extension_id": extension_id,
                "enabled": enabled,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "extension_activation_failed",
                "message": error,
            })
            .to_string(),
        }
    }

    pub fn plugin_path_admission(path: &str) -> &'static str {
        if path.is_empty() {
            return "missing-path";
        }
        if path.len() > 4096 || path.contains('\0') {
            return "invalid-path";
        }
        let normalized = path.to_ascii_lowercase();
        if normalized.starts_with("builtin://") {
            return "builtin";
        }
        let format = if normalized.ends_with(".vst3") {
            "sandbox-vst3"
        } else if normalized.ends_with(".component") {
            "sandbox-au"
        } else if normalized.ends_with(".clap") {
            "sandbox-clap"
        } else {
            return "unsupported-extension";
        };
        // Keep this method useful as a format classifier even before a plugin
        // has been installed. Callers that actually load a plugin still
        // perform a filesystem admission check below.
        if !std::path::Path::new(path).exists() {
            return format;
        }
        if crate::plugin_catalog::is_quarantined_path(path) {
            return "quarantined";
        }
        if !crate::plugin_catalog::is_admitted_path(path) {
            return "unadmitted-plugin";
        }
        format
    }

    pub fn audio_driver_status(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.audio_driver_status().to_string())
            .unwrap_or_else(|| "unavailable".to_owned())
    }

    pub fn process_sandboxed_plugin_block(
        &self,
        track_id: u32,
        sandbox_index: u32,
        left: &mut [f32],
        right: &mut [f32],
    ) -> bool {
        if left.is_empty() || left.len() != right.len() || left.len() > MAX_SANDBOX_AUDIO_BLOCK {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.process_sandboxed_plugin_block(track_id, sandbox_index, left, right)
        })
    }

    pub fn is_silent_audio_fallback(&self) -> bool {
        self.engine
            .as_ref()
            .is_none_or(|engine| engine.is_silent_fallback())
    }

    /// Runs one bounded stereo block through the native C++ graph.
    pub fn process_audio_block(&self, left: &mut [f32], right: &mut [f32]) -> bool {
        if left.is_empty() || left.len() != right.len() || left.len() > MAX_NATIVE_AUDIO_BLOCK {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        engine.process_audio_block(left, right);
        let mut valid = true;
        for sample in left.iter_mut().chain(right.iter_mut()) {
            if !sample.is_finite() {
                *sample = 0.0;
                valid = false;
            }
        }
        valid
    }
}
