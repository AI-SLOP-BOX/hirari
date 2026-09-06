//! Versioned, UI-independent entry points for external Aura clients.
//!
//! GUI, CLI, language bindings, and automation should depend on this module
//! instead of reaching into the native engine bridge directly.

use crate::production_events::{EventBatch, EventCursor, EventHub, ProductionEvent};
use crate::{bridge_error::BridgeError, AuraCore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub const CORE_API_VERSION: &str = "aura.core.v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaveContainer {
    Wav,
    Wave64,
}

impl WaveContainer {
    fn native_format(self) -> u32 {
        match self {
            Self::Wav => 0,
            Self::Wave64 => 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MixRenderRequest {
    pub project_path: PathBuf,
    pub output_path: PathBuf,
    pub container: WaveContainer,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MixRenderResult {
    pub api_version: &'static str,
    pub project_path: PathBuf,
    pub output_path: PathBuf,
    pub container: WaveContainer,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoadedMixRenderRequest {
    pub output_path: PathBuf,
    pub container: WaveContainer,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PluginParameterValue {
    pub parameter_id: u32,
    pub value: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AudioPluginSpec {
    /// Catalog alias such as `Aura Compressor`, `MyPlugin@clap`, or an exact
    /// catalog id. External formats use the same sandbox admission path as Aura.
    pub alias: String,
    #[serde(default)]
    pub parameters: Vec<PluginParameterValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AudioProcessRequest {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    #[serde(default)]
    pub plugins: Vec<AudioPluginSpec>,
    #[serde(default)]
    pub gain_db: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AudioProcessResult {
    pub api_version: &'static str,
    pub output_path: PathBuf,
    pub track_id: u32,
    pub plugin_count: usize,
    pub bytes_written: u64,
}

/// The stable, headless Core API surface for v1 clients.
pub struct CoreApiV1 {
    core: Rc<AuraCore>,
    events: Arc<Mutex<EventHub>>,
}

impl CoreApiV1 {
    /// Score a rendered UTAU/vocal take against its intended note plan.
    /// Returns per-note issues suitable for the editor's red/yellow/green UI.
    pub fn analyze_vocal_quality_json(
        targets_json: &str,
        frames_json: &str,
        sample_rate: f32,
    ) -> Result<String, BridgeError> {
        let targets: Vec<crate::vocal_quality::VocalNoteTarget> =
            serde_json::from_str(targets_json)
                .map_err(|error| BridgeError::new("invalid_vocal_targets", error.to_string()))?;
        let frames: Vec<crate::vocal_quality::VocalPitchFrame> = serde_json::from_str(frames_json)
            .map_err(|error| BridgeError::new("invalid_vocal_frames", error.to_string()))?;
        serde_json::to_string(&crate::vocal_quality::analyze_vocal_quality(
            &targets,
            &frames,
            sample_rate,
        ))
        .map_err(|error| BridgeError::new("vocal_quality_serialization_failed", error.to_string()))
    }

    pub fn waveform_integrated_lufs_json(
        left: &[f32],
        right: &[f32],
        _sample_rate: f32,
        _window: usize,
    ) -> String {
        let n = left.len().min(right.len()).max(1) as f32;
        let sum = left
            .iter()
            .zip(right)
            .map(|(l, r)| (f64::from(*l).powi(2) + f64::from(*r).powi(2)) * 0.5)
            .sum::<f64>();
        let lufs = (10.0 * (sum / f64::from(n)).max(1.0e-12).log10() - 0.691) as f32;
        serde_json::json!({"ok": true, "integrated_lufs_estimate": lufs}).to_string()
    }

    pub fn new_headless() -> Result<Self, BridgeError> {
        AuraCore::new_offline()
            .map(|core| {
                let core = Rc::new(core);
                let events = core.production_event_hub();
                Self { core, events }
            })
            .map_err(|error| BridgeError::new("core_initialization_failed", error.to_string()))
    }

    /// Adapt an existing application Core without creating a second engine.
    pub fn from_shared_core(core: Rc<AuraCore>) -> Self {
        let events = core.production_event_hub();
        Self { core, events }
    }

    /// Share one event stream between multiple adapters in the same process.
    pub fn with_event_hub(core: Rc<AuraCore>, events: Arc<Mutex<EventHub>>) -> Self {
        Self { core, events }
    }

    pub fn subscribe_events(
        &self,
        cursor: EventCursor,
        limit: usize,
    ) -> Result<EventBatch, BridgeError> {
        self.events
            .lock()
            .map(|hub| hub.subscribe_from(cursor, limit))
            .map_err(|_| BridgeError::new("event_hub_unavailable", "event stream lock is poisoned"))
    }

    /// Adapters call this only after a state mutation has been accepted.
    pub fn publish_event(
        &self,
        generation: u64,
        event: ProductionEvent,
    ) -> Result<u64, BridgeError> {
        self.events
            .lock()
            .map(|mut hub| hub.publish(generation, event).sequence)
            .map_err(|_| BridgeError::new("event_hub_unavailable", "event stream lock is poisoned"))
    }

    pub fn open_project(&self, path: impl AsRef<Path>) -> Result<(), BridgeError> {
        let path = path.as_ref();
        let path_text = valid_path(path, "invalid_project_path")?;
        self.core
            .load_project_v2(path_text)
            .map_err(|error| BridgeError::new("project_load_failed", error.to_string()))
    }

    pub fn render_mix(&self, request: MixRenderRequest) -> Result<MixRenderResult, BridgeError> {
        let project_text = valid_path(&request.project_path, "invalid_project_path")?;
        let output_text = valid_path(&request.output_path, "invalid_render_path")?;
        self.core
            .load_project_v2(project_text)
            .map_err(|error| BridgeError::new("project_load_failed", error.to_string()))?;

        let diagnostic = self
            .core
            .bounce_project_diagnostic_json(output_text, request.container.native_format());
        let value: serde_json::Value = serde_json::from_str(&diagnostic).map_err(|error| {
            BridgeError::new(
                "invalid_engine_response",
                format!("native render returned invalid JSON: {error}"),
            )
        })?;
        if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            let code = value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bounce_failed");
            return Err(BridgeError::new(code, "native mix render failed"));
        }
        let bytes_written = std::fs::metadata(&request.output_path)
            .map_err(|error| BridgeError::new("render_output_missing", error.to_string()))?
            .len();
        if bytes_written == 0 {
            return Err(BridgeError::new(
                "render_output_empty",
                "native mix render produced an empty file",
            ));
        }
        Ok(MixRenderResult {
            api_version: CORE_API_VERSION,
            project_path: request.project_path,
            output_path: request.output_path,
            container: request.container,
            bytes_written,
        })
    }

    /// Queue a render of the currently loaded project through the same native
    /// render graph used by synchronous CLI/API renders.
    pub fn queue_loaded_mix(&self, request: LoadedMixRenderRequest) -> Result<(), BridgeError> {
        let output_text = valid_path(&request.output_path, "invalid_render_path")?;
        if request.container != WaveContainer::Wav {
            return Err(BridgeError::new(
                "unsupported_async_render_format",
                "asynchronous loaded-project render currently supports WAV only",
            ));
        }
        if !output_text.ends_with(".wav") {
            return Err(BridgeError::new(
                "invalid_render_path",
                "WAV render output must end with .wav",
            ));
        }
        self.core
            .start_render_async_to(output_text)
            .then_some(())
            .ok_or_else(|| {
                BridgeError::new(
                    "render_queue_rejected",
                    "engine is busy or the render could not be queued",
                )
            })
    }

    /// Adds a validated batch-delivery job to the shared Core queue.
    pub fn enqueue_export_job_json(&self, job_json: &str) -> Result<usize, BridgeError> {
        let value = require_ok(
            self.core.enqueue_export_job_json(job_json),
            "export_job_rejected",
        )?;
        value
            .get("queued")
            .and_then(serde_json::Value::as_u64)
            .map(|count| count as usize)
            .ok_or_else(|| BridgeError::new("invalid_export_response", "queued count missing"))
    }

    /// Executes the shared queue through the native bounce graph and returns
    /// the published output paths.
    pub fn execute_export_queue(&self, output_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, BridgeError> {
        let output = valid_path(output_dir.as_ref(), "invalid_export_directory")?;
        let value = require_ok(
            self.core.execute_export_queue_json(output),
            "export_queue_failed",
        )?;
        let paths = value
            .get("completed")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| BridgeError::new("invalid_export_response", "completed outputs missing"))?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(PathBuf::from)
            .collect();
        Ok(paths)
    }

    /// Adds a stem job to the advanced queue that is backed by the native
    /// project renderer rather than an unconnected planning-only queue.
    pub fn enqueue_advanced_export_job(&self, job_json: &str) -> Result<usize, BridgeError> {
        let value = require_ok(
            self.core.enqueue_advanced_export_job_json(job_json),
            "advanced_export_job_rejected",
        )?;
        value
            .get("queued")
            .and_then(serde_json::Value::as_u64)
            .map(|count| count as usize)
            .ok_or_else(|| BridgeError::new("invalid_advanced_export_response", "queued count missing"))
    }

    /// Executes advanced queued stems through the same native bounce graph as
    /// the normal loaded-project render command.
    pub fn execute_advanced_export(&self, output_dir: impl AsRef<Path>) -> Result<usize, BridgeError> {
        let output = valid_path(output_dir.as_ref(), "invalid_export_directory")?;
        let value = require_ok(
            self.core.execute_advanced_export_json(output),
            "advanced_export_failed",
        )?;
        value
            .get("completed")
            .and_then(serde_json::Value::as_u64)
            .map(|count| count as usize)
            .ok_or_else(|| BridgeError::new("invalid_advanced_export_response", "completed count missing"))
    }

    /// Installs a measured HRTF pair supplied by the host's SOFA/database
    /// adapter. The core keeps only the bounded realtime kernel.
    pub fn set_hrtf_kernel(&self, track_id: u32, payload_json: &str) -> Result<usize, BridgeError> {
        let value = require_ok(
            self.core.set_hrtf_kernel_json(track_id, payload_json),
            "hrtf_kernel_rejected",
        )?;
        value
            .get("taps")
            .and_then(serde_json::Value::as_u64)
            .map(|taps| taps as usize)
            .ok_or_else(|| BridgeError::new("invalid_hrtf_response", "tap count missing"))
    }

    pub fn clear_hrtf_kernel(&self, track_id: u32) -> Result<(), BridgeError> {
        if self.core.clear_hrtf_kernel(track_id) {
            Ok(())
        } else {
            Err(BridgeError::new("hrtf_kernel_clear_rejected", "track or HRTF kernel not available"))
        }
    }

    /// Build a temporary graph from an audio file, process it, and render it.
    /// This API has no dependency on Aura's project document or GUI concepts.
    pub fn process_audio(
        &self,
        request: AudioProcessRequest,
    ) -> Result<AudioProcessResult, BridgeError> {
        let input_text = valid_path(&request.input_path, "invalid_audio_input_path")?;
        let output_text = valid_path(&request.output_path, "invalid_render_path")?;
        if !request.input_path.is_file() {
            return Err(BridgeError::new(
                "audio_input_not_found",
                "input must be a readable audio file",
            ));
        }
        if !output_text.ends_with(".wav") {
            return Err(BridgeError::new(
                "invalid_render_path",
                "processed output must end with .wav",
            ));
        }
        if !request.gain_db.is_finite() || !(-96.0..=24.0).contains(&request.gain_db) {
            return Err(BridgeError::new(
                "invalid_gain",
                "gain_db must be finite and within -96..=24 dB",
            ));
        }
        if request.plugins.len() > 64 {
            return Err(BridgeError::new(
                "plugin_chain_too_large",
                "a processing chain may contain at most 64 plugins",
            ));
        }

        self.core.new_project();
        let track_id = self.core.add_track(0);
        if track_id == 0 {
            return Err(BridgeError::new(
                "track_create_failed",
                "native graph rejected the audio node",
            ));
        }
        require_ok(
            self.core
                .add_region_diagnostic_json(track_id, input_text, 0.0),
            "audio_input_rejected",
        )?;

        let linear_gain = 10.0_f32.powf(request.gain_db / 20.0).clamp(0.0, 2.0);
        require_ok(
            self.core.set_volume_diagnostic_json(track_id, linear_gain),
            "gain_rejected",
        )?;

        for (plugin_index, plugin) in request.plugins.iter().enumerate() {
            if plugin.alias.trim().is_empty() || plugin.alias.len() > 512 {
                return Err(BridgeError::new(
                    "invalid_plugin_alias",
                    "plugin alias must be 1..=512 characters",
                ));
            }
            require_nested_ok(
                self.core
                    .add_named_sandboxed_plugin_diagnostic_json(track_id, &plugin.alias),
                "plugin_insert_rejected",
            )?;
            for parameter in &plugin.parameters {
                if !parameter.value.is_finite() || !(0.0..=1.0).contains(&parameter.value) {
                    return Err(BridgeError::new(
                        "invalid_plugin_parameter",
                        "plugin parameter values must be normalized to 0..=1",
                    ));
                }
                require_ok(
                    self.core.set_plugin_parameter_diagnostic_json(
                        track_id,
                        plugin_index as u32,
                        parameter.parameter_id,
                        parameter.value,
                    ),
                    "plugin_parameter_rejected",
                )?;
            }
        }

        require_ok(
            self.core.bounce_project_diagnostic_json(output_text, 0),
            "audio_process_failed",
        )?;
        let bytes_written = std::fs::metadata(&request.output_path)
            .map_err(|error| BridgeError::new("render_output_missing", error.to_string()))?
            .len();
        Ok(AudioProcessResult {
            api_version: CORE_API_VERSION,
            output_path: request.output_path,
            track_id,
            plugin_count: request.plugins.len(),
            bytes_written,
        })
    }

    pub fn core(&self) -> &AuraCore {
        &self.core
    }

    /// Returns the versioned capability contract for alternate UIs and AI
    /// clients. Consumers can gate controls without probing private structs.
    pub fn engine_capabilities_json(&self) -> String {
        self.core.engine_capabilities_json()
    }
}

fn valid_path<'a>(path: &'a Path, code: &str) -> Result<&'a str, BridgeError> {
    let text = path
        .to_str()
        .ok_or_else(|| BridgeError::new(code, "path must be valid UTF-8"))?;
    if text.trim().is_empty() || text.len() > 4096 {
        return Err(BridgeError::new(code, "path must be 1..=4096 characters"));
    }
    Ok(text)
}

fn require_ok(raw: String, fallback_code: &str) -> Result<serde_json::Value, BridgeError> {
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        BridgeError::new(
            "invalid_engine_response",
            format!("native engine returned invalid JSON: {error}"),
        )
    })?;
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(value);
    }
    Err(diagnostic_error(&value, fallback_code))
}

fn require_nested_ok(raw: String, fallback_code: &str) -> Result<serde_json::Value, BridgeError> {
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        BridgeError::new(
            "invalid_engine_response",
            format!("native engine returned invalid JSON: {error}"),
        )
    })?;
    let result = value.get("result").unwrap_or(&value);
    if result.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(value);
    }
    Err(diagnostic_error(result, fallback_code))
}

fn diagnostic_error(value: &serde_json::Value, fallback_code: &str) -> BridgeError {
    BridgeError::new(
        value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback_code),
        value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("native engine rejected the operation"),
    )
    .retryable(
        value
            .get("retryable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_container_encoding_are_stable() {
        assert_eq!(CORE_API_VERSION, "aura.core.v1");
        assert_eq!(WaveContainer::Wav.native_format(), 0);
        assert_eq!(WaveContainer::Wave64.native_format(), 1);
        assert_eq!(
            serde_json::to_string(&WaveContainer::Wave64).unwrap(),
            "\"wave64\""
        );
    }

    #[test]
    fn headless_api_rejects_empty_paths() {
        let api = CoreApiV1::new_headless().expect("offline core should initialize");
        let error = api.open_project("").expect_err("empty path must fail");
        assert_eq!(error.code, "invalid_project_path");
    }

    #[test]
    fn capability_contract_is_versioned_and_actionable() {
        let api = CoreApiV1::new_headless().expect("offline core should initialize");
        let value: serde_json::Value = serde_json::from_str(&api.engine_capabilities_json())
            .expect("capability contract must be valid JSON");
        assert_eq!(value["schema"], "aura.engine-capabilities.v1");
        let capabilities = value["capabilities"]
            .as_array()
            .expect("capabilities array");
        assert!(capabilities
            .iter()
            .any(|entry| entry["id"] == "direct_routing"));
        assert!(capabilities
            .iter()
            .any(|entry| entry["status"] == "integration_required"));
    }

    #[test]
    fn shared_core_publishes_state_changes_to_api_subscribers() {
        let api = CoreApiV1::new_headless().expect("offline core should initialize");
        let track_id = api.core().add_track(0);
        assert_ne!(track_id, 0);
        let batch = api
            .subscribe_events(EventCursor { after: 0 }, 16)
            .expect("event subscription should work");
        assert!(batch.events.iter().any(|event| matches!(
            event.event,
            ProductionEvent::TrackAdded { track_id: id, .. } if id == track_id
        )));
    }
}
