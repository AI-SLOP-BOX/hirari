fn sync_sidecar_parent(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

const MAX_NATIVE_AUDIO_BLOCK: usize = 16_384;
// The shared sandbox mailbox has a separate, smaller wire contract. Keep it
// aligned with PluginSandboxProtocol::kMaxFrames rather than accidentally
// widening the Rust admission check beyond the IPC allocation.
const MAX_SANDBOX_AUDIO_BLOCK: usize = 8_192;

fn parse_plugin_instance_id(value: &str) -> Option<(u32, u32)> {
    // Keep the accepted persisted form strict without accepting arbitrary
    // path-like identifiers.
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() == 4 && parts[0] == "track" && parts[2] == "slot" {
        Some((parts[1].parse().ok()?, parts[3].parse().ok()?))
    } else {
        None
    }
}

fn copy_file_atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> bool {
    if source == destination || !source.is_file() {
        return false;
    }
    let temporary = PathBuf::from(format!(
        "{}.copy-tmp-{}-{}",
        destination.display(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| {
        let mut input = std::fs::File::open(source)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        std::fs::rename(&temporary, destination)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    sync_sidecar_parent(destination)
}

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
                    "monitor_gain": s.effective_monitor_gain(),
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

    pub fn set_midi_events_json(&self, snapshot: &str) -> bool {
        let Ok(events) = serde_json::from_str::<Vec<midi::MIDIEvent>>(snapshot) else {
            return false;
        };
        if events.iter().any(|event| !event.validate()) {
            return false;
        }
        let mut events = events;
        if !midi::MIDIOrchestrator::normalize_events(&mut events) {
            return false;
        }
        let Ok(mut current) = self.midi_events.lock() else {
            return false;
        };
        *current = events;
        true
    }

    /// Structured counterpart for CLI/UI callers. The legacy bool API remains
    /// for ABI compatibility, but parse, validation, normalization, and lock
    /// failures must not collapse into the same silent `false` result.
    pub fn set_midi_events_diagnostic_json(&self, snapshot: &str) -> String {
        let events = match serde_json::from_str::<Vec<midi::MIDIEvent>>(snapshot) {
            Ok(events) => events,
            Err(error) => {
                return serde_json::json!({
                    "code": "invalid_midi_snapshot",
                    "message": error.to_string(),
                    "retryable": false
                })
                .to_string();
            }
        };
        if events.iter().any(|event| !event.validate()) {
            return serde_json::json!({
                "code": "invalid_midi_event",
                "message": "one or more MIDI events failed validation",
                "retryable": false
            })
            .to_string();
        }
        let mut normalized = events;
        if !midi::MIDIOrchestrator::normalize_events(&mut normalized) {
            return serde_json::json!({
                "code": "midi_normalization_rejected",
                "message": "MIDI event normalization was rejected",
                "retryable": false
            })
            .to_string();
        }
        let Ok(mut current) = self.midi_events.lock() else {
            return serde_json::json!({
                "code": "midi_state_unavailable",
                "message": "MIDI state lock is poisoned",
                "retryable": true
            })
            .to_string();
        };
        *current = normalized;
        serde_json::json!({"ok": true, "operation": "set_midi_events"}).to_string()
    }

    pub fn apply_midi_swing(&self, subdivision_beats: f32, amount: f32) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::apply_swing(&mut events, subdivision_beats, amount);
        drop(events);
        result && self.apply_swing_scheduled_midi(subdivision_beats, amount)
    }

    fn apply_swing_scheduled_midi(&self, subdivision_beats: f32, amount: f32) -> bool {
        let subdivision = self.beats_to_samples(subdivision_beats as f64);
        if subdivision == 0 {
            return false;
        }
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            let cell = note.start_sample / subdivision;
            if !cell.is_multiple_of(2) {
                let offset = (subdivision as f64 * 0.5 * amount as f64).round();
                let next = note.start_sample as f64 + offset;
                if !next.is_finite() || next < 0.0 || next > u64::MAX as f64 {
                    return false;
                }
                note.start_sample = next as u64;
            }
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn apply_midi_swing_diagnostic_json(&self, subdivision_beats: f32, amount: f32) -> String {
        let result = if !subdivision_beats.is_finite()
            || !(0.001..=16.0).contains(&subdivision_beats)
            || !amount.is_finite()
            || !(-1.0..=1.0).contains(&amount)
        {
            crate::bridge_error::BridgeError::new(
                "invalid_midi_swing",
                "subdivision and swing amount are out of range",
            )
        } else {
            match self.midi_events.lock() {
                Err(_) => crate::bridge_error::BridgeError::new(
                    "midi_state_unavailable",
                    "MIDI state lock is poisoned",
                )
                .retryable(true),
                Ok(mut events) => {
                    if midi::MIDIOrchestrator::apply_swing(&mut events, subdivision_beats, amount) {
                        drop(events);
                        if !self.apply_swing_scheduled_midi(subdivision_beats, amount) {
                            return serde_json::json!({"ok":false,"code":"scheduled_midi_swing_rejected","retryable":false}).to_string();
                        }
                        return "{\"ok\":true,\"operation\":\"apply_midi_swing\"}".to_owned();
                    }
                    crate::bridge_error::BridgeError::new(
                        "midi_swing_rejected",
                        "MIDI swing operation was rejected",
                    )
                }
            }
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    fn quantize_scheduled_midi(&self, grid_beats: f32, strength: f32) -> bool {
        let grid_samples = self.beats_to_samples(grid_beats as f64);
        if grid_samples == 0 {
            return false;
        }
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            let target = ((note.start_sample as f64 / grid_samples as f64).round()
                * grid_samples as f64) as u64;
            let position = note.start_sample as f64
                + (target as f64 - note.start_sample as f64) * strength as f64;
            if !position.is_finite() || position < 0.0 || position > u64::MAX as f64 {
                return false;
            }
            note.start_sample = position.round() as u64;
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn quantize_midi(&self, grid_beats: f32, strength: f32) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::quantize(&mut events, grid_beats, strength);
        drop(events);
        result && self.quantize_scheduled_midi(grid_beats, strength)
    }

    pub fn quantize_midi_diagnostic_json(&self, grid_beats: f32, strength: f32) -> String {
        if !grid_beats.is_finite()
            || !(0.001..=16.0).contains(&grid_beats)
            || !strength.is_finite()
            || !(0.0..=1.0).contains(&strength)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_quantize",
                "retryable": false,
            })
            .to_string();
        }
        let Ok(mut events) = self.midi_events.lock() else {
            return serde_json::json!({
                "ok": false,
                "code": "midi_state_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        if !midi::MIDIOrchestrator::quantize(&mut events, grid_beats, strength) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_quantize_rejected",
                "retryable": false,
            })
            .to_string();
        }
        let event_count = events.len();
        drop(events);
        if !self.quantize_scheduled_midi(grid_beats, strength) {
            return serde_json::json!({
                "ok": false,
                "code": "scheduled_midi_quantize_rejected",
                "retryable": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "quantize_midi",
            "grid_beats": grid_beats,
            "strength": strength,
            "event_count": event_count,
        })
        .to_string()
    }

    pub fn humanize_midi(&self, timing_beats: f32, velocity: i16, seed: u64) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::humanize(&mut events, timing_beats, velocity, seed);
        drop(events);
        result && self.humanize_scheduled_midi(timing_beats, velocity, seed)
    }

    fn humanize_scheduled_midi(&self, timing_beats: f32, velocity: i16, mut seed: u64) -> bool {
        let max_offset = self.beats_to_samples(timing_beats.abs() as f64);
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let unit = ((seed >> 33) as f64 / (u32::MAX as f64)) * 2.0 - 1.0;
            let delta = (unit * max_offset as f64).round() as i128;
            let next = note.start_sample as i128 + delta;
            if next < 0 || next > u64::MAX as i128 {
                return false;
            }
            note.start_sample = next as u64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let velocity_delta = ((seed >> 33) % (velocity.unsigned_abs() as u64 + 1)) as i16;
            let signed_delta = if seed & 1 == 0 {
                velocity_delta
            } else {
                -velocity_delta
            };
            note.velocity = (note.velocity as i16 + signed_delta).clamp(1, 127) as u8;
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn humanize_midi_diagnostic_json(
        &self,
        timing_beats: f32,
        velocity: i16,
        seed: u64,
    ) -> String {
        let result = if !timing_beats.is_finite()
            || !(-4.0..=4.0).contains(&timing_beats)
            || !(-127..=127).contains(&velocity)
        {
            crate::bridge_error::BridgeError::new(
                "invalid_midi_humanize",
                "timing and velocity variation are out of range",
            )
        } else {
            match self.midi_events.lock() {
                Err(_) => crate::bridge_error::BridgeError::new(
                    "midi_state_unavailable",
                    "MIDI state lock is poisoned",
                )
                .retryable(true),
                Ok(mut events) => {
                    if midi::MIDIOrchestrator::humanize(&mut events, timing_beats, velocity, seed) {
                        drop(events);
                        if !self.humanize_scheduled_midi(timing_beats, velocity, seed) {
                            return serde_json::json!({"ok":false,"code":"scheduled_midi_humanize_rejected","retryable":false}).to_string();
                        }
                        return "{\"ok\":true,\"operation\":\"humanize_midi\"}".to_owned();
                    }
                    crate::bridge_error::BridgeError::new(
                        "midi_humanize_rejected",
                        "MIDI humanize operation was rejected",
                    )
                }
            }
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn comping_snapshot_json(&self) -> String {
        self.comping
            .lock()
            .ok()
            .and_then(|comp| serde_json::to_string(&*comp).ok())
            .unwrap_or_else(|| "{\"takes\":[],\"current_comp\":[]}".to_owned())
    }

    pub fn restore_comping_snapshot_json(&self, snapshot: &str) -> bool {
        let Ok(candidate) = serde_json::from_str::<comping::CompingOrchestrator>(snapshot) else {
            return false;
        };
        if !candidate.audit_comping()
            || candidate
                .takes
                .windows(2)
                .any(|pair| pair[0].id == pair[1].id)
        {
            return false;
        }
        let Ok(mut current) = self.comping.lock() else {
            return false;
        };
        *current = candidate;
        true
    }

    pub fn restore_comping_snapshot_diagnostic_json(&self, snapshot: &str) -> String {
        let candidate = match serde_json::from_str::<comping::CompingOrchestrator>(snapshot) {
            Ok(value) => value,
            Err(_) => {
                return serde_json::to_string(&crate::bridge_error::BridgeError::new(
                    "invalid_comping_snapshot_json",
                    "comping snapshot is not valid JSON",
                ))
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
            }
        };
        if !candidate.audit_comping()
            || candidate
                .takes
                .windows(2)
                .any(|pair| pair[0].id == pair[1].id)
        {
            return serde_json::to_string(&crate::bridge_error::BridgeError::new(
                "invalid_comping_snapshot",
                "comping snapshot failed ordering, overlap, or duplicate-take validation",
            ))
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Ok(mut current) = self.comping.lock() else {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "comping_state_unavailable",
                    "comping state lock is poisoned",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        };
        *current = candidate;
        "{\"ok\":true,\"operation\":\"restore_comping_snapshot\"}".to_owned()
    }

    fn comping_sidecar_path(path: &str) -> PathBuf {
        PathBuf::from(format!("{path}.comping.json"))
    }

    fn sidecar_temp_path(sidecar: &std::path::Path) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        PathBuf::from(format!(
            "{}.tmp-{}-{}",
            sidecar.display(),
            std::process::id(),
            nonce
        ))
    }

    fn save_comping_sidecar(&self, path: &str) -> bool {
        let sidecar = Self::comping_sidecar_path(path);
        let temporary = Self::sidecar_temp_path(&sidecar);
        let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        else {
            return false;
        };
        use std::io::Write;
        if file
            .write_all(self.comping_snapshot_json().as_bytes())
            .is_err()
            || file.sync_all().is_err()
        {
            let _ = std::fs::remove_file(&temporary);
            return false;
        }
        if std::fs::rename(&temporary, &sidecar).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return false;
        }
        sync_sidecar_parent(&sidecar)
    }

    fn load_comping_sidecar(&self, path: &str) -> bool {
        let sidecar = Self::comping_sidecar_path(path);
        let snapshot = match std::fs::read_to_string(sidecar) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return self.restore_comping_snapshot_json("{\"takes\":[],\"current_comp\":[]}");
            }
        };
        self.restore_comping_snapshot_json(&snapshot)
    }

    fn midi_sidecar_path(path: &str) -> PathBuf {
        PathBuf::from(format!("{path}.midi-events.json"))
    }

    fn save_midi_sidecar(&self, path: &str) -> bool {
        let sidecar = Self::midi_sidecar_path(path);
        let temporary = Self::sidecar_temp_path(&sidecar);
        let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        else {
            return false;
        };
        use std::io::Write;
        if file.write_all(self.midi_events_json().as_bytes()).is_err() || file.sync_all().is_err() {
            let _ = std::fs::remove_file(&temporary);
            return false;
        }
        if std::fs::rename(&temporary, &sidecar).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return false;
        }
        sync_sidecar_parent(&sidecar)
    }

    fn load_midi_sidecar(&self, path: &str) -> bool {
        let sidecar = Self::midi_sidecar_path(path);
        let snapshot = match std::fs::read_to_string(sidecar) {
            Ok(snapshot) => snapshot,
            Err(_) => "[]".to_owned(),
        };
        self.set_midi_events_json(&snapshot)
    }

    pub fn scan_preview_audio(&self, path: &str) -> anyhow::Result<usize> {
        self.preview_audio
            .lock()
            .map_err(|_| anyhow::anyhow!("preview audio lock poisoned"))?
            .scan(Path::new(path))
            .map_err(anyhow::Error::msg)
    }

    pub fn scan_preview_audio_diagnostic_json(&self, path: &str) -> String {
        let result = if path.is_empty() || path.contains('\0') {
            crate::bridge_error::BridgeError::new(
                "invalid_preview_path",
                "preview scan path is empty or invalid",
            )
        } else if !Path::new(path).is_dir() {
            crate::bridge_error::BridgeError::new(
                "preview_directory_not_found",
                "preview scan path is not a directory",
            )
            .retryable(true)
        } else {
            match self
                .preview_audio
                .lock()
                .map_err(|_| "preview audio lock poisoned".to_owned())
                .and_then(|mut runtime| runtime.scan(Path::new(path)))
            {
                Ok(count) => return format!("{{\"ok\":true,\"asset_count\":{count}}}"),
                Err(error) => crate::bridge_error::BridgeError::new("preview_scan_failed", error)
                    .retryable(true),
            }
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Returns the actual preview-library entries currently registered by the
    /// audio runtime. This is deliberately JSON so Slint, CLI, and external
    /// automation can consume the same control-plane catalog.
    pub fn preview_audio_catalog_json(&self) -> String {
        match self.preview_audio.lock() {
            Ok(runtime) => serde_json::json!({
                "ok": true,
                "assets": runtime.catalog(),
            })
            .to_string(),
            Err(_) => serde_json::json!({
                "ok": false,
                "code": "preview_audio_lock_poisoned",
                "assets": [],
            })
            .to_string(),
        }
    }

    pub fn preload_preview_audio(&self, id: u64) -> anyhow::Result<()> {
        self.preview_audio
            .lock()
            .map_err(|_| anyhow::anyhow!("preview audio lock poisoned"))?
            .preload(id)
            .map_err(anyhow::Error::msg)
    }

    pub fn preload_preview_audio_diagnostic_json(&self, id: u64) -> String {
        let result = match self
            .preview_audio
            .lock()
            .map_err(|_| "preview audio lock poisoned".to_owned())
            .and_then(|mut runtime| runtime.preload(id))
        {
            Ok(()) => return format!("{{\"ok\":true,\"asset_id\":{id}}}"),
            Err(error) if error == "audio asset not found" => {
                crate::bridge_error::BridgeError::new("preview_asset_not_found", error)
                    .object(format!("asset:{id}"))
            }
            Err(error) => crate::bridge_error::BridgeError::new("preview_preload_failed", error)
                .object(format!("asset:{id}"))
                .retryable(true),
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn register_preview_audio(&self, path: &str) -> anyhow::Result<u64> {
        self.preview_audio
            .lock()
            .map_err(|_| anyhow::anyhow!("preview audio lock poisoned"))?
            .register_file(Path::new(path))
            .map_err(anyhow::Error::msg)
    }

    pub fn assign_preview_drum_pad(&self, pad: usize, id: Option<u64>) -> bool {
        self.preview_audio
            .lock()
            .map(|mut runtime| runtime.assign_pad(pad, id))
            .unwrap_or(false)
    }

    pub fn assign_preview_drum_pad_diagnostic_json(&self, pad: usize, id: Option<u64>) -> String {
        let result = if pad >= 16 {
            crate::bridge_error::BridgeError::new(
                "invalid_preview_pad",
                "preview drum pad must be between 0 and 15",
            )
            .object(format!("preview-pad:{pad}"))
        } else if self
            .preview_audio
            .lock()
            .map(|mut runtime| runtime.assign_pad(pad, id))
            .unwrap_or(false)
        {
            return format!("{{\"ok\":true,\"pad\":{pad}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "preview_asset_not_found",
                "preview asset is not registered",
            )
            .object(format!("preview-pad:{pad}"))
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Loads a cached pad into the native engine and triggers a one-shot
    /// preview. File decoding remains outside the audio callback.
    pub fn trigger_preview_drum_pad(&self, pad: usize) -> anyhow::Result<()> {
        let (samples, source_rate) = self
            .preview_audio
            .lock()
            .map_err(|_| anyhow::anyhow!("preview audio lock poisoned"))?
            .pad_audio(pad)
            .ok_or_else(|| anyhow::anyhow!("preview drum pad is not assigned or loaded"))?;
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AudioEngine is unavailable"))?;
        engine.set_preview_sample(&samples, source_rate);
        engine.trigger_preview_sample();
        Ok(())
    }

    pub fn trigger_preview_drum_pad_diagnostic_json(&self, pad: usize) -> String {
        if pad >= 16 {
            return "{\"code\":\"invalid_preview_pad\",\"retryable\":false}".to_owned();
        }
        let result = match self
            .preview_audio
            .lock()
            .map_err(|_| "preview audio lock poisoned".to_owned())
            .and_then(|runtime| {
                runtime
                    .pad_audio(pad)
                    .ok_or_else(|| "preview drum pad is not assigned or loaded".to_owned())
            }) {
            Ok((samples, source_rate)) => {
                let Some(engine) = self.engine.as_ref() else {
                    return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
                };
                engine.set_preview_sample(&samples, source_rate);
                engine.trigger_preview_sample();
                return format!("{{\"ok\":true,\"pad\":{pad}}}");
            }
            Err(error) => crate::bridge_error::BridgeError::new("preview_pad_unavailable", error)
                .object(format!("preview-pad:{pad}")),
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Registers, decodes, and immediately previews a browser-selected file.
    /// All file I/O happens before the native engine receives the immutable
    /// sample snapshot.
    pub fn preview_audio_file(&self, path: &str) -> anyhow::Result<()> {
        let (samples, source_rate) = {
            let mut runtime = self
                .preview_audio
                .lock()
                .map_err(|_| anyhow::anyhow!("preview audio lock poisoned"))?;
            let id = runtime
                .register_file(Path::new(path))
                .map_err(anyhow::Error::msg)?;
            runtime
                .asset_audio(id)
                .ok_or_else(|| anyhow::anyhow!("decoded preview audio is empty"))?
        };
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AudioEngine is unavailable"))?;
        engine.set_preview_sample(&samples, source_rate);
        engine.trigger_preview_sample();
        Ok(())
    }

    // Transport
    pub fn set_playing(&self, p: bool) {
        if let Some(e) = self.engine.as_ref() {
            let _ = e.try_set_playing(p);
        }
    }

    pub fn try_set_playing(&self, p: bool) -> bool {
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.try_set_playing(p));
        if changed {
            self.publish_production_event(if p {
                crate::production_events::ProductionEvent::TransportStarted
            } else {
                crate::production_events::ProductionEvent::TransportStopped
            });
        }
        changed
    }

    pub fn set_playing_diagnostic_json(&self, playing: bool) -> String {
        if self.engine.as_ref().is_none() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.try_set_playing(playing) {
            return serde_json::json!({
                "ok": true,
                "operation": "set_playing",
                "playing": playing,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "transport_rejected",
            "retryable": true,
            "playing": playing,
        })
        .to_string()
    }
    pub fn set_loop(&self, enabled: bool) {
        if let Some(e) = self.engine.as_ref() {
            e.set_loop(enabled);
        }
    }

    pub fn set_loop_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_loop(enabled);
        serde_json::json!({"ok": true, "operation": "set_loop", "enabled": enabled}).to_string()
    }

    pub fn set_metronome_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_metronome_enabled(enabled);
        serde_json::json!({
            "ok": true,
            "operation": "set_metronome",
            "enabled": engine.is_metronome_enabled(),
        })
        .to_string()
    }

    pub fn set_cycle_range_diagnostic_json(
        &self,
        start_sample: u64,
        end_sample: u64,
        enabled: bool,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if start_sample >= end_sample || !engine.set_cycle_range(start_sample, end_sample, enabled)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_cycle_range",
                "retryable": false,
                "start_sample": start_sample,
                "end_sample": end_sample,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_cycle_range",
            "enabled": enabled,
            "start_sample": start_sample,
            "end_sample": end_sample,
        })
        .to_string()
    }
    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_playing())
    }
    pub fn get_playhead(&self) -> u64 {
        self.engine.as_ref().map_or(0, |e| e.get_playhead())
    }
    pub fn samples_to_beats(&self, samples: u64) -> f64 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.samples_to_beats(samples))
    }
    pub fn beats_to_samples(&self, beats: f64) -> u64 {
        if !beats.is_finite() || beats <= 0.0 {
            return 0;
        }
        self.engine
            .as_ref()
            .map_or(0, |e| e.beats_to_samples(beats))
    }
    pub fn get_tempo_events(&self) -> Vec<f64> {
        self.engine
            .as_ref()
            .map_or_else(Vec::new, |e| e.get_tempo_events().into_iter().collect())
    }
    pub fn get_time_signature_events(&self) -> Vec<f64> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.get_time_signature_events().into_iter().collect()
        })
    }
    pub fn set_time_signature_event(&self, beat: f64, numerator: u8, denominator: u8) -> bool {
        if !beat.is_finite() || beat < 0.0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_time_signature_event(beat, numerator, denominator))
    }
    pub fn set_time_signature_event_diagnostic_json(
        &self,
        beat: f64,
        numerator: u8,
        denominator: u8,
    ) -> String {
        if !beat.is_finite()
            || beat < 0.0
            || numerator == 0
            || numerator > 32
            || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_time_signature",
                "retryable": false,
            })
            .to_string();
        }
        if self.set_time_signature_event(beat, numerator, denominator) {
            return serde_json::json!({
                "ok": true,
                "beat": beat,
                "numerator": numerator,
                "denominator": denominator,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "time_signature_rejected",
            "retryable": false,
        })
        .to_string()
    }
    pub fn set_tempo_event(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() {
            return false;
        }
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|e| e.set_tempo_event(beat, bpm, ramp));
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn set_tempo_event_diagnostic_json(&self, beat: f64, bpm: f64, ramp: bool) -> String {
        if !beat.is_finite() || !bpm.is_finite() {
            return "{\"code\":\"non_finite_tempo_event\",\"retryable\":false}".to_owned();
        }
        if beat < 0.0 || !(20.0..=300.0).contains(&bpm) {
            return "{\"code\":\"tempo_event_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_tempo_event(beat, bpm, ramp) {
            return serde_json::json!({"ok": true, "beat": beat, "bpm": bpm, "ramp": ramp})
                .to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "tempo_event_rejected",
                "tempo event was rejected",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn remove_tempo_event(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|e| e.remove_tempo_event(beat));
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn remove_tempo_event_diagnostic_json(&self, beat: f64) -> String {
        if !beat.is_finite() || beat < 0.0 {
            return "{\"code\":\"invalid_tempo_position\",\"retryable\":false}".to_owned();
        }
        if self.remove_tempo_event(beat) {
            return serde_json::json!({"ok": true, "beat": beat}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "tempo_event_not_found",
                "tempo event was not found",
            )
            .object(format!("tempo:{beat}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn move_tempo_event(&self, from_beat: f64, to_beat: f64) -> bool {
        if !from_beat.is_finite() || !to_beat.is_finite() || from_beat <= 0.0 || to_beat <= 0.0 {
            return false;
        }
        let changed = self
            .engine
            .as_ref()
            .is_some_and(|e| e.move_tempo_event(from_beat, to_beat));
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn get_tempo(&self) -> f32 {
        self.engine.as_ref().map_or(0.0, |e| e.get_tempo())
    }
    pub fn set_tempo(&self, bpm: f32) -> bool {
        if !bpm.is_finite() {
            return false;
        }
        self.engine.as_ref().is_some_and(|e| e.set_tempo(bpm))
    }
    pub fn set_tempo_diagnostic_json(&self, bpm: f32) -> String {
        if !bpm.is_finite() {
            return "{\"code\":\"non_finite_tempo\",\"retryable\":false}".to_owned();
        }
        if !(20.0..=300.0).contains(&bpm) {
            return "{\"code\":\"tempo_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_tempo(bpm) {
            return serde_json::json!({"ok": true, "bpm": bpm}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new("tempo_rejected", "tempo update was rejected")
                .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn master_gain(&self) -> f32 {
        self.engine
            .as_ref()
            .map_or(1.0, |engine| engine.get_master_gain())
    }

    pub fn set_master_gain(&self, value: f32) -> bool {
        value.is_finite()
            && (0.0..=2.0).contains(&value)
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.set_master_gain(value))
    }

    pub fn set_master_gain_diagnostic_json(&self, value: f32) -> String {
        if !value.is_finite() || !(0.0..=2.0).contains(&value) {
            return "{\"code\":\"master_gain_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_master_gain(value) {
            return serde_json::json!({"ok": true, "master_gain": value}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "master_gain_rejected",
                "master gain update was rejected",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn add_control_room_speaker(&self, name: &str, gain: f32) -> bool {
        !name.trim().is_empty()
            && name.len() <= 128
            && gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.add_control_room_speaker(name, gain))
    }
    pub fn select_control_room_speaker(&self, index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.select_control_room_speaker(index))
    }
    pub fn remove_control_room_speaker(&self, index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.remove_control_room_speaker(index))
    }
    pub fn set_control_room_speaker_gain(&self, index: u32, gain: f32) -> bool {
        gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.set_control_room_speaker_gain(index, gain))
    }
    pub fn set_control_room_speaker_enabled(&self, index: u32, enabled: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_control_room_speaker_enabled(index, enabled))
    }
    pub fn upsert_control_room_cue(&self, id: u32, gain: f32, enabled: bool) -> bool {
        id != 0
            && gain.is_finite()
            && (0.0..=4.0).contains(&gain)
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.upsert_control_room_cue(id, gain, enabled))
    }
    pub fn remove_control_room_cue(&self, id: u32) -> bool {
        id != 0
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.remove_control_room_cue(id))
    }
    pub fn set_control_room_cue_enabled(&self, id: u32, enabled: bool) -> bool {
        id != 0
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.set_control_room_cue_enabled(id, enabled))
    }
    pub fn control_room_cue_gain(&self, id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.control_room_cue_gain(id))
    }
    pub fn control_room_validate(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.control_room_validate())
    }
    pub fn set_control_room_dim(&self, enabled: bool) {
        if let Some(engine) = self.engine.as_ref() {
            engine.set_control_room_dim(enabled);
        }
    }
    pub fn set_control_room_talkback(&self, enabled: bool, gain: f32) {
        if gain.is_finite() {
            if let Some(engine) = self.engine.as_ref() {
                engine.set_control_room_talkback(enabled, gain.clamp(0.0, 4.0));
            }
        }
    }
    pub fn control_room_dimmed(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.control_room_dimmed())
    }
    pub fn control_room_talkback_enabled(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.control_room_talkback_enabled())
    }
    pub fn control_room_monitor_gain(&self) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.control_room_monitor_gain())
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
        let mut previous_time = f64::NEG_INFINITY;
        for triple in packed_points.as_chunks::<3>().0 {
            if triple[0] < 0.0
                || triple[0].fract() != 0.0
                || triple[0] <= previous_time
                || !(0.0..=1.0).contains(&triple[1])
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

    pub fn add_plugin(&self, track_id: u32, plugin_type: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.add_plugin(track_id, plugin_type))
    }
    pub fn add_plugin_diagnostic_json(&self, track_id: u32, plugin_type: u32) -> String {
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.add_plugin(track_id, plugin_type) {
            return format!(
                "{{\"ok\":true,\"track_id\":{track_id},\"plugin_type\":{plugin_type}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "plugin_add_rejected",
                format!("plugin type {plugin_type} could not be added to track {track_id}"),
            )
            .object(format!("track:{track_id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn plugin_state(&self, track_id: u32, plugin_index: u32) -> Vec<u8> {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return Vec::new();
        };
        self.engine.as_ref().map_or_else(Vec::new, |engine| {
            engine
                .get_plugin_state(track_id, plugin_index)
                .into_iter()
                .collect()
        })
    }

    pub fn set_plugin_state(&self, track_id: u32, plugin_index: u32, state: &[u8]) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        if state.len() > 4 * 1024 * 1024 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_plugin_state(track_id, plugin_index, state))
    }

    /// Structured state restore result. The legacy bool API remains for
    /// bindings that cannot carry diagnostics, while new callers can
    /// distinguish oversize, unavailable, and native restore failures.
    pub fn set_plugin_state_diagnostic(
        &self,
        track_id: u32,
        plugin_index: u32,
        state: &[u8],
    ) -> crate::PluginStateDiagnostic {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return crate::PluginStateDiagnostic {
                ok: false,
                code: "project_transaction_unavailable",
                message: "project transaction lock is unavailable".into(),
            };
        };
        if state.len() > 4 * 1024 * 1024 {
            return crate::PluginStateDiagnostic {
                ok: false,
                code: "state_oversize",
                message: "plugin state exceeds the 4 MiB limit".into(),
            };
        }
        let Some(engine) = self.engine.as_ref() else {
            return crate::PluginStateDiagnostic {
                ok: false,
                code: "engine_unavailable",
                message: "audio engine is unavailable".into(),
            };
        };
        if engine.set_plugin_state(track_id, plugin_index, state) {
            crate::PluginStateDiagnostic {
                ok: true,
                code: "ok",
                message: String::new(),
            }
        } else {
            crate::PluginStateDiagnostic {
                ok: false,
                code: "native_restore_failed",
                message: format!(
                    "native plugin state restore failed for track {track_id}, plugin {plugin_index}"
                ),
            }
        }
    }

    pub fn remove_plugin(&self, track_id: u32, plugin_index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.remove_plugin(track_id, plugin_index))
    }

    pub fn move_plugin(&self, track_id: u32, from_index: u32, to_index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.move_plugin(track_id, from_index, to_index))
    }
    pub fn remove_plugin_diagnostic_json(&self, track_id: u32, plugin_index: u32) -> String {
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.remove_plugin(track_id, plugin_index) {
            return format!(
                "{{\"ok\":true,\"track_id\":{track_id},\"plugin_index\":{plugin_index}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "plugin_not_found_or_rejected",
                format!("plugin {plugin_index} could not be removed from track {track_id}"),
            )
            .object(format!("track:{track_id}/plugin:{plugin_index}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn add_sandboxed_plugin(&self, track_id: u32, path: &str) -> bool {
        let admission = Self::plugin_path_admission(path);
        if matches!(
            admission,
            "missing-path"
                | "invalid-path"
                | "unsupported-extension"
                | "quarantined"
                | "unadmitted-plugin"
        ) {
            return false;
        }
        if admission != "builtin" && crate::plugin_catalog::is_quarantined_path(path) {
            return false;
        }
        if admission != "builtin" {
            let candidate = std::path::Path::new(path);
            let Ok(metadata) = std::fs::symlink_metadata(candidate) else {
                return false;
            };
            if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
                return false;
            }
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.add_sandboxed_plugin(track_id, path))
    }

    /// Structured admission result for automation callers. Keep the legacy
    /// bool API for UI bindings, but do not make CLI/LLM callers infer a
    /// missing plugin, unsupported format, and engine failure from `false`.
    pub fn add_sandboxed_plugin_diagnostic_json(&self, track_id: u32, path: &str) -> String {
        let result = if self.engine.is_null() {
            crate::bridge_error::BridgeError::new(
                "engine_unavailable",
                "audio engine is not available",
            )
            .retryable(true)
        } else if path.trim().is_empty() {
            crate::bridge_error::BridgeError::new("invalid_plugin_path", "plugin path is empty")
        } else {
            let admission = Self::plugin_path_admission(path);
            if matches!(admission, "unsupported-extension" | "invalid-path") {
                crate::bridge_error::BridgeError::new(
                    "unsupported_plugin",
                    format!("plugin admission rejected: {admission}"),
                )
            } else if admission == "missing-path" {
                crate::bridge_error::BridgeError::new(
                    "plugin_not_found",
                    "plugin path does not exist",
                )
            } else if admission == "quarantined" {
                crate::bridge_error::BridgeError::new(
                    "plugin_quarantined",
                    "plugin binary is quarantined and must be explicitly re-enabled",
                )
            } else if admission == "unadmitted-plugin" {
                crate::bridge_error::BridgeError::new(
                    "plugin_path_not_admitted",
                    "plugin must be discovered and supported by the packaged worker",
                )
            } else if admission != "builtin" {
                let candidate = std::path::Path::new(path);
                match std::fs::symlink_metadata(candidate) {
                    Err(_) => crate::bridge_error::BridgeError::new(
                        "plugin_not_found",
                        "plugin path does not exist",
                    ),
                    Ok(metadata)
                        if metadata.file_type().is_symlink()
                            || (!metadata.is_file() && !metadata.is_dir()) =>
                    {
                        crate::bridge_error::BridgeError::new(
                            "plugin_path_rejected",
                            "plugin path must be a regular file or bundle directory",
                        )
                    }
                    Ok(_) if crate::plugin_catalog::is_quarantined_path(path) => {
                        crate::bridge_error::BridgeError::new(
                            "plugin_quarantined",
                            "plugin binary is quarantined and must be explicitly re-enabled",
                        )
                    }
                    Ok(_) if self.add_sandboxed_plugin(track_id, path) => {
                        return "{\"ok\":true}".to_owned();
                    }
                    Ok(_) => crate::bridge_error::BridgeError::new(
                        "plugin_admission_failed",
                        "native plugin admission failed",
                    )
                    .retryable(true),
                }
            } else if self.add_sandboxed_plugin(track_id, path) {
                return "{\"ok\":true}".to_owned();
            } else {
                crate::bridge_error::BridgeError::new(
                    "plugin_admission_failed",
                    "native plugin admission failed",
                )
                .retryable(true)
            }
        };
        serde_json::to_string(&result).unwrap_or_else(|_| {
            "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
        })
    }

    pub fn last_sandbox_failure_text(&self, track_id: u32) -> String {
        self.engine.as_ref().map_or_else(
            || "engine-unavailable".to_owned(),
            |engine| engine.get_last_sandbox_failure_text(track_id).to_string(),
        )
    }

    pub fn set_plugin_bypass(&self, track_id: u32, plugin_index: u32, bypassed: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_plugin_bypass(track_id, plugin_index, bypassed))
    }
    pub fn set_plugin_bypass_diagnostic_json(
        &self,
        track_id: u32,
        plugin_index: u32,
        bypassed: bool,
    ) -> String {
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.set_plugin_bypass(track_id, plugin_index, bypassed) {
            return format!(
                "{{\"ok\":true,\"track_id\":{track_id},\"plugin_index\":{plugin_index}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "plugin_bypass_rejected",
                format!("plugin {plugin_index} bypass update rejected for track {track_id}"),
            )
            .object(format!("track:{track_id}/plugin:{plugin_index}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn get_plugin_bypass(&self, track_id: u32, plugin_index: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.get_plugin_bypass(track_id, plugin_index))
    }

    pub fn set_macro_value(&self, macro_index: u32, value: f32) {
        if value.is_finite() {
            if let Some(engine) = self.engine.as_ref() {
                engine.set_macro_value(macro_index, value.clamp(0.0, 1.0));
            }
            let mappings = self
                .macro_mappings
                .lock()
                .map(|items| {
                    items
                        .iter()
                        .filter(|item| item.macro_index as u32 == macro_index)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for mapping in mappings {
                let Some((track_id, plugin_index)) =
                    parse_plugin_instance_id(&mapping.target_instance_id)
                else {
                    continue;
                };
                let Ok(parameter_id) = mapping.target_parameter_id.parse::<u32>() else {
                    continue;
                };
                let normalized = if mapping.invert { 1.0 - value } else { value };
                let curved = if mapping.curve.abs() < f32::EPSILON {
                    normalized
                } else {
                    (normalized + mapping.curve * normalized * (1.0 - normalized)).clamp(0.0, 1.0)
                };
                let mapped = mapping.min + curved * (mapping.max - mapping.min);
                let _ = self.engine.as_ref().is_some_and(|engine| {
                    engine.set_plugin_parameter_without_undo(
                        track_id,
                        plugin_index,
                        parameter_id,
                        mapped,
                    )
                });
            }
        }
    }

    pub fn add_macro_mapping(
        &self,
        mapping: crate::project_contracts::MacroMappingContract,
    ) -> anyhow::Result<()> {
        if mapping.macro_index >= 128
            || mapping.mapping_id.trim().is_empty()
            || mapping.target_instance_id.trim().is_empty()
            || mapping.target_parameter_id.trim().is_empty()
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
            || !(-1.0..=1.0).contains(&mapping.curve)
        {
            return Err(anyhow::anyhow!("invalid macro mapping"));
        }
        let mut mappings = self
            .macro_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("Macro mapping lock poisoned"))?;
        if mappings
            .iter()
            .any(|item| item.mapping_id == mapping.mapping_id)
        {
            return Err(anyhow::anyhow!("macro mapping already exists"));
        }
        if mappings.len() >= 4096 {
            return Err(anyhow::anyhow!("macro mapping limit exceeded"));
        }
        mappings.push(mapping);
        Ok(())
    }

    pub fn remove_macro_mapping(&self, mapping_id: &str) -> bool {
        let Ok(mut mappings) = self.macro_mappings.lock() else {
            return false;
        };
        let before = mappings.len();
        mappings.retain(|mapping| mapping.mapping_id != mapping_id);
        mappings.len() != before
    }

    pub fn macro_mappings_json(&self) -> String {
        serde_json::to_string(
            &*self
                .macro_mappings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn add_midi_learn_mapping(
        &self,
        mapping: crate::project_contracts::MidiLearnMappingContract,
    ) -> anyhow::Result<()> {
        if mapping.mapping_id.trim().is_empty()
            || mapping.device_id.trim().is_empty()
            || mapping.channel > 15
            || mapping.controller > 16_383
            || mapping.target_instance_id.trim().is_empty()
            || mapping.target_parameter_id.trim().is_empty()
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
        {
            return Err(anyhow::anyhow!("invalid MIDI learn mapping"));
        }
        let mut mappings = self
            .midi_learn_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI mapping lock poisoned"))?;
        if mappings
            .iter()
            .any(|item| item.mapping_id == mapping.mapping_id)
        {
            return Err(anyhow::anyhow!("MIDI mapping already exists"));
        }
        if mappings.len() >= 4096 {
            return Err(anyhow::anyhow!("MIDI mapping limit exceeded"));
        }
        if let Ok(mut acquired) = self.midi_pickup_acquired.lock() {
            acquired.remove(&mapping.mapping_id);
        }
        mappings.push(mapping);
        Ok(())
    }

    pub fn remove_midi_learn_mapping(&self, mapping_id: &str) -> bool {
        let Ok(mut mappings) = self.midi_learn_mappings.lock() else {
            return false;
        };
        let before = mappings.len();
        mappings.retain(|mapping| mapping.mapping_id != mapping_id);
        if let Ok(mut acquired) = self.midi_pickup_acquired.lock() {
            acquired.remove(mapping_id);
        }
        mappings.len() != before
    }

    pub fn midi_learn_mappings_json(&self) -> String {
        serde_json::to_string(
            &*self
                .midi_learn_mappings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn control_snapshot_json(&self) -> String {
        let (cycle_start, cycle_end, cycle_enabled, metronome_enabled) = self
            .engine
            .as_ref()
            .map(|engine| {
                (
                    engine.cycle_start(),
                    engine.cycle_end(),
                    engine.is_loop_enabled(),
                    engine.is_metronome_enabled(),
                )
            })
            .unwrap_or((0, 0, false, false));
        let engine_capabilities = serde_json::from_str::<serde_json::Value>(
            &self.engine_capabilities_json(),
        )
        .unwrap_or_else(
            |_| serde_json::json!({"schema":"aura.engine-capabilities.v1","capabilities":[]}),
        );
        serde_json::json!({
            "project_generation": self.project_generation(),
            "audio_generation": self.audio_config_generation(),
            "tempo_events": self.get_tempo_events(),
            "time_signature_events": self.get_time_signature_events(),
            "cycle_range": {"enabled": cycle_enabled, "start_sample": cycle_start, "end_sample": cycle_end},
            "metronome": {"enabled": metronome_enabled},
            "master_gain": self.master_gain(),
            "track_stacks": serde_json::from_str::<serde_json::Value>(&self.track_stacks_json()).unwrap_or(serde_json::json!([])),
            "markers": serde_json::from_str::<serde_json::Value>(&self.markers_json()).unwrap_or(serde_json::json!([])),
            "macro_mappings": serde_json::from_str::<serde_json::Value>(&self.macro_mappings_json()).unwrap_or(serde_json::json!([])),
            "midi_learn_mappings": serde_json::from_str::<serde_json::Value>(&self.midi_learn_mappings_json()).unwrap_or(serde_json::json!([])),
            "midi_notes": serde_json::to_value(
                &*self.scheduled_midi_notes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
            ).unwrap_or(serde_json::json!([])),
            "chord_track": serde_json::from_str::<serde_json::Value>(&self.chord_track_json()).unwrap_or(serde_json::json!([])),
            "runtime_health": self.runtime_health_snapshot(),
            "engine_capabilities": engine_capabilities,
        }).to_string()
    }

    /// Machine-readable contract for UI authors, AI clients, and alternate
    /// frontends. This deliberately reports the public engine surface rather
    /// than pretending that unverified vendor integrations are complete.
    pub fn engine_capabilities_json(&self) -> String {
        serde_json::json!({
            "schema": "aura.engine-capabilities.v1",
            "engine_api": "1.0",
            "rt_safe_audio_thread": true,
            "capabilities": [
                {"id":"audio_note_editing","status":"implemented","apis":["analyzeAudioNoteSegments","upsertAudioNoteSegment","warpAudioNoteSegment"]},
                {"id":"group_phase_coherent_warp","status":"implemented","apis":["quantizeAudioGroup"]},
                {"id":"chord_track","status":"implemented","apis":["chord_track_json","set_chord_event"]},
                {"id":"midi_logical_editor","status":"implemented","apis":["execute_midi_logical_editor"]},
                {"id":"direct_routing","status":"implemented","apis":["fanoutStereo","fanoutPlanar"]},
                {"id":"control_room","status":"implemented","apis":["control_room_monitor_snapshot_json","set_control_room_dim"]},
                {"id":"audio_device_io","status":"implemented","apis":["list_audio_devices_json","select_audio_device","apply_audio_config"]},
                {"id":"mix_snapshots","status":"implemented","apis":["save_snapshot","restore_snapshot"]},
                {"id":"plugin_sandbox_catalog","status":"implemented","apis":["installed_plugin_catalog_json"]},
                {"id":"vst3_sandbox_worker","status":"implemented","apis":["add_sandboxed_plugin","process_sandboxed_plugin_block","process_sandboxed_plugin_midi_block"]},
                {"id":"export_queue","status":"implemented","apis":["enqueue_export","export_queue_snapshot_json"]},
                {"id":"vst3_clap_native_gui","status":"integration_required"},
                {"id":"ara2_partner_integration","status":"integration_required"},
                {"id":"windows_asio_hardware","status":"integration_required"}
            ]
        }).to_string()
    }

    pub fn set_macro_value_diagnostic_json(&self, macro_index: u32, value: f32) -> String {
        if macro_index >= 128 || !value.is_finite() || !(0.0..=1.0).contains(&value) {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_macro_value",
                "macro index or value is outside the supported range",
            )
            .object(format!("macro:{macro_index}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(audio) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        audio.set_macro_value(macro_index, value);
        format!("{{\"ok\":true,\"macro_index\":{macro_index},\"value\":{value}}}")
    }

    /// Binds a physical MIDI CC to one of the native Macro controls. The
    /// native side keeps the target in fixed atomic storage, so incoming CC
    /// events do not acquire a Rust mutex or allocate on the audio boundary.
    pub fn bind_midi_cc_to_macro_diagnostic_json(
        &self,
        channel: u8,
        controller: u8,
        macro_index: u32,
        minimum: f32,
        maximum: f32,
        curve: f32,
        pickup: bool,
    ) -> String {
        if channel > 16
            || controller > 127
            || macro_index >= 128
            || !minimum.is_finite()
            || !maximum.is_finite()
            || minimum > maximum
            || !curve.is_finite()
            || !(-1.0..=1.0).contains(&curve)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_macro_mapping",
                "retryable": false,
            })
            .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if !engine.bind_midi_cc_to_macro(
            channel,
            controller,
            macro_index,
            minimum,
            maximum,
            curve,
            pickup,
        ) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_macro_mapping_rejected",
                "retryable": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "bind_midi_cc_to_macro",
            "channel": channel,
            "controller": controller,
            "macro_index": macro_index,
            "minimum": minimum,
            "maximum": maximum,
            "curve": curve,
            "pickup": pickup,
        })
        .to_string()
    }

    pub fn bind_midi_cc14_to_macro_diagnostic_json(
        &self,
        channel: u8,
        controller: u16,
        macro_index: u32,
        minimum: f32,
        maximum: f32,
        curve: f32,
        pickup: bool,
    ) -> String {
        if channel > 16
            || controller > 16_383
            || macro_index >= 128
            || !minimum.is_finite()
            || !maximum.is_finite()
            || minimum > maximum
            || !curve.is_finite()
            || !(-1.0..=1.0).contains(&curve)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_macro_mapping",
                "retryable": false,
            })
            .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if !engine.bind_midi_cc14_to_macro(
            channel,
            controller,
            macro_index,
            minimum,
            maximum,
            curve,
            pickup,
        ) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_macro_mapping_rejected",
                "retryable": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "bind_midi_cc14_to_macro",
            "channel": channel,
            "controller": controller,
            "macro_index": macro_index,
            "minimum": minimum,
            "maximum": maximum,
            "curve": curve,
            "pickup": pickup,
        })
        .to_string()
    }

    /// Feeds a decoded hardware CC event into the native mapping snapshot.
    /// Device adapters can call this after validating their own transport
    /// framing; the native path remains allocation-free.
    pub fn handle_midi_cc(&self, channel: u8, controller: u8, value: u8) {
        self.handle_midi_cc_from_device("", channel, controller, value);
    }

    /// Device-aware variant used by hardware adapters. An empty device ID
    /// preserves the legacy wildcard behavior for older MIDI bridges.
    pub fn handle_midi_cc_from_device(
        &self,
        device_id: &str,
        channel: u8,
        controller: u8,
        value: u8,
    ) {
        if channel <= 15 && controller <= 127 && value <= 127 {
            if let Some(engine) = self.engine.as_ref() {
                engine.handle_midi_cc(channel, controller, value);
            }
            self.apply_persisted_midi_mapping(
                device_id,
                channel,
                controller as u16,
                f32::from(value) / 127.0,
            );
        }
    }

    /// Handles a full 14-bit controller value. The mapping contract already
    /// uses u16 controller IDs so NRPN and paired-CC adapters can preserve
    /// their resolution instead of truncating to seven bits.
    pub fn handle_midi_cc14_from_device(
        &self,
        device_id: &str,
        channel: u8,
        controller: u16,
        value: u16,
    ) {
        if channel > 15 || controller > 16_383 || value > 16_383 {
            return;
        }
        if let Some(engine) = self.engine.as_ref() {
            engine.handle_midi_cc14(channel, controller, value);
        }
        self.apply_persisted_midi_mapping(
            device_id,
            channel,
            controller,
            f32::from(value) / 16_383.0,
        );
    }

    fn apply_persisted_midi_mapping(
        &self,
        device_id: &str,
        channel: u8,
        controller: u16,
        normalized: f32,
    ) {
        let mappings = self
            .midi_learn_mappings
            .lock()
            .map(|items| {
                items
                    .iter()
                    .filter(|mapping| {
                        (device_id.is_empty() || mapping.device_id == device_id)
                            && mapping.channel == channel
                            && mapping.controller == controller
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut pickup_acquired = self.midi_pickup_acquired.lock().ok();
        for mapping in mappings {
            let Some((track_id, plugin_index)) =
                parse_plugin_instance_id(&mapping.target_instance_id)
            else {
                continue;
            };
            let Ok(parameter_id) = mapping.target_parameter_id.parse::<u32>() else {
                continue;
            };
            if mapping.pickup
                && pickup_acquired
                    .as_ref()
                    .is_some_and(|acquired| !acquired.contains(&mapping.mapping_id))
            {
                let Some(engine) = self.engine.as_ref() else {
                    continue;
                };
                let current = engine.get_plugin_parameter(track_id, plugin_index, parameter_id);
                let span = (mapping.max - mapping.min).abs().max(0.0001);
                let target = mapping.min + span * normalized;
                if !current.is_finite() || (current - target).abs() > span * 0.02 {
                    continue;
                }
                if let Some(acquired) = pickup_acquired.as_mut() {
                    acquired.insert(mapping.mapping_id.clone());
                }
            }
            let shaped = if mapping.curve.abs() < f32::EPSILON {
                normalized
            } else if mapping.curve > 0.0 {
                normalized.powf(1.0 + mapping.curve * 3.0)
            } else {
                1.0 - (1.0 - normalized).powf(1.0 - mapping.curve * 3.0)
            };
            let mapped = mapping.min + shaped.clamp(0.0, 1.0) * (mapping.max - mapping.min);
            let _ = self.engine.as_ref().is_some_and(|engine| {
                engine.set_plugin_parameter_without_undo(
                    track_id,
                    plugin_index,
                    parameter_id,
                    mapped,
                )
            });
        }
    }

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
    pub fn set_playhead(&self, pos: u64) {
        if let Some(e) = self.engine.as_ref() {
            e.set_playhead(pos);
            let sample_rate = self.get_sample_rate().round().max(1.0) as u64;
            let clock = crate::production_timeline::MasterClock {
                tick: crate::production_timeline::MasterTick(pos as u128),
                ticks_per_second: sample_rate,
            };
            self.publish_production_event(
                crate::production_events::ProductionEvent::PlayheadMoved { clock },
            );
        }
    }

    pub fn set_playhead_diagnostic_json(&self, pos: u64) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_playhead(pos);
        serde_json::json!({
            "ok": true,
            "operation": "set_playhead",
            "playhead": engine.get_playhead(),
        })
        .to_string()
    }
    pub fn set_test_tone(&self, enabled: bool) {
        if let Some(e) = self.engine.as_ref() {
            e.set_test_tone(enabled);
        }
    }

    pub fn set_test_tone_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_test_tone(enabled);
        serde_json::json!({"ok": true, "operation": "set_test_tone", "enabled": enabled})
            .to_string()
    }

    // Actions
    pub fn set_track_fader(&self, tid: u32, val: f32) {
        self.set_volume(tid, val);
    }
    pub fn set_track_fader_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_volume_diagnostic_json(tid, val)
    }
    pub fn set_track_pan(&self, tid: u32, val: f32) {
        self.set_pan(tid, val);
    }
    pub fn set_track_pan_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_pan_diagnostic_json(tid, val)
    }

    pub fn set_volume(&self, tid: u32, val: f32) -> bool {
        let changed = self.set_track_volume_with_stack(tid, val);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TrackGainChanged {
                    track_id: tid,
                    gain: val,
                },
            );
        }
        changed
    }

    pub fn set_volume_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_track_scalar_diagnostic_json("volume", tid, val, |engine, id, value| {
            engine.set_track_volume(id, value)
        })
    }

    pub fn set_eq_diagnostic_json(
        &self,
        tid: u32,
        low_band: f32,
        low_cut: f32,
        high_band: f32,
        high_cut: f32,
    ) -> String {
        if [low_band, low_cut, high_band, high_cut]
            .iter()
            .any(|value| !value.is_finite())
        {
            return "{\"code\":\"invalid_parameter\",\"message\":\"EQ parameters must be finite\"}"
                .to_owned();
        }
        let ok = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_eq(tid, low_band, low_cut, high_band, high_cut));
        if ok {
            serde_json::json!({"ok":true,"operation":"set_eq","track_id":tid,"low_band":low_band,"low_cut":low_cut,"high_band":high_band,"high_cut":high_cut}).to_string()
        } else {
            serde_json::json!({"code":"eq_rejected","track_id":tid}).to_string()
        }
    }

    /// Apply a bounded gain-staging correction in dB to the current fader
    /// value. This is reversible through the command transaction/undo layer.
    pub fn apply_gain_staging_diagnostic_json(&self, tid: u32, gain_db: f32) -> String {
        if !gain_db.is_finite() || !(-24.0..=24.0).contains(&gain_db) {
            return serde_json::json!({"code":"invalid_parameter","message":"gain staging correction must be within -24..=24 dB","track_id":tid}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\"}".to_owned();
        };
        let before = engine.get_track_volume(tid);
        let after = (before * 10.0_f32.powf(gain_db / 20.0)).clamp(0.0, 2.0);
        if !engine.set_track_volume(tid, after) {
            return serde_json::json!({"code":"track_not_found","track_id":tid}).to_string();
        }
        serde_json::json!({"ok":true,"operation":"apply_gain_staging","track_id":tid,"gain_db":gain_db,"before":before,"after":after}).to_string()
    }
    pub fn set_track_delay_samples(&self, tid: u32, samples: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_delay_samples(tid, samples))
    }
    pub fn track_delay_samples(&self, tid: u32) -> Option<u32> {
        self.engine
            .as_ref()
            .map(|engine| engine.get_track_delay_samples(tid))
    }
    pub fn set_track_delay_samples_diagnostic_json(&self, tid: u32, samples: u32) -> String {
        if samples > 8192 {
            return serde_json::json!({"code":"invalid_parameter","message":"track delay must be within 0..=8192 samples","track_id":tid}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"code":"engine_unavailable","retryable":true}).to_string();
        };
        if engine.set_track_delay_samples(tid, samples) {
            serde_json::json!({"ok":true,"field":"track_delay_samples","track_id":tid,"value":samples}).to_string()
        } else {
            serde_json::json!({"code":"track_not_found_or_rejected","track_id":tid}).to_string()
        }
    }
    pub fn track_volume(&self, tid: u32) -> Option<f32> {
        self.engine.as_ref().and_then(|engine| {
            let value = engine.get_track_volume(tid);
            value.is_finite().then_some(value)
        })
    }
    pub fn set_pan(&self, tid: u32, val: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_pan(tid, val))
    }

    pub fn set_pan_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_track_scalar_diagnostic_json("pan", tid, val, |engine, id, value| {
            engine.set_track_pan(id, value)
        })
    }

    fn set_track_scalar_diagnostic_json<F>(
        &self,
        field: &str,
        tid: u32,
        val: f32,
        apply: F,
    ) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine, u32, f32) -> bool,
    {
        if !val.is_finite() {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_parameter",
                format!("track {field} must be finite"),
            )
            .object(format!("track:{tid}"));
            return serde_json::to_string(&error).unwrap_or_else(|_| {
                "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
            });
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine, tid, val) {
            return format!("{{\"ok\":true,\"field\":\"{field}\",\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                format!("track {tid} rejected {field} update"),
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result).unwrap_or_else(|_| {
            "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
        })
    }

    /// Track type codes are part of the native bridge contract. Keep the
    /// legacy numeric entry point for ABI compatibility, but reject unknown
    /// values before they reach the engine and expose named helpers below.
    pub fn add_track(&self, t_type: u32) -> u32 {
        if t_type > 4 {
            return 0;
        }
        let id = self.engine.as_ref().map_or(0, |e| e.add_track(t_type));
        if id != 0 {
            self.publish_production_event(crate::production_events::ProductionEvent::TrackAdded {
                track_id: id,
                name: format!("Track {id}"),
            });
        }
        // Beginner-friendly default routing: newly created signal tracks are
        // sent to the first available Bus. Bus/Vocal tracks remain un-routed
        // so the operation cannot create a cycle or a self-route.
        if id != 0 && matches!(t_type, 0..=2) {
            if let Some(engine) = self.engine.as_ref() {
                if let Ok(layout) =
                    serde_json::from_str::<serde_json::Value>(&engine.get_project_layout_json())
                {
                    if let Some(bus_id) = layout
                        .get("tracks")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|tracks| {
                            tracks.iter().find(|track| {
                                track.get("type").and_then(serde_json::Value::as_str) == Some("Bus")
                            })
                        })
                        .and_then(|track| track.get("id").and_then(serde_json::Value::as_u64))
                    {
                        let _ = engine.set_route_gain(id, bus_id as u32, 1.0, true);
                    }
                }
            }
        }
        id
    }

    pub fn add_audio_track(&self) -> u32 {
        self.add_track(0)
    }

    pub fn add_midi_track(&self) -> u32 {
        self.add_track(1)
    }

    pub fn add_instrument_track(&self) -> u32 {
        self.add_track(2)
    }

    pub fn add_bus_track(&self) -> u32 {
        self.add_track(3)
    }

    /// Aux channels share the native Bus signal type, but remain a distinct
    /// named operation for UI and automation clients.
    pub fn add_aux_track(&self) -> u32 {
        let id = self.add_bus_track();
        if id != 0 {
            if let Ok(mut aux_ids) = self.aux_track_ids.lock() {
                aux_ids.insert(id);
            }
            // Keep Aux creation visibly distinct in the arrange/mixer views
            // even though the native signal path uses the shared BusTrack
            // implementation.
            let _ = self.set_track_name(id, &format!("Aux {}", id));
        }
        id
    }

    pub fn add_vocal_track(&self) -> u32 {
        self.add_track(4)
    }

    pub fn aux_track_ids_json(&self) -> String {
        let mut ids = self
            .aux_track_ids
            .lock()
            .map(|value| value.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        ids.sort_unstable();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn restore_aux_track_ids_json(&self, snapshot: &str) -> bool {
        let Ok(ids) = serde_json::from_str::<Vec<u32>>(snapshot) else {
            return false;
        };
        if ids.contains(&0) || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return false;
        }
        let Ok(mut current) = self.aux_track_ids.lock() else {
            return false;
        };
        *current = ids.into_iter().collect();
        true
    }

    pub fn add_vca_group(&self, group_id: u32, gain: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.add_vca_group(group_id, gain))
    }

    pub fn assign_track_to_vca(&self, track_id: u32, group_id: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.assign_track_to_vca(track_id, group_id))
    }

    pub fn set_vca_group_gain(&self, group_id: u32, gain: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_vca_group_gain(group_id, gain))
    }

    pub fn get_vca_track_gain(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(1.0, |engine| engine.get_vca_track_gain(track_id))
    }

    pub fn get_vca_snapshot_json(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.get_vca_snapshot_json().to_string())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn clear_vca_groups(&self) {
        if let Some(engine) = self.engine.as_ref() {
            engine.clear_vca_groups();
        }
    }

    /// Publishes a native immutable freeze snapshot for a track.  This is a
    /// real graph operation, not merely a project flag: the native engine
    /// renders and owns the frozen audio before returning success.
    pub fn freeze_track_diagnostic_json(&self, track_id: u32, total_samples: u64) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({
                "ok": false,
                "code": "engine_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if total_samples == 0 || sample_rate == 0 {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_freeze_range",
                "retryable": false,
            })
            .to_string();
        }
        if engine.freeze_track(track_id, total_samples, sample_rate) {
            return serde_json::json!({
                "ok": true,
                "operation": "freeze_track",
                "track_id": track_id,
                "total_samples": total_samples,
                "sample_rate": sample_rate,
                "audio_generation": engine.get_audio_config_generation(),
                "frozen": true,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "freeze_track_rejected",
            "track_id": track_id,
            "retryable": true,
        })
        .to_string()
    }

    pub fn freeze_track_to_project_end_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok":false,"code":"engine_unavailable","retryable":true})
                .to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if sample_rate == 0 || !engine.freeze_track_to_project_end(track_id, sample_rate) {
            return serde_json::json!({"ok":false,"code":"freeze_track_rejected","track_id":track_id}).to_string();
        }
        serde_json::json!({"ok":true,"operation":"freeze_track","track_id":track_id,"sample_rate":sample_rate,"scope":"project_end"}).to_string()
    }

    pub fn freeze_track_to_project_end(&self, track_id: u32) -> bool {
        self.engine.as_ref().is_some_and(|engine| {
            let sample_rate = engine.get_sample_rate().round() as u32;
            sample_rate != 0 && engine.freeze_track_to_project_end(track_id, sample_rate)
        })
    }

    pub fn unfreeze_track(&self, track_id: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.unfreeze_track(track_id))
    }

    pub fn freeze_track_to_file_diagnostic_json(
        &self,
        track_id: u32,
        path: &str,
        total_samples: u64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable", "retryable": true}).to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if path.trim().is_empty() || total_samples == 0 || sample_rate == 0 {
            return serde_json::json!({"ok": false, "code": "invalid_freeze_request", "retryable": false}).to_string();
        }
        if engine.freeze_track_to_file(track_id, path, total_samples, sample_rate) {
            return serde_json::json!({
                "ok": true,
                "operation": "freeze_track",
                "track_id": track_id,
                "path": path,
                "total_samples": total_samples,
                "sample_rate": sample_rate,
                "audio_generation": engine.get_audio_config_generation(),
                "frozen": true,
            })
            .to_string();
        }
        serde_json::json!({"ok": false, "code": "freeze_track_file_rejected", "track_id": track_id, "retryable": true}).to_string()
    }

    pub fn unfreeze_track_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({
                "ok": false,
                "code": "engine_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        if engine.unfreeze_track(track_id) {
            return serde_json::json!({
                "ok": true,
                "operation": "unfreeze_track",
                "track_id": track_id,
                "frozen": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "track_not_found",
            "track_id": track_id,
            "retryable": false,
        })
        .to_string()
    }

    pub fn track_freeze_status_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable"}).to_string();
        };
        serde_json::json!({
            "ok": true,
            "operation": "track_freeze_status",
            "track_id": track_id,
            "frozen": engine.is_track_frozen(track_id),
        })
        .to_string()
    }
    pub fn add_track_diagnostic_json(&self, t_type: u32) -> String {
        if t_type > 4 {
            return serde_json::json!({
                "ok": false,
                "code": "unsupported_track_type",
                "track_type": t_type,
                "supported_track_types": ["Audio", "Midi", "Instrument", "Bus", "Vocal"],
                "retryable": false,
            })
            .to_string();
        }
        if self.engine.is_null() {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "engine_unavailable",
                    "audio engine unavailable",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let track_id = self.add_track(t_type);
        if track_id != 0 {
            return format!("{{\"ok\":true,\"track_id\":{track_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_creation_failed",
                "track allocation failed",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn remove_track(&self, id: u32) -> bool {
        let removed = self.engine.as_ref().is_some_and(|e| e.remove_track(id));
        if !removed {
            return false;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            for stack in stacks.iter_mut() {
                stack.member_track_ids.retain(|member| *member != id);
            }
            stacks.retain(|stack| !stack.member_track_ids.is_empty());
        }
        if let Ok(mut bases) = self.track_stack_base_volumes.lock() {
            bases.remove(&id);
        }
        let _ = self.apply_track_stack_gains();
        true
    }
    pub fn remove_track_diagnostic_json(&self, id: u32) -> String {
        if self.engine.is_null() {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "engine_unavailable",
                    "audio engine unavailable",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if self.remove_track(id) {
            return format!("{{\"ok\":true,\"track_id\":{id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track removal rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn set_track_name(&self, id: u32, name: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_name(id, name))
    }

    pub fn set_track_name_diagnostic_json(&self, id: u32, name: &str) -> String {
        if id == 0 || name.trim().is_empty() || name.contains('\0') || name.len() > 256 {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_track_name",
                "track id or name is invalid",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.set_track_name(id, name) {
            return format!("{{\"ok\":true,\"track_id\":{id}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track rename was rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn new_project(&self) {
        if let Some(e) = self.engine.as_ref() {
            e.new_project();
        }
        if let Ok(mut chords) = self.chord_track.lock() {
            chords.clear();
        }
        if let Ok(mut history) = self.chord_history.lock() {
            history.clear();
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
        if let Ok(mut history) = self.midi_lyric_history.lock() {
            history.clear();
        }
        self.reset_markers();
    }
    pub fn duplicate_track(&self, id: u32) -> u32 {
        self.engine.as_ref().map_or(0, |e| e.duplicate_track(id))
    }

    pub fn duplicate_track_diagnostic_json(&self, id: u32) -> String {
        if id == 0 {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_track_id",
                    "track id must be non-zero",
                )
                .object(format!("track:{id}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let new_id = engine.duplicate_track(id);
        if new_id != 0 {
            return format!("{{\"ok\":true,\"source_track_id\":{id},\"track_id\":{new_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track duplication was rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn add_region(&self, tid: u32, path: &str, start: f64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.add_region(tid, path, start))
    }

    pub fn add_region_diagnostic_json(&self, tid: u32, path: &str, start: f64) -> String {
        if tid == 0 || path.is_empty() || path.contains('\0') || !start.is_finite() || start < 0.0 {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_region_input",
                "region target, path, or start position is invalid",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !Path::new(path).is_file() {
            let error = crate::bridge_error::BridgeError::new(
                "region_audio_not_found",
                "region audio path is not a regular file",
            )
            .object(path)
            .retryable(true);
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.add_region(tid, path, start) {
            return format!("{{\"ok\":true,\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "region_add_rejected",
                "native engine rejected region insertion",
            )
            .object(format!("track:{tid}"))
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn add_region_at_beat(&self, tid: u32, path: &str, beat: f64) -> bool {
        self.add_region(tid, path, self.beats_to_samples(beat.max(0.0)) as f64)
    }
    pub fn replace_region_audio(&self, tid: u32, rid: u32, path: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.replace_region_audio(tid, rid, path))
    }

    pub fn replace_region_audio_diagnostic_json(&self, tid: u32, rid: u32, path: &str) -> String {
        if tid == 0 || rid == 0 || path.is_empty() || path.contains('\0') {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_region_input",
                "track, region, or audio path is invalid",
            )
            .object(format!("track:{tid}/region:{rid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !Path::new(path).is_file() {
            let error = crate::bridge_error::BridgeError::new(
                "region_audio_not_found",
                "replacement audio path is not a regular file",
            )
            .object(path)
            .retryable(true);
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.replace_region_audio(tid, rid, path) {
            return format!("{{\"ok\":true,\"track_id\":{tid},\"region_id\":{rid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "region_replace_rejected",
                "native engine rejected audio replacement",
            )
            .object(format!("track:{tid}/region:{rid}"))
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
