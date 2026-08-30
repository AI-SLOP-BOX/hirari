use crate::project_contracts::{
    MacroMappingContract, MarkerContract, MidiLearnMappingContract, MidiNoteContract,
    OpenUtauTuningContract, OpenUtauVocalContract, TrackStackContract,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
#[derive(Debug, Clone, Default)]
pub struct LoudnessData {
    pub integrated: f32,
    pub short_term: f32,
    pub true_peak_l: f32,
    pub true_peak_r: f32,
    pub correlation: f32,
}

#[derive(Debug, Clone, Default)]
pub struct ClashData {
    pub frequency: f32,
    pub severity: f32,
}

#[derive(Debug, Deserialize)]
struct NativeLayoutTrack {
    id: u32,
    #[serde(default)]
    regions: Vec<NativeLayoutRegion>,
}

#[derive(Debug, Deserialize)]
struct NativeLayoutRegion {
    id: u32,
    path: String,
    start: u64,
    len: u64,
}

fn valid_pcm_or_float_wav(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let read_u16 = |v: &[u8]| u16::from_le_bytes([v[0], v[1]]);
    let read_u32 = |v: &[u8]| u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    let read_u64 = |v: &[u8]| v.try_into().ok().map(u64::from_le_bytes);
    let mut cursor = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut rf64_data_size: Option<u64> = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[cursor..cursor + 4];
        let declared_size = read_u32(&bytes[cursor + 4..cursor + 8]);
        let size = if kind == b"data" && declared_size == u32::MAX {
            rf64_data_size.and_then(|value| usize::try_from(value).ok())
        } else {
            usize::try_from(declared_size).ok()
        };
        let Some(size) = size else { return false };
        let start = cursor + 8;
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"ds64" && size >= 16 {
            let Some(value) = read_u64(&bytes[start + 8..start + 16]) else {
                return false;
            };
            rf64_data_size = Some(value);
        } else if kind == b"fmt " && size >= 16 {
            let chunk = &bytes[start..end];
            fmt = Some((
                read_u16(&chunk[0..2]),
                read_u16(&chunk[2..4]),
                read_u32(&chunk[4..8]),
                read_u16(&chunk[12..14]),
                read_u16(&chunk[14..16]),
            ));
        } else if kind == b"data" {
            data = Some(&bytes[start..end]);
        }
        let padded_end = end
            .checked_add(size & 1)
            .filter(|value| *value <= bytes.len());
        let Some(padded_end) = padded_end else {
            return false;
        };
        cursor = padded_end;
    }
    let Some((audio_format, channels, sample_rate, block_align, bits_per_sample)) = fmt else {
        return false;
    };
    let Some(payload) = data else { return false };
    let valid_format = audio_format == 1 || audio_format == 3;
    let valid_depth = matches!(bits_per_sample, 16 | 24 | 32);
    let valid_channels = channels > 0 && channels <= 2;
    valid_format
        && valid_depth
        && valid_channels
        && sample_rate > 0
        && block_align > 0
        && payload.len() % usize::from(block_align) == 0
}

fn float_wav_samples_are_finite(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let read_u16 = |v: &[u8]| u16::from_le_bytes([v[0], v[1]]);
    let read_u32 = |v: &[u8]| u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    let read_u64 = |v: &[u8]| v.try_into().ok().map(u64::from_le_bytes);
    let mut cursor = 12usize;
    let mut is_float = false;
    let mut payload = None;
    let mut rf64_data_size: Option<u64> = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[cursor..cursor + 4];
        let declared_size = read_u32(&bytes[cursor + 4..cursor + 8]);
        let size = if kind == b"data" && declared_size == u32::MAX {
            rf64_data_size.and_then(|value| usize::try_from(value).ok())
        } else {
            usize::try_from(declared_size).ok()
        };
        let Some(size) = size else {
            return false;
        };
        let start = cursor + 8;
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"ds64" && size >= 16 {
            let Some(value) = read_u64(&bytes[start + 8..start + 16]) else {
                return false;
            };
            rf64_data_size = Some(value);
        } else if kind == b"fmt " && size >= 16 {
            is_float = read_u16(&bytes[start..start + 2]) == 3
                && read_u16(&bytes[start + 14..start + 16]) == 32;
        } else if kind == b"data" {
            payload = Some(&bytes[start..end]);
        }
        let padded_end = end
            .checked_add(size & 1)
            .filter(|value| *value <= bytes.len());
        let Some(padded_end) = padded_end else {
            return false;
        };
        cursor = padded_end;
    }
    let Some(payload) = payload else { return false };
    !is_float
        || payload.chunks_exact(4).all(|sample| {
            f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]).is_finite()
        })
}

fn is_wav_output_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}

pub fn report_aura_log(level: u32, msg: &str) {
    match level {
        0 => log::error!("[AURA] {}", msg),
        1 => log::warn!("[AURA] {}", msg),
        2 => log::info!("[AURA] {}", msg),
        _ => log::debug!("[AURA] {}", msg),
    }
}

pub struct AuraCore {
    // --- INDUSTRIAL: Explicit Lifetime Binding ---
    // engine is the root of sovereignty. analysis is a dependent handle.
    engine: cxx::UniquePtr<ffi::AudioEngine>,
    analysis: cxx::UniquePtr<ffi::AnalysisHub>,
    gpu_initialized: bool,
    preview_audio: std::sync::Mutex<preview_audio_runtime::PreviewAudioRuntime>,
    recording_session: std::sync::Mutex<Option<recording_session::RecordingSession>>,
    comping: std::sync::Mutex<comping::CompingOrchestrator>,
    aux_track_ids: std::sync::Mutex<HashSet<u32>>,
    midi_events: std::sync::Mutex<Vec<midi::MIDIEvent>>,
    /// Canonical scheduled-note state mirrored into the native snapshot.
    /// This is deliberately separate from transient MIDI input events.
    scheduled_midi_notes: std::sync::Mutex<Vec<MidiNoteContract>>,
    pub(crate) midi_monitor: std::sync::Mutex<crate::midi_monitor::MidiMonitor>,
    pub(crate) review_notes: std::sync::Mutex<crate::review_notes::ReviewNoteStore>,
    pub(crate) control_room: std::sync::Mutex<crate::control_room::ControlRoomState>,
    /// Canonical chord-track events shared by composition UI, generators, and API clients.
    chord_track: std::sync::Mutex<Vec<crate::harmonic::ChordEvent>>,
    /// Metadata snapshots keyed by native undo depth for lyric-only changes.
    midi_lyric_history: std::sync::Mutex<Vec<MidiLyricHistoryEntry>>,
    chord_history: std::sync::Mutex<Vec<ChordHistoryEntry>>,
    chord_redo_history: std::sync::Mutex<Vec<ChordHistoryEntry>>,
    comping_history: std::sync::Mutex<Vec<CompingHistoryEntry>>,
    comping_redo_history: std::sync::Mutex<Vec<CompingHistoryEntry>>,
    plugin_parameter_events: std::sync::Mutex<Vec<PluginParameterEvent>>,
    /// Serializes project save/load and plugin-state hydration on the
    /// control plane. Audio processing remains native and lock-free.
    project_transaction: std::sync::Mutex<()>,
    /// OpenUtau source/render pairs belong to the canonical project document,
    /// not to the native layout JSON. Keep them on the same control-plane
    /// transaction so importing a vocal cannot disappear on the next save.
    openutau_vocals: std::sync::Mutex<Vec<OpenUtauVocalContract>>,
    /// Canonical stack topology shared by arrange, mixer, CLI, and project
    /// persistence. Native routing is rebuilt only after this state validates.
    track_stacks: std::sync::Mutex<Vec<TrackStackContract>>,
    /// Per-member fader baselines used to apply stack gain without repeatedly
    /// multiplying an already-scaled native fader.
    track_stack_base_volumes: std::sync::Mutex<HashMap<u32, f32>>,
    markers: std::sync::Mutex<Vec<MarkerContract>>,
    /// Persisted macro fan-out edges.  The native engine owns DSP execution;
    /// this control-plane copy makes mappings survive save/load and history.
    macro_mappings: std::sync::Mutex<Vec<MacroMappingContract>>,
    midi_learn_mappings: std::sync::Mutex<Vec<MidiLearnMappingContract>>,
    /// Named MixConsole scenes. Values are captured on the control plane and
    /// recalled atomically by the mixer adapter; audio callbacks never lock it.
    mix_snapshots: std::sync::Mutex<crate::snapshots::SnapshotOrchestrator>,
    /// Runtime pickup latches are separate from the persisted mapping. They
    /// reset when a mapping changes or a project is reloaded.
    midi_pickup_acquired: std::sync::Mutex<HashSet<String>>,
    pub(crate) production_events:
        std::sync::Arc<std::sync::Mutex<crate::production_events::EventHub>>,
}

#[derive(Clone)]
pub(crate) struct MidiLyricHistoryEntry {
    pub depth_after: u32,
    pub before: Vec<MidiNoteContract>,
    pub after: Vec<MidiNoteContract>,
}

#[derive(Clone)]
pub(crate) struct ChordHistoryEntry {
    pub depth_after: u32,
    pub before: Vec<crate::harmonic::ChordEvent>,
    pub after: Vec<crate::harmonic::ChordEvent>,
}

#[derive(Clone)]
pub(crate) struct CompingHistoryEntry {
    pub before: crate::comping::CompingOrchestrator,
    pub after: crate::comping::CompingOrchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginParameterEvent {
    pub track_id: u32,
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SandboxStatus {
    pub track_id: u32,
    pub plugin_index: u32,
    pub alive: bool,
    pub can_retry: bool,
    pub failure: u8,
    pub dropped_output_midi: u32,
    pub mailbox_overruns: u32,
    pub input_midi_truncations: u32,
    pub recovery_mode: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxStateDiagnostic {
    pub ok: bool,
    pub code: u8,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginStateDiagnostic {
    pub ok: bool,
    pub code: &'static str,
    pub message: String,
}

impl SandboxStatus {
    pub const QUARANTINED: u8 = 1;

    pub fn is_quarantined(&self) -> bool {
        self.recovery_mode == Self::QUARANTINED
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFailureKind {
    None,
    MissingHelper,
    InvalidPluginPath,
    SpawnFailed,
    PluginLoadFailed,
    PluginInstanceFailed,
    ProcessHung,
    ProcessFailed,
    Quarantined,
    Unsupported,
    Unknown(u8),
}

impl SandboxFailureKind {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::None,
            1 => Self::MissingHelper,
            2 => Self::InvalidPluginPath,
            3 => Self::SpawnFailed,
            5 => Self::PluginLoadFailed,
            7 => Self::PluginInstanceFailed,
            12 => Self::ProcessHung,
            13 => Self::ProcessFailed,
            10 => Self::Quarantined,
            14 => Self::Unsupported,
            value => Self::Unknown(value),
        }
    }
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::SpawnFailed | Self::ProcessHung | Self::ProcessFailed
        )
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::MissingHelper => "missing_helper",
            Self::InvalidPluginPath => "invalid_plugin_path",
            Self::SpawnFailed => "spawn_failed",
            Self::PluginLoadFailed => "plugin_load_failed",
            Self::PluginInstanceFailed => "plugin_instance_failed",
            Self::ProcessHung => "process_hung",
            Self::ProcessFailed => "process_failed",
            Self::Quarantined => "quarantined",
            Self::Unsupported => "unsupported",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSnapshot {
    pub track_id: u32,
    pub plugin_index: u32,
    pub display_name: String,
    pub alive: bool,
    pub can_retry: bool,
    pub failure: u8,
    pub dropped_output_midi: u32,
    pub mailbox_overruns: u32,
    pub recovery_mode: u8,
}

impl SandboxSnapshot {
    pub const CLEAR_BLOCK: u8 = 0;
    pub const QUARANTINED: u8 = 1;

    pub fn is_quarantined(&self) -> bool {
        self.recovery_mode == Self::QUARANTINED
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BounceSnapshot {
    pub state: u32,
    pub progress: f32,
    pub progress_available: bool,
}

fn normalize_bounce_progress(progress: f32) -> (f32, bool) {
    if progress.is_finite() {
        (progress.clamp(0.0, 1.0), true)
    } else {
        (0.0, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RuntimeHealthSnapshot {
    pub dsp_load: f32,
    pub peak_left: f32,
    pub peak_right: f32,
    pub correlation: f32,
    pub active_voices: u32,
    pub telemetry_count: u32,
    pub telemetry_version: u32,
    pub audio_device_ready: bool,
    pub audio_silent_fallback: bool,
    pub audio_driver_status: &'static str,
    pub playing: bool,
    pub playhead: u64,
    pub bounce_state: u32,
    pub bounce_progress: f32,
    pub sandbox_failures: u32,
    pub audio_range_overflow: bool,
    pub non_finite_plugin_samples: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHealthState {
    Offline,
    Idle,
    Healthy,
    Degraded,
    Fault,
}

impl RuntimeHealthSnapshot {
    pub fn state(self) -> RuntimeHealthState {
        if !self.audio_device_ready {
            return RuntimeHealthState::Offline;
        }
        if self.sandbox_failures > 0 || self.non_finite_plugin_samples > 0 || self.dsp_load > 1.0 {
            return RuntimeHealthState::Fault;
        }
        if self.dsp_load > 0.8 || self.peak_left > 1.0 || self.peak_right > 1.0 {
            return RuntimeHealthState::Degraded;
        }
        if !self.playing {
            RuntimeHealthState::Idle
        } else {
            RuntimeHealthState::Healthy
        }
    }

    pub fn status_text(self) -> &'static str {
        if self.audio_silent_fallback {
            return "AUDIO SILENT FALLBACK";
        }
        match self.audio_driver_status {
            "start-failed" => return "AUDIO START FAILED",
            "unavailable" => return "AUDIO UNAVAILABLE",
            "stopped" => return "AUDIO STOPPED",
            _ => {}
        }
        match self.state() {
            RuntimeHealthState::Offline => "AUDIO OFFLINE",
            RuntimeHealthState::Idle => "ENGINE IDLE",
            RuntimeHealthState::Healthy => "ENGINE HEALTHY",
            RuntimeHealthState::Degraded => "ENGINE DEGRADED",
            RuntimeHealthState::Fault => "ENGINE FAULT",
        }
    }
}

#[cfg(test)]
mod runtime_health_tests {
    use super::{
        float_wav_samples_are_finite, valid_pcm_or_float_wav, RuntimeHealthSnapshot,
        RuntimeHealthState, SandboxFailureKind,
    };

    #[test]
    fn malformed_wav_headers_are_rejected_without_panicking() {
        for length in 0..96usize {
            let mut bytes = vec![0u8; length];
            if length >= 12 {
                bytes[0..4].copy_from_slice(b"RF64");
                bytes[8..12].copy_from_slice(b"WAVE");
            }
            assert!(!valid_pcm_or_float_wav(&bytes));
            assert!(!float_wav_samples_are_finite(&bytes));
        }
    }

    #[test]
    fn sandbox_failure_codes_are_stable_for_external_diagnostics() {
        assert_eq!(SandboxFailureKind::None.code(), "none");
        assert_eq!(SandboxFailureKind::MissingHelper.code(), "missing_helper");
        assert_eq!(
            SandboxFailureKind::InvalidPluginPath.code(),
            "invalid_plugin_path"
        );
        assert_eq!(SandboxFailureKind::SpawnFailed.code(), "spawn_failed");
        assert_eq!(
            SandboxFailureKind::PluginLoadFailed.code(),
            "plugin_load_failed"
        );
        assert_eq!(
            SandboxFailureKind::PluginInstanceFailed.code(),
            "plugin_instance_failed"
        );
        assert_eq!(SandboxFailureKind::ProcessHung.code(), "process_hung");
        assert_eq!(SandboxFailureKind::ProcessFailed.code(), "process_failed");
        assert_eq!(SandboxFailureKind::Quarantined.code(), "quarantined");
        assert_eq!(SandboxFailureKind::Unsupported.code(), "unsupported");
        assert_eq!(SandboxFailureKind::Unknown(255).code(), "unknown");
    }

    fn snapshot(status: &'static str, ready: bool, silent: bool) -> RuntimeHealthSnapshot {
        RuntimeHealthSnapshot {
            dsp_load: 0.0,
            peak_left: 0.0,
            peak_right: 0.0,
            correlation: 0.0,
            active_voices: 0,
            telemetry_count: 0,
            telemetry_version: 0,
            audio_device_ready: ready,
            audio_silent_fallback: silent,
            audio_driver_status: status,
            playing: false,
            playhead: 0,
            bounce_state: 0,
            bounce_progress: 0.0,
            sandbox_failures: 0,
            audio_range_overflow: false,
            non_finite_plugin_samples: 0,
        }
    }

    #[test]
    fn driver_states_never_report_healthy_audio_when_start_failed() {
        let failed = snapshot("start-failed", false, false);
        assert_eq!(failed.state(), RuntimeHealthState::Offline);
        assert_eq!(failed.status_text(), "AUDIO START FAILED");

        let fallback = snapshot("silent-fallback", false, true);
        assert_eq!(fallback.status_text(), "AUDIO SILENT FALLBACK");
    }

    #[test]
    fn stopped_and_unavailable_states_have_distinct_diagnostics() {
        assert_eq!(
            snapshot("stopped", false, false).status_text(),
            "AUDIO STOPPED"
        );
        assert_eq!(
            snapshot("unavailable", false, false).status_text(),
            "AUDIO UNAVAILABLE"
        );
    }
}

fn decode_sandbox_statuses(raw: &[u32]) -> Vec<SandboxStatus> {
    const STATUS_HEADER_V9: u32 = 0x4155_5209;
    let (raw, width) = if raw.first().copied() == Some(STATUS_HEADER_V9) {
        (&raw[1..], 9)
    } else if raw.len() >= 9 && raw.len().is_multiple_of(9) {
        (raw, 9)
    } else if raw.len() >= 8 && raw.len().is_multiple_of(8) {
        (raw, 8)
    } else if raw.len() >= 7 && raw.len().is_multiple_of(7) {
        (raw, 7)
    } else {
        (raw, 6)
    };
    raw.chunks_exact(width)
        .map(|chunk| SandboxStatus {
            track_id: chunk[0],
            plugin_index: chunk[1],
            alive: chunk[2] != 0,
            can_retry: chunk[3] != 0,
            failure: chunk[4].min(u8::MAX as u32) as u8,
            dropped_output_midi: chunk[5],
            mailbox_overruns: if width >= 7 { chunk[6] } else { 0 },
            input_midi_truncations: if width >= 8 { chunk[7] } else { 0 },
            recovery_mode: if width == 9 {
                chunk[8].min(u8::MAX as u32) as u8
            } else {
                0
            },
        })
        .collect()
}

fn safe_plugin_display_name(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let name = name.strip_suffix(".component").unwrap_or(name);
    let name = name.strip_suffix(".vst3").unwrap_or(name);
    let name = name.strip_suffix(".clap").unwrap_or(name);
    if name.is_empty() {
        "Plugin".to_string()
    } else {
        name.chars().take(128).collect()
    }
}

impl AuraCore {
    /// Control-plane catalog used by the plugin browser and CLI. It exposes
    /// installed Vital/Surge XT CLAP entries when present, plus AU/VST3
    /// capability entries without touching the audio callback.
    pub fn installed_plugin_catalog_json(&self) -> String {
        crate::plugin_catalog::json()
    }

    pub fn set_plugin_favorite_diagnostic_json(&self, id: &str, favorite: bool) -> String {
        match crate::plugin_catalog::set_favorite(id, favorite) {
            Ok(changed) => serde_json::json!({
                "ok": true, "operation": "set_plugin_favorite", "id": id,
                "favorite": favorite, "changed": changed,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false, "code": "plugin_favorite_failed", "message": error.to_string(),
            })
            .to_string(),
        }
    }

    pub fn quarantine_plugin_diagnostic_json(&self, path: &str) -> String {
        match crate::plugin_catalog::quarantine_path(path) {
            Ok(plugin) => serde_json::json!({
                "ok": true,
                "operation": "quarantine_plugin",
                "plugin": plugin,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "plugin_quarantine_failed",
                "message": error,
            })
            .to_string(),
        }
    }

    pub fn clear_plugin_quarantine_diagnostic_json(&self, path: &str) -> String {
        match crate::plugin_catalog::clear_quarantine_path(path) {
            Ok(plugin) => serde_json::json!({
                "ok": true,
                "operation": "clear_plugin_quarantine",
                "plugin": plugin,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "plugin_quarantine_clear_failed",
                "message": error,
            })
            .to_string(),
        }
    }

    /// Insert a named installed plugin through the same sandbox admission path
    /// used by project hydration. The catalog's worker capability probe is
    /// authoritative: CLAP, AU, and VST3 are only offered when the packaged
    /// worker reports that exact backend as available for this build.
    pub fn add_named_sandboxed_plugin_diagnostic_json(&self, track_id: u32, alias: &str) -> String {
        let Some(plugin) = crate::plugin_catalog::resolve(alias) else {
            return serde_json::json!({"ok":false,"code":"plugin_not_found","alias":alias})
                .to_string();
        };
        if plugin.quarantined {
            return serde_json::json!({
                "ok": false,
                "code": "plugin_quarantined",
                "alias": alias,
                "binary_hash": plugin.binary_hash,
                "path": plugin.path,
            })
            .to_string();
        }
        if plugin.format == "internal" {
            let plugin_type = match plugin.path.as_str() {
                "Aura/Limiter" => 0,
                "Aura/Compressor" => 1,
                "Aura/Gate" => 2,
                "Aura/Saturation" => 3,
                "Aura/Transient" => 4,
                "Aura/DeEsser" => 5,
                "Aura/Delay" => 6,
                "Aura/Reverb" => 7,
                "Aura/DynamicEQ" => 8,
                "Aura/MidSide" => 9,
                "Aura/Width" => 10,
                _ => {
                    return serde_json::json!({
                        "ok": false,
                        "code": "unknown_internal_plugin",
                        "path": plugin.path,
                    })
                    .to_string()
                }
            };
            let result = self.add_plugin_diagnostic_json(track_id, plugin_type);
            return serde_json::json!({
                "plugin": plugin,
                "result": serde_json::from_str::<serde_json::Value>(&result)
                    .unwrap_or_else(|_| serde_json::json!({"ok":false,"code":"invalid_native_result"})),
            }).to_string();
        }
        if !plugin.sandboxed {
            return serde_json::json!({"ok":false,"code":"plugin_format_not_ready","format":plugin.format,"path":plugin.path}).to_string();
        }
        let result = self.add_sandboxed_plugin_diagnostic_json(track_id, &plugin.path);
        serde_json::json!({"plugin":plugin,"result":serde_json::from_str::<serde_json::Value>(&result).unwrap_or_else(|_| serde_json::json!({"ok":false,"code":"invalid_native_result"}))}).to_string()
    }

    pub fn openutau_status_json(&self) -> String {
        serde_json::to_string(&crate::openutau::status())
            .unwrap_or_else(|_| "{\"installed\":false}".into())
    }

    /// Return the bounded structured note model used by the in-DAW vocal
    /// editor. Unlike note_preview(), this keeps timing, pitch, and lyric
    /// fields machine-readable for CLI/LLM tooling and piano-roll editing.
    pub fn openutau_notes_json(&self, source_path: &str) -> String {
        if let Err(error) = crate::openutau::validate_source_file(source_path) {
            return serde_json::json!({"ok": false, "code": "openutau_source_rejected", "error": error}).to_string();
        }
        let notes = crate::openutau::parse_notes(source_path);
        serde_json::json!({
            "ok": true,
            "source_path": source_path,
            "note_count": notes.len(),
            "notes": notes,
        })
        .to_string()
    }

    pub fn openutau_midi_notes_json(
        &self,
        source_path: &str,
        track_id: u32,
        sample_rate: u32,
        ticks_per_beat: u32,
    ) -> String {
        match crate::openutau::notes_as_midi(source_path, track_id, sample_rate, ticks_per_beat) {
            Ok(notes) => serde_json::json!({
                "ok": true,
                "operation": "openutau_midi_notes",
                "source_path": source_path,
                "track_id": track_id,
                "sample_rate": sample_rate,
                "ticks_per_beat": ticks_per_beat,
                "notes": notes,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "openutau_midi_conversion_failed",
                "error": error,
            })
            .to_string(),
        }
    }

    /// Validate an OpenUtau source/render pair before it becomes a project
    /// vocal region. The actual render is intentionally external to the
    /// realtime engine; once rendered, the pair is fully reproducible from
    /// project metadata and the referenced audio asset.
    pub fn openutau_import_diagnostic_json(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> String {
        let import = crate::openutau::audit_import_files(source_path, rendered_audio_path);
        if let Ok(audit) = import {
            return serde_json::json!({
                "ok": true,
                "source_path": source_path,
                "rendered_audio_path": rendered_audio_path,
                "source_hash": audit.source_hash,
                "rendered_audio_hash": audit.rendered_audio_hash,
                "rendered_audio_bytes": audit.rendered_audio_bytes,
                "rendered_sample_rate": audit.rendered_sample_rate,
                "rendered_channels": audit.rendered_channels,
                "rendered_frames": audit.rendered_frames,
                "source_note_count": audit.source_note_count,
                "source_singers": audit.source_singers,
                "openutau": crate::openutau::status(),
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "openutau_import_rejected",
            "error": import.err(),
        })
        .to_string()
    }

    pub fn register_openutau_vocal(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> anyhow::Result<()> {
        let audit = crate::openutau::audit_import_files(source_path, rendered_audio_path)
            .map_err(anyhow::Error::msg)?;
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        if !vocals.iter().any(|entry| {
            entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path
        }) {
            vocals.push(OpenUtauVocalContract {
                source_path: source_path.to_owned(),
                rendered_audio_path: rendered_audio_path.to_owned(),
                singer: String::new(),
                source_generation: self.project_generation(),
                source_hash: audit.source_hash,
                rendered_audio_hash: audit.rendered_audio_hash,
                rendered_audio_bytes: audit.rendered_audio_bytes,
                rendered_sample_rate: audit.rendered_sample_rate,
                rendered_channels: audit.rendered_channels,
                rendered_frames: audit.rendered_frames,
                source_note_count: audit.source_note_count,
                source_singers: audit.source_singers,
                tuning: OpenUtauTuningContract {
                    scoop: 0.35,
                    vibrato: 0.45,
                    dynamics: 0.60,
                    consonants: 0.50,
                },
            });
        }
        Ok(())
    }

    pub fn set_openutau_tuning(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
        scoop: f32,
        vibrato: f32,
        dynamics: f32,
        consonants: f32,
    ) -> anyhow::Result<()> {
        let values = [scoop, vibrato, dynamics, consonants];
        if values
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(anyhow::anyhow!(
                "OpenUtau tuning values must be normalized 0..=1"
            ));
        }
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        let Some(vocal) = vocals.iter_mut().find(|entry| {
            entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path
        }) else {
            return Err(anyhow::anyhow!("OpenUtau vocal source is not registered"));
        };
        vocal.tuning = OpenUtauTuningContract {
            scoop,
            vibrato,
            dynamics,
            consonants,
        };
        Ok(())
    }

    pub fn unregister_openutau_vocal(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> anyhow::Result<()> {
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        vocals.retain(|entry| {
            !(entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path)
        });
        Ok(())
    }

    pub fn track_stacks_json(&self) -> String {
        self.track_stacks
            .lock()
            .ok()
            .and_then(|stacks| serde_json::to_string(&*stacks).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn markers_json(&self) -> String {
        self.markers
            .lock()
            .ok()
            .and_then(|value| serde_json::to_string(&*value).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn restore_markers_json(&self, snapshot: &str) -> bool {
        let Ok(candidate) = serde_json::from_str::<Vec<MarkerContract>>(snapshot) else {
            return false;
        };
        let mut ids = HashSet::with_capacity(candidate.len());
        if candidate.len() > 65_536
            || candidate
                .iter()
                .any(|marker| marker.validate().is_err() || !ids.insert(marker.id))
        {
            return false;
        }
        let Ok(mut current) = self.markers.lock() else {
            return false;
        };
        *current = candidate;
        true
    }

    pub fn upsert_marker(&self, id: u32, label: &str, beat: f64, color: &str) -> bool {
        let marker = MarkerContract {
            id,
            label: label.to_owned(),
            beat,
            color: color.to_owned(),
        };
        if marker.validate().is_err() {
            return false;
        }
        let Ok(mut markers) = self.markers.lock() else {
            return false;
        };
        if let Some(existing) = markers.iter_mut().find(|item| item.id == id) {
            *existing = marker;
        } else if markers.len() >= 65_536 {
            return false;
        } else {
            markers.push(marker);
        }
        markers.sort_by(|a, b| a.beat.total_cmp(&b.beat).then_with(|| a.id.cmp(&b.id)));
        drop(markers);
        self.publish_production_event(crate::production_events::ProductionEvent::MarkerChanged {
            marker_id: id,
        });
        true
    }

    pub fn delete_marker(&self, id: u32) -> bool {
        let Ok(mut markers) = self.markers.lock() else {
            return false;
        };
        let before = markers.len();
        markers.retain(|marker| marker.id != id);
        let changed = markers.len() != before;
        drop(markers);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::MarkerChanged { marker_id: id },
            );
        }
        changed
    }

    pub fn reset_markers(&self) {
        if let Ok(mut markers) = self.markers.lock() {
            markers.clear();
            markers.push(MarkerContract {
                id: 1,
                label: "START".to_owned(),
                beat: 0.0,
                color: "#646496".to_owned(),
            });
        }
    }

    pub fn restore_track_stacks_json(&self, snapshot: &str) -> bool {
        let Ok(candidate) = serde_json::from_str::<Vec<TrackStackContract>>(snapshot) else {
            return false;
        };
        let mut ids = HashSet::new();
        if candidate.iter().any(|stack| {
            stack.id == 0
                || !ids.insert(stack.id)
                || stack.name.trim().is_empty()
                || stack.name.len() > 256
                || !stack.master_gain.is_finite()
                || !(0.0..=2.0).contains(&stack.master_gain)
                || {
                    let mut members = HashSet::new();
                    stack
                        .member_track_ids
                        .iter()
                        .any(|id| *id == 0 || !members.insert(*id))
                }
        }) {
            return false;
        }
        let Ok(mut current) = self.track_stacks.lock() else {
            return false;
        };
        *current = candidate;
        drop(current);
        if let Ok(mut bases) = self.track_stack_base_volumes.lock() {
            bases.clear();
        }
        self.apply_track_stack_gains()
    }

    /// Recomputes native member faders from their unscaled project values.
    /// Stack gain is therefore idempotent and overlapping stacks remain
    /// deterministic instead of multiplying an already-scaled fader.
    fn apply_track_stack_gains(&self) -> bool {
        let Ok(stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Ok(mut bases) = self.track_stack_base_volumes.lock() else {
            return false;
        };
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let mut multipliers = bases
            .keys()
            .copied()
            .map(|track_id| (track_id, 1.0))
            .collect::<HashMap<_, _>>();
        for stack in stacks.iter() {
            for track_id in &stack.member_track_ids {
                multipliers
                    .entry(*track_id)
                    .and_modify(|value| *value *= stack.master_gain)
                    .or_insert(stack.master_gain);
                let entry = bases
                    .entry(*track_id)
                    .or_insert_with(|| engine.get_track_volume(*track_id));
                if !entry.is_finite() {
                    return false;
                }
            }
        }
        multipliers.into_iter().all(|(track_id, multiplier)| {
            let Some(base) = bases.get(&track_id).copied() else {
                return false;
            };
            engine.set_track_volume(track_id, (base * multiplier).clamp(0.0, 2.0))
        })
    }

    /// Applies a user-facing member fader edit while preserving the stack
    /// multiplier. This prevents the next stack edit from snapping the fader
    /// back to an older baseline.
    pub fn set_track_volume_with_stack(&self, track_id: u32, value: f32) -> bool {
        if !value.is_finite() || !(0.0..=2.0).contains(&value) {
            return false;
        }
        let (has_stack, multiplier) = self
            .track_stacks
            .lock()
            .ok()
            .map(|stacks| {
                let matching = stacks
                    .iter()
                    .filter(|stack| stack.member_track_ids.contains(&track_id))
                    .collect::<Vec<_>>();
                (
                    !matching.is_empty(),
                    matching
                        .iter()
                        .fold(1.0f32, |product, stack| product * stack.master_gain),
                )
            })
            .unwrap_or((false, 1.0));
        if !has_stack {
            return self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.set_track_volume(track_id, value));
        }
        let Ok(mut bases) = self.track_stack_base_volumes.lock() else {
            return false;
        };
        let base = if multiplier > f32::EPSILON {
            value / multiplier
        } else {
            // A zero stack gain is effectively a mute. Preserve the user's
            // member-fader edit as the future baseline instead of losing it.
            value
        };
        if !base.is_finite() {
            return false;
        }
        let base = base.clamp(0.0, 2.0);
        bases.insert(track_id, base);
        let applied = (base * multiplier).clamp(0.0, 2.0);
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_volume(track_id, applied))
    }

    /// Creates or replaces one Track Stack atomically on the control plane.
    /// Native member faders are republished from their stable baselines after
    /// the metadata change, so callers never observe a half-written stack.
    pub fn upsert_track_stack(
        &self,
        id: u32,
        name: &str,
        member_track_ids: &[u32],
        master_gain: f32,
        collapsed: bool,
    ) -> bool {
        if id == 0
            || name.trim().is_empty()
            || name.len() > 256
            || !master_gain.is_finite()
            || !(0.0..=2.0).contains(&master_gain)
            || member_track_ids.is_empty()
        {
            return false;
        }
        let mut members = HashSet::with_capacity(member_track_ids.len());
        if member_track_ids
            .iter()
            .any(|track_id| *track_id == 0 || !members.insert(*track_id))
        {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let previous = stacks.clone();
        let candidate = TrackStackContract {
            id,
            name: name.trim().chars().take(256).collect(),
            member_track_ids: {
                let mut ids = member_track_ids.to_vec();
                ids.sort_unstable();
                ids
            },
            master_gain,
            collapsed,
        };
        if let Some(existing) = stacks.iter_mut().find(|stack| stack.id == id) {
            *existing = candidate;
        } else {
            stacks.push(candidate);
            stacks.sort_by_key(|stack| stack.id);
        }
        drop(stacks);
        if let (Ok(mut bases), Some(engine)) =
            (self.track_stack_base_volumes.lock(), self.engine.as_ref())
        {
            for track_id in member_track_ids {
                bases
                    .entry(*track_id)
                    .or_insert_with(|| engine.get_track_volume(*track_id));
            }
        }
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            *stacks = previous;
        }
        let _ = self.apply_track_stack_gains();
        false
    }

    pub fn set_track_stack_gain(&self, id: u32, master_gain: f32) -> bool {
        if !master_gain.is_finite() || !(0.0..=2.0).contains(&master_gain) {
            return false;
        }
        let previous_gain = {
            let Ok(mut stacks) = self.track_stacks.lock() else {
                return false;
            };
            let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) else {
                return false;
            };
            let previous_gain = stack.master_gain;
            stack.master_gain = master_gain;
            previous_gain
        };
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            if let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) {
                stack.master_gain = previous_gain;
            }
        }
        let _ = self.apply_track_stack_gains();
        false
    }

    pub fn set_track_stack_collapsed(&self, id: u32, collapsed: bool) -> bool {
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Some(stack) = stacks.iter_mut().find(|stack| stack.id == id) else {
            return false;
        };
        stack.collapsed = collapsed;
        true
    }

    pub fn delete_track_stack(&self, id: u32) -> bool {
        if id == 0 {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let before = stacks.len();
        stacks.retain(|stack| stack.id != id);
        if stacks.len() == before {
            return false;
        }
        drop(stacks);
        self.apply_track_stack_gains()
    }

    pub fn add_track_stack_member(&self, stack_id: u32, track_id: u32) -> bool {
        if stack_id == 0 || track_id == 0 {
            return false;
        }
        let Ok(mut stacks) = self.track_stacks.lock() else {
            return false;
        };
        let Some(stack) = stacks.iter_mut().find(|stack| stack.id == stack_id) else {
            return false;
        };
        if stack.member_track_ids.contains(&track_id) {
            return true;
        }
        stack.member_track_ids.push(track_id);
        stack.member_track_ids.sort_unstable();
        drop(stacks);
        if let (Ok(mut bases), Some(engine)) =
            (self.track_stack_base_volumes.lock(), self.engine.as_ref())
        {
            bases
                .entry(track_id)
                .or_insert_with(|| engine.get_track_volume(track_id));
        }
        if self.apply_track_stack_gains() {
            return true;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            if let Some(stack) = stacks.iter_mut().find(|stack| stack.id == stack_id) {
                stack.member_track_ids.retain(|member| *member != track_id);
            }
        }
        false
    }

    pub fn new() -> anyhow::Result<Self> {
        // Unit tests must never race the machine's physical audio device.
        // Relying only on an environment variable is order-dependent: a test
        // can construct AuraCore before the shared test guard has a chance to
        // set it.  Compile-time test isolation makes bare `cargo test` safe as
        // well as the CMake/CI entry points that set the variable explicitly.
        let isolated =
            cfg!(test) || std::env::var("AURA_NATIVE_TEST_ISOLATION").as_deref() == Ok("1");
        Self::new_with_mode(isolated)
    }

    /// Construct the Core without opening a physical audio device.
    ///
    /// CLI, server, test, and automation clients should use this entry point
    /// so headless operation never depends on machine audio configuration.
    pub fn new_offline() -> anyhow::Result<Self> {
        Self::new_with_mode(true)
    }

    fn new_with_mode(isolated: bool) -> anyhow::Result<Self> {
        let engine = if isolated {
            ffi::new_audio_engine_offline()
        } else {
            ffi::new_audio_engine()
        };
        if engine.is_null() {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: Failed to initialize AudioEngine (nullptr)"
            ));
        }

        // Explicitly initialize GPU before start (Point 2)
        let gpu_initialized = ffi::initialize_gpu_with_status();
        if !gpu_initialized {
            report_aura_log(
                1,
                "GPU initialization failed; continuing with CPU fallbacks.",
            );
        }

        let Some(engine_ref) = engine.as_ref() else {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: AudioEngine handle is invalid"
            ));
        };
        if isolated {
            engine_ref.new_project();
            let _ = engine_ref.remove_track(0);
        }
        // Engine bootstrap/template setup is not a user edit and must not
        // appear as the first undoable action of a new Core instance.
        engine_ref.clear_undo_history();
        let analysis = ffi::new_analysis_hub(engine_ref);
        if analysis.is_null() {
            return Err(anyhow::anyhow!(
                "AURA | FATAL: Failed to initialize AnalysisHub (nullptr)"
            ));
        }

        Ok(Self {
            engine,
            analysis,
            gpu_initialized,
            preview_audio: std::sync::Mutex::new(preview_audio_runtime::PreviewAudioRuntime::new()),
            recording_session: std::sync::Mutex::new(None),
            comping: std::sync::Mutex::new(comping::CompingOrchestrator::new()),
            aux_track_ids: std::sync::Mutex::new(HashSet::new()),
            midi_events: std::sync::Mutex::new(Vec::new()),
            scheduled_midi_notes: std::sync::Mutex::new(Vec::new()),
            midi_monitor: std::sync::Mutex::new(crate::midi_monitor::MidiMonitor::new()),
            review_notes: std::sync::Mutex::new(crate::review_notes::ReviewNoteStore::new()),
            control_room: std::sync::Mutex::new(crate::control_room::ControlRoomState::default()),
            chord_track: std::sync::Mutex::new(Vec::new()),
            midi_lyric_history: std::sync::Mutex::new(Vec::new()),
            chord_history: std::sync::Mutex::new(Vec::new()),
            chord_redo_history: std::sync::Mutex::new(Vec::new()),
            comping_history: std::sync::Mutex::new(Vec::new()),
            comping_redo_history: std::sync::Mutex::new(Vec::new()),
            plugin_parameter_events: std::sync::Mutex::new(Vec::with_capacity(64)),
            project_transaction: std::sync::Mutex::new(()),
            openutau_vocals: std::sync::Mutex::new(Vec::new()),
            track_stacks: std::sync::Mutex::new(Vec::new()),
            track_stack_base_volumes: std::sync::Mutex::new(HashMap::new()),
            markers: std::sync::Mutex::new(vec![
                MarkerContract {
                    id: 1,
                    label: "START".to_owned(),
                    beat: 0.0,
                    color: "#646496".to_owned(),
                },
                MarkerContract {
                    id: 2,
                    label: "DEVELOPMENT".to_owned(),
                    beat: 32.0,
                    color: "#966464".to_owned(),
                },
            ]),
            macro_mappings: std::sync::Mutex::new(Vec::new()),
            midi_learn_mappings: std::sync::Mutex::new(Vec::new()),
            mix_snapshots: std::sync::Mutex::new(crate::snapshots::SnapshotOrchestrator::new()),
            midi_pickup_acquired: std::sync::Mutex::new(HashSet::new()),
            production_events: std::sync::Arc::new(std::sync::Mutex::new(
                crate::production_events::EventHub::default(),
            )),
        })
    }

    pub(crate) fn production_event_hub(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::production_events::EventHub>> {
        self.production_events.clone()
    }

    pub(crate) fn publish_production_event(
        &self,
        event: crate::production_events::ProductionEvent,
    ) {
        if let Ok(mut hub) = self.production_events.lock() {
            let _ = hub.publish(self.project_generation(), event);
        }
    }

    /// Returns a lifetime-bound handle to the analysis hub.
    /// This enforces that the hub cannot be used if the core is mutated or dropped.
    pub fn analysis(&self) -> Option<&ffi::AnalysisHub> {
        self.analysis.as_ref()
    }

    pub fn gpu_initialized(&self) -> bool {
        self.gpu_initialized
    }

    pub fn audio_config_generation(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_audio_config_generation())
    }

    /// Stable diagnostic envelope for UI, CLI, and future FFI callers.
    /// This keeps driver failure details and generation context together.
    pub fn audio_driver_diagnostic_json(&self) -> String {
        let reported_status = self.audio_driver_status();
        // The host's lifecycle state can briefly remain `running` while the
        // underlying callback has already stopped. Diagnostics must describe
        // the observable device state, not only the control-side state flag.
        let callback_ready = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.is_audio_device_ready());
        let status = if reported_status == "running" && !callback_ready {
            "stopped".to_owned()
        } else {
            reported_status
        };
        let error = match status.as_str() {
            "initialized" | "running" => None,
            "start-failed" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_start_failed",
                    "Audio device failed to start",
                )
                .retryable(true),
            ),
            "silent-fallback" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_silent_fallback",
                    "Audio is running in silent fallback mode",
                )
                .retryable(true),
            ),
            "stopped" => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_stopped",
                    "Audio device is stopped",
                )
                .retryable(true),
            ),
            _ => Some(
                crate::bridge_error::BridgeError::new(
                    "audio_device_unavailable",
                    "Audio device is unavailable",
                )
                .retryable(true),
            ),
        };
        serde_json::json!({
            "ok": error.is_none() && callback_ready,
            "status": status,
            "error_code": self
                .engine
                .as_ref()
                .map_or(0, |engine| engine.audio_driver_error_code()),
            "error": error,
            "project_generation": self.project_generation(),
            "audio_generation": self.audio_config_generation(),
        })
        .to_string()
    }

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

impl Drop for AuraCore {
    fn drop(&mut self) {
        // Cancel bridge-owned offline work before the handle disappears. The
        // native singleton remains alive for other bridge handles, while this
        // handle's driver and render requests are stopped deterministically.
        self.cancel_render();
        if let Some(engine) = self.engine.as_ref() {
            engine.shutdown();
        }
    }
}

include!("aura_core_methods_1.rs");
include!("aura_core_methods_2.rs");
include!("aura_core_methods_3.rs");
include!("aura_core_methods_4.rs");

#[cfg(test)]
mod tests {
    include!("lib_tests.rs");
}
