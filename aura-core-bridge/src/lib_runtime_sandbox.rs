impl AuraCore {
    pub fn sandbox_statuses(&self) -> Vec<SandboxStatus> {
        self.engine.as_ref().map_or_else(Vec::new, |engine| {
            decode_sandbox_statuses(&engine.get_sandbox_statuses())
        })
    }

    pub fn sandbox_failure_kind(&self, code: u8) -> SandboxFailureKind {
        SandboxFailureKind::from_code(code)
    }

    pub fn last_sandbox_failure(&self, track_id: u32) -> u8 {
        self.engine.as_ref().map_or(0, |engine| {
            engine
                .get_last_sandbox_failure(track_id)
                .min(u8::MAX as u32) as u8
        })
    }

    /// Stable diagnostic envelope for one sandboxed plugin instance.
    pub fn sandbox_failure_diagnostic_json(&self, track_id: u32) -> String {
        let failure_code = self.last_sandbox_failure(track_id);
        let kind = self.sandbox_failure_kind(failure_code);
        let error = if matches!(kind, SandboxFailureKind::None) {
            None
        } else {
            Some(
                crate::bridge_error::BridgeError::new(
                    format!("sandbox_{}", kind.code()),
                    self.last_sandbox_failure_text(track_id),
                )
                .retryable(kind.retryable())
                .object(format!("track:{track_id}"))
                .at_generation(self.project_generation()),
            )
        };
        serde_json::json!({
            "ok": error.is_none(),
            "track_id": track_id,
            "failure_code": failure_code,
            "failure_kind": kind.code(),
            "message": error.as_ref().map(|value| value.message.as_str()),
            "error": error,
            "audio_generation": self.audio_config_generation(),
        })
        .to_string()
    }

    pub fn sandbox_plugin_paths(&self) -> Vec<String> {
        self.engine.as_ref().map_or_else(Vec::new, |engine| {
            engine.get_sandbox_plugin_paths().into_iter().collect()
        })
    }

    /// Reads a bounded plugin state blob on the control thread. The audio
    /// callback never enters this IPC path; callers use it for project save,
    /// restart recovery, and integration tests.
    pub fn sandbox_plugin_state(&self, track_id: u32, sandbox_index: u32) -> Vec<u8> {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return Vec::new();
        };
        self.engine.as_ref().map_or_else(Vec::new, |engine| {
            engine
                .get_sandbox_plugin_state(track_id, sandbox_index)
                .into_iter()
                .collect()
        })
    }

    pub fn set_sandbox_plugin_state(
        &self,
        track_id: u32,
        sandbox_index: u32,
        state: &[u8],
    ) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_sandbox_plugin_state(track_id, sandbox_index, state))
    }

    pub fn sandbox_plugin_state_diagnostic(
        &self,
        track_id: u32,
        sandbox_index: u32,
        state: &[u8],
    ) -> SandboxStateDiagnostic {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return SandboxStateDiagnostic {
                ok: false,
                code: 6,
                message: "project-transaction-busy".into(),
            };
        };
        if state.len() > 4 * 1024 * 1024 {
            return SandboxStateDiagnostic {
                ok: false,
                code: 1,
                message: "state-oversize".into(),
            };
        }
        let Some(engine) = self.engine.as_ref() else {
            return SandboxStateDiagnostic {
                ok: false,
                code: 5,
                message: "state-unavailable".into(),
            };
        };
        let ok = engine.set_sandbox_plugin_state(track_id, sandbox_index, state);
        let code = engine.get_sandbox_plugin_state_error(track_id, sandbox_index);
        let message = engine
            .get_sandbox_plugin_state_error_text(track_id, sandbox_index)
            .to_string();
        SandboxStateDiagnostic { ok, code, message }
    }

    pub fn restart_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.restart_sandboxed_plugin(track_id, sandbox_index))
    }

    pub fn retry_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.retry_sandboxed_plugin(track_id, sandbox_index))
    }

    /// Structured recovery result for CLI/UI callers. The legacy bool methods
    /// remain available to bindings that only need a success flag.
    pub fn recover_sandboxed_plugin_diagnostic_json(
        &self,
        track_id: u32,
        sandbox_index: u32,
        restart: bool,
    ) -> String {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return serde_json::json!({
                "ok": false,
                "code": "project_transaction_busy",
                "retryable": true,
                "track_id": track_id,
                "plugin_index": sandbox_index,
            })
            .to_string();
        };
        let Some(snapshot) = self.sandbox_snapshots().into_iter().find(|snapshot| {
            snapshot.track_id == track_id && snapshot.plugin_index == sandbox_index
        }) else {
            return serde_json::json!({
                "ok": false,
                "code": "sandbox_not_found",
                "retryable": false,
                "track_id": track_id,
                "plugin_index": sandbox_index,
                "project_generation": self.project_generation(),
                "audio_generation": self.audio_config_generation(),
            })
            .to_string();
        };
        let ok = if restart {
            self.engine
                .as_ref()
                .is_some_and(|engine| engine.restart_sandboxed_plugin(track_id, sandbox_index))
        } else if snapshot.can_retry {
            self.engine
                .as_ref()
                .is_some_and(|engine| engine.retry_sandboxed_plugin(track_id, sandbox_index))
        } else {
            false
        };
        let after = self
            .sandbox_snapshots()
            .into_iter()
            .find(|current| current.track_id == track_id && current.plugin_index == sandbox_index);
        if ok {
            return serde_json::json!({
                "ok": true,
                "restarted": restart,
                "recovery_mode": after.as_ref().map(|value| value.recovery_mode),
                "alive": after.as_ref().map(|value| value.alive),
                "failure": after.as_ref().map(|value| value.failure),
                "track_id": track_id,
                "plugin_index": sandbox_index,
                "project_generation": self.project_generation(),
                "audio_generation": self.audio_config_generation(),
            })
            .to_string();
        }
        let code = if !snapshot.can_retry && !restart {
            "sandbox_not_retryable"
        } else if snapshot.recovery_mode != 0 {
            "sandbox_recovery_failed"
        } else {
            "sandbox_restart_failed"
        };
        serde_json::json!({
            "ok": false,
            "code": code,
            "retryable": true,
            "failure": snapshot.failure,
            "recovery_mode": snapshot.recovery_mode,
            "alive": snapshot.alive,
            "track_id": track_id,
            "plugin_index": sandbox_index,
            "project_generation": self.project_generation(),
            "audio_generation": self.audio_config_generation(),
        })
        .to_string()
    }

    pub fn process_sandboxed_plugin_midi_block(
        &self,
        track_id: u32,
        sandbox_index: u32,
        frames: u32,
        midi_data: &[u8],
    ) -> Vec<u8> {
        self.engine.as_ref().map_or_else(Vec::new, |engine| {
            engine
                .process_sandboxed_plugin_midi_block(track_id, sandbox_index, frames, midi_data)
                .into_iter()
                .collect()
        })
    }

    pub fn sandbox_snapshots(&self) -> Vec<SandboxSnapshot> {
        let statuses = self.sandbox_statuses();
        let paths = self.sandbox_plugin_paths();
        statuses
            .into_iter()
            .enumerate()
            .map(|(index, status)| {
                let display_name = paths
                    .get(index)
                    .map(|path| safe_plugin_display_name(path))
                    .unwrap_or_else(|| "Plugin".to_string());
                SandboxSnapshot {
                    track_id: status.track_id,
                    plugin_index: status.plugin_index,
                    display_name,
                    alive: status.alive,
                    can_retry: status.can_retry,
                    failure: status.failure,
                    dropped_output_midi: status.dropped_output_midi,
                    mailbox_overruns: status.mailbox_overruns,
                    recovery_mode: status.recovery_mode,
                }
            })
            .collect()
    }

    pub fn maintain_sandboxes(&self, auto_restart: bool) -> u32 {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return 0;
        };
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.maintain_sandboxed_plugins(auto_restart))
    }

    /// Drains control-thread AU watchdog edges. The audio callback only sets
    /// an atomic edge; this poll is the sole place that consumes it.
    pub fn take_watchdog_trips(&self) -> u32 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.take_watchdog_trips())
    }

    /// Attempts recovery for every failed sandbox and returns the number of
    /// workers the native host reported as recovered.
    pub fn recover_sandboxed_plugins(&self) -> u32 {
        self.maintain_sandboxes(true)
    }

    /// Reads all high-rate runtime indicators through one native snapshot.
    /// The UI can poll this once per frame without touching engine internals.
    pub fn runtime_health_snapshot(&self) -> RuntimeHealthSnapshot {
        let raw: Vec<f32> = self
            .engine
            .as_ref()
            .map(|engine| engine.get_runtime_health_v().into_iter().collect())
            .unwrap_or_default();
        let value = |index: usize| raw.get(index).copied().unwrap_or(0.0);
        let (bounce_state, bounce_progress) = self.get_bounce_status().unwrap_or((0, 0.0));
        let sandbox_failures = self
            .sandbox_statuses()
            .iter()
            .filter(|status| !status.alive || status.failure != 0)
            .count() as u32;
        RuntimeHealthSnapshot {
            dsp_load: value(0).clamp(0.0, 4.0),
            peak_left: value(1).max(0.0),
            peak_right: value(2).max(0.0),
            correlation: value(3).clamp(-1.0, 1.0),
            active_voices: value(4).max(0.0) as u32,
            telemetry_count: value(5).max(0.0) as u32,
            telemetry_version: value(6).max(0.0) as u32,
            audio_device_ready: value(7) > 0.5,
            audio_silent_fallback: self.is_silent_audio_fallback(),
            audio_driver_status: match self.audio_driver_status().as_str() {
                "initialized" => "initialized",
                "running" => "running",
                "start-failed" => "start-failed",
                "stopped" => "stopped",
                "silent-fallback" => "silent-fallback",
                "unavailable" => "unavailable",
                _ => "unavailable",
            },
            playing: value(8) > 0.5,
            playhead: value(9).max(0.0) as u64,
            bounce_state,
            bounce_progress: bounce_progress.clamp(0.0, 1.0),
            sandbox_failures,
            audio_range_overflow: self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.audio_range_overflowed()),
            non_finite_plugin_samples: self
                .engine
                .as_ref()
                .map_or(0, |engine| engine.non_finite_plugin_samples()),
        }
    }

    pub fn runtime_health_status_text(&self) -> &'static str {
        self.runtime_health_snapshot().status_text()
    }
}
