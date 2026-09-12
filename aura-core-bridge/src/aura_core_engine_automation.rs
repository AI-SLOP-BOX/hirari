impl AuraCore {
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
            "status_vocabulary": ["implemented", "integration_required", "host_bind_required", "provider_bind_required", "protocol_only", "discovery_only", "sdk_or_host_bind_required", "sdk_or_backend_bind_required"],
            "capabilities": [
                {"id":"audio_note_editing","status":"implemented","verification":"engine_unit_contract","apis":["analyzeAudioNoteSegments","upsertAudioNoteSegment","warpAudioNoteSegment"]},
                {"id":"group_phase_coherent_warp","status":"implemented","verification":"engine_unit_contract","apis":["quantizeAudioGroup"]},
                {"id":"chord_track","status":"implemented","verification":"engine_unit_contract","apis":["chord_track_json","set_chord_event"]},
                {"id":"midi_logical_editor","status":"implemented","verification":"engine_unit_contract","apis":["execute_midi_logical_editor"]},
                {"id":"direct_routing","status":"implemented","verification":"engine_unit_contract","apis":["fanoutStereo","fanoutPlanar"]},
                {"id":"control_room","status":"implemented","verification":"engine_unit_contract","apis":["control_room_monitor_snapshot_json","set_control_room_dim"]},
                {"id":"audio_device_io","status":"integration_required","verification":"host_device_matrix_required","apis":["list_audio_devices_json","select_audio_device","apply_audio_config"]},
                {"id":"mix_snapshots","status":"implemented","verification":"engine_unit_contract","apis":["save_snapshot","restore_snapshot"]},
                {"id":"plugin_sandbox_catalog","status":"discovery_only","verification":"installed_plugins_required","apis":["installed_plugin_catalog_json"]},
                {"id":"vst3_sandbox_worker","status":"host_bind_required","verification":"sdk_and_vendor_matrix_required","apis":["add_sandboxed_plugin","process_sandboxed_plugin_block","process_sandboxed_plugin_midi_block"]},
                {"id":"export_queue","status":"implemented","verification":"engine_unit_contract","apis":["enqueue_export_job_json","export_queue_snapshot_json","execute_export_queue_json","enqueue_advanced_export_job_json","advanced_export_snapshot_json","execute_advanced_export_json"]},
                {"id":"async_waveform_decode","status":"implemented","verification":"engine_unit_contract","apis":["queue_region_waveform","poll_region_waveform","region_waveform_pending"]},
                {"id":"midi_vibrato_rate","status":"implemented","verification":"engine_unit_contract","apis":["set_midi_note_vibrato_rate_without_undo","midi_notes_json"]},
                {"id":"measured_hrtf_kernel","status":"integration_required","apis":["set_hrtf_kernel_json","set_hrtf_kernel","clear_hrtf_kernel"]},
                {"id":"native_plugin_editor_host","status":"host_bind_required","apis":["plugin_editor_capability_diagnostic_json","open_plugin_native_editor_json","close_plugin_native_editor_json"]},
                {"id":"vst3_clap_native_gui","status":"sdk_or_host_bind_required"},
                {"id":"ara2_protocol_endpoint","status":"protocol_only","verification":"partner_host_not_verified","apis":["ara2_bind_document_json","ara2_request_analysis_json","ara2_set_analysis_state_json","ara2_set_note_segments_json","ara2_document_snapshot_json","ara2_read_region_audio_json"]},
                {"id":"ara2_partner_integration","status":"provider_bind_required"},
                {"id":"windows_asio_hardware","status":"sdk_or_backend_bind_required"}
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
}
