impl HirariCore {
    pub fn set_spatial_position(&self, tid: u32, x: f32, y: f32, z: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_spatial_position(tid, x, y, z))
    }

    /// Installs a bounded measured HRTF impulse-response pair on one track.
    /// The copy happens on the control plane; the realtime panner only reads
    /// its fixed-size kernel. Empty/mismatched/non-finite data is rejected by
    /// the native kernel contract.
    pub fn set_hrtf_kernel(&self, tid: u32, left: Vec<f32>, right: Vec<f32>) -> bool {
        if left.is_empty() || left.len() != right.len() || left.len() > 128
            || left.iter().chain(right.iter()).any(|sample| !sample.is_finite())
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_hrtf_kernel(tid, left, right))
    }

    pub fn clear_hrtf_kernel(&self, tid: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.clear_hrtf_kernel(tid))
    }

    /// Loads one measured HRTF pair from a provider payload. The payload is
    /// intentionally simple (`{"left":[...],"right":[...]}`) so an app can
    /// adapt SOFA/database lookups without coupling the core to a file format.
    pub fn set_hrtf_kernel_json(&self, tid: u32, payload: &str) -> String {
        let value = match serde_json::from_str::<serde_json::Value>(payload) {
            Ok(value) => value,
            Err(_) => return r#"{"ok":false,"code":"invalid_hrtf_payload","retryable":false}"#.into(),
        };
        let to_samples = |name: &str| -> Option<Vec<f32>> {
            value.get(name)?.as_array()?.iter().map(|sample| {
                let value = sample.as_f64()? as f32;
                value.is_finite().then_some(value)
            }).collect()
        };
        let Some(left) = to_samples("left") else {
            return r#"{"ok":false,"code":"invalid_hrtf_left","retryable":false}"#.into();
        };
        let Some(right) = to_samples("right") else {
            return r#"{"ok":false,"code":"invalid_hrtf_right","retryable":false}"#.into();
        };
        if self.set_hrtf_kernel(tid, left.clone(), right.clone()) {
            serde_json::json!({"ok":true,"track":tid,"taps":left.len()}).to_string()
        } else {
            r#"{"ok":false,"code":"hrtf_kernel_rejected","retryable":false}"#.into()
        }
    }
    pub fn set_track_armed(&self, tid: u32, armed: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_armed(tid, armed))
    }

    pub fn set_track_input_monitor(&self, tid: u32, enabled: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_input_monitor(tid, enabled))
    }

    pub fn set_track_input_monitor_channels(
        &self, tid: u32, enabled: bool, left: u32, right: u32,
    ) -> bool {
        left < 32 && right < 32 && self.engine.as_ref()
            .is_some_and(|e| e.set_track_input_monitor_channels(tid, enabled, left, right))
    }

    pub fn set_mute(&self, tid: u32, m: bool) {
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.set_track_mute(tid, m);
        }
    }
    pub fn set_mute_diagnostic_json(&self, tid: u32, m: bool) -> String {
        self.set_track_toggle_diagnostic_json("mute", tid, |engine, id| {
            engine.set_track_mute(id, m)
        })
    }
    pub fn set_solo(&self, tid: u32, s: bool) {
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.set_track_solo(tid, s);
        }
    }
    pub fn set_solo_diagnostic_json(&self, tid: u32, s: bool) -> String {
        self.set_track_toggle_diagnostic_json("solo", tid, |engine, id| {
            engine.set_track_solo(id, s)
        })
    }

    fn set_track_toggle_diagnostic_json<F>(&self, field: &str, tid: u32, apply: F) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine, u32) -> bool,
    {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine, tid) {
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

    pub fn set_phase_invert(&self, tid: u32, inverted: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_phase_invert(tid, inverted))
    }

    pub fn execute_vocal_remover(&self, tid: u32) {
        if let Some(e) = self.engine.as_ref() {
            let _ = e.execute_vocal_remover(tid);
        }
    }

    pub fn execute_vocal_remover_diagnostic_json(&self, tid: u32) -> String {
        if tid == 0 {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_track_id",
                    "track id must be non-zero",
                )
                .object(format!("track:{tid}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.execute_vocal_remover(tid) {
            return format!(
                "{{\"ok\":true,\"operation\":\"execute_vocal_remover\",\"track_id\":{tid}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "vocal remover requires an existing stereo track",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_articulation_map(&self, tid: u32, map: String) {
        // The native engine stores the map identity, not its display name.
        let mut hash = 2166136261u32;
        for byte in map.as_bytes() {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(16777619);
        }
        if let Some(e) = self.engine.as_ref() {
            let _ = e.set_articulation_map(tid, hash);
        }
    }

    pub fn set_midi_expression_map(
        &self,
        tid: u32,
        mut entries: Vec<crate::project_contracts::ExpressionMapEntry>,
        edited_articulation_id: u8,
    ) -> bool {
        if tid == 0 || entries.len() > 255 {
            return false;
        }
        let mut seen = std::collections::HashSet::with_capacity(entries.len());
        let mut seen_names = std::collections::HashSet::with_capacity(entries.len());
        let mut packed = Vec::with_capacity(entries.len() * 6);
        let mut json_entries = Vec::with_capacity(entries.len());
        for entry in &mut entries {
            if entry.name.trim().is_empty() {
                entry.name = format!("Articulation {}", entry.articulation_id);
            }
            if entry.articulation_id == 0 || entry.name.len() > 128 || entry.name.contains('\0') ||
                entry.channel > 15 || entry.group > 16 ||
                (entry.group == 0 && !entry.transition_off_outputs.is_empty()) ||
                entry.outputs.len() + entry.off_outputs.len() + entry.transition_off_outputs.len()
                    + usize::from(entry.group != 0) > 16 ||
                !seen_names.insert(entry.name.trim().to_ascii_lowercase()) ||
                entry.outputs.iter().chain(&entry.off_outputs)
                    .chain(&entry.transition_off_outputs).any(|output| !output.validate()) ||
                !seen.insert(entry.articulation_id) {
                return false;
            }
            if entry.group != 0 {
                packed.extend([entry.articulation_id as u32, entry.channel as u32, 16, entry.group as u32, 0, 0]);
            }
            for (output, off_phase) in entry.outputs.iter().map(|output| (output, false))
                .chain(entry.off_outputs.iter().map(|output| (output, true))) {
                use crate::project_contracts::ExpressionMapOutput as Output;
                let (base_kind, a, b, c) = match output {
                    Output::KeySwitch { note, velocity, length_ticks } => (
                        1, *note as u32, *velocity as u32, *length_ticks,
                    ),
                    Output::ProgramChange { bank_msb, bank_lsb, program } => (
                        2, *program as u32, bank_msb.unwrap_or(255) as u32, bank_lsb.unwrap_or(255) as u32,
                    ),
                    Output::ControlChange { controller, value } => (3, *controller as u32, *value as u32, 0),
                    Output::ChannelPressure { value } => (4, *value as u32, 0, 0),
                    Output::PitchBend { value } => {
                        let bend = (*value as i32 + 8192) as u32;
                        (5, bend & 0x7f, (bend >> 7) & 0x7f, 0)
                    }
                };
                let kind = base_kind + if off_phase { 5 } else { 0 };
                packed.extend([entry.articulation_id as u32, entry.channel as u32, kind, a, b, c]);
            }
            for output in &entry.transition_off_outputs {
                use crate::project_contracts::ExpressionMapOutput as Output;
                let (base_kind, a, b, c) = match output {
                    Output::KeySwitch { note, velocity, length_ticks } =>
                        (1, *note as u32, *velocity as u32, *length_ticks),
                    Output::ProgramChange { bank_msb, bank_lsb, program } =>
                        (2, *program as u32, bank_msb.unwrap_or(255) as u32,
                            bank_lsb.unwrap_or(255) as u32),
                    Output::ControlChange { controller, value } => (3, *controller as u32, *value as u32, 0),
                    Output::ChannelPressure { value } => (4, *value as u32, 0, 0),
                    Output::PitchBend { value } => {
                        let bend = (*value as i32 + 8192) as u32;
                        (5, bend & 0x7f, (bend >> 7) & 0x7f, 0)
                    }
                };
                packed.extend([entry.articulation_id as u32, entry.channel as u32,
                    base_kind + 10, a, b, c]);
            }
            json_entries.push(entry.clone());
        }
        let Ok(json) = serde_json::to_string(&json_entries) else { return false };
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_expression_map(tid, packed, json, u32::from(edited_articulation_id))
        })
    }

    /// Stores a validated track-local rich expression map in the native
    /// project layout. MIDI playback continues to use the separately lowered
    /// 1.0 articulation table until all rich slot transition semantics are
    /// supported by the real-time event path.
    pub fn set_midi_expression_map_pro(
        &self,
        tid: u32,
        map: crate::expression_map::ExpressionMapPro,
    ) -> bool {
        if tid == 0 || !map.validate() {
            return false;
        }
        let Ok(entries) = map.to_midi_expression_entries() else { return false };
        let Ok(json) = map.to_json() else { return false };
        let Some(engine) = self.engine.as_ref() else { return false };
        self.begin_undo_transaction("Import MIDI Expression Map");
        if !self.set_midi_expression_map(tid, entries, 0) ||
            !engine.set_expression_map_pro(tid, json) {
            let _ = self.abort_undo_transaction();
            return false;
        }
        self.end_undo_transaction()
    }

    pub fn set_articulation_map_diagnostic_json(&self, tid: u32, map: &str) -> String {
        if tid == 0 || map.trim().is_empty() || map.contains('\0') {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_articulation_map",
                    "track id or articulation map is invalid",
                )
                .object(format!("track:{tid}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let mut hash = 2166136261u32;
        for byte in map.as_bytes() {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(16777619);
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.set_articulation_map(tid, hash) {
            return format!(
                "{{\"ok\":true,\"operation\":\"set_articulation_map\",\"track_id\":{tid}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "articulation map target was rejected",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn execute_mixing_advice(&self, _title: String) {
        self.execute_auto_mixing();
    }

    pub fn execute_mixing_advice_diagnostic_json(&self, title: String) -> String {
        let title = title.trim();
        if title.is_empty() {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_mixing_advice_title",
                "mixing advice title must not be empty",
            )
            .at_generation(self.project_generation());
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.execute_auto_mixing() {
            return serde_json::json!({
                "ok": true,
                "operation": "execute_mixing_advice",
                "title": title,
                "generation": self.project_generation(),
            })
            .to_string();
        } else {
            crate::bridge_error::BridgeError::new(
                "project_empty_or_rejected",
                "mixing advice requires at least one track",
            )
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_track_eq(&self, tid: u32, lb: f32, lc: f32, hb: f32, hc: f32) {
        if let Some(e) = self.engine.as_ref() {
            e.set_track_eq(tid, lb, lc, hb, hc);
        }
    }
}
