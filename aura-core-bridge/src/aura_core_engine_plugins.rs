impl AuraCore {
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

    /// Structured plugin-state read result. The legacy byte-vector API cannot
    /// distinguish an empty preset from an unavailable or missing plugin.
    pub fn plugin_state_diagnostic_json(&self, track_id: u32, plugin_index: u32) -> String {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return serde_json::json!({
                "ok": false,
                "code": "project_transaction_unavailable"
            })
            .to_string();
        };
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable"}).to_string();
        };
        let state = engine.get_plugin_state(track_id, plugin_index);
        if state.is_empty() {
            return serde_json::json!({
                "ok": false,
                "code": "plugin_state_unavailable",
                "track_id": track_id,
                "plugin_index": plugin_index
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "track_id": track_id,
            "plugin_index": plugin_index,
            "state": state
        })
        .to_string()
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
}
