impl AuraCore {
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

    /// Queues browser decoding without moving the non-Send native engine to a
    /// worker. Call `poll_preview_audio_decode` from the main/control loop.
    pub fn preview_audio_file_async(&self, path: &str) -> u64 {
        crate::preview_audio_runtime::PreviewAudioRuntime::queue_decode(Path::new(path).to_path_buf())
    }

    pub fn poll_preview_audio_decode(&self) -> anyhow::Result<Option<u64>> {
        let Some((generation, result)) =
            crate::preview_audio_runtime::PreviewAudioRuntime::take_completed_decode()
        else {
            return Ok(None);
        };
        let (samples, source_rate) = result.map_err(anyhow::Error::msg)?;
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AudioEngine is unavailable"))?;
        engine.set_preview_sample(&samples, source_rate);
        engine.trigger_preview_sample();
        Ok(Some(generation))
    }

    // Transport
}
