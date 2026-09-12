impl AuraCore {
    /// Structured project hydration result for CLI/UI callers.  The legacy
    /// `load_project_v2` Result remains available to Rust callers, while this
    /// boundary preserves whether a failed load was a plugin-state timeout,
    /// a missing asset, or a generic validation error.  A failed hydration
    /// has already attempted native rollback before this method returns.
    pub fn load_project_v2_diagnostic_json(&self, path: &str) -> String {
        match self.load_project_v2(path) {
            Ok(()) => serde_json::json!({
                "ok": true,
                "path": path,
                "project_generation": self.project_generation(),
                "audio_generation": self.audio_config_generation(),
            })
            .to_string(),
            Err(error) => {
                let message = error.to_string();
                let code = if message.contains("state-timeout") {
                    "plugin_state_timeout"
                } else if message.contains("state-checksum-mismatch") {
                    "plugin_state_checksum_mismatch"
                } else if message.contains("state-version-unsupported") {
                    "plugin_state_version_unsupported"
                } else if message.contains("missing") {
                    "project_asset_missing"
                } else {
                    "project_hydration_failed"
                };
                let retryable = matches!(code, "plugin_state_timeout" | "project_asset_missing");
                let bridge_error = crate::bridge_error::BridgeError::new(code, message)
                    .retryable(retryable)
                    .object(format!("project:{path}"))
                    .at_generation(self.project_generation());
                serde_json::json!({
                    "ok": false,
                    "path": path,
                    "error": bridge_error,
                    "project_generation": self.project_generation(),
                    "audio_generation": self.audio_config_generation(),
                })
                .to_string()
            }
        }
    }
}
