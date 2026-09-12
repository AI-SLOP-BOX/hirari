impl AuraCore {
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
