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

include!("lib_runtime_audio_io.rs");

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
    /// Per-note vibrato-rate overrides kept separately so legacy MIDI note
    /// documents remain wire-compatible while the editor can still update
    /// articulation in place.
    midi_vibrato_rates: std::sync::Mutex<HashMap<(u32, u8, u64), u16>>,
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
    /// Project-scoped external transport state shared by the sync UI and
    /// device adapters. The native clock remains sample-accurate; this
    /// control-plane mirror makes MMC/MTC state observable and persistent
    /// within the running session.
    pub(crate) external_sync: std::sync::Mutex<crate::sync_transport::ExternalSyncController>,
    pub(crate) export_queue: std::sync::Mutex<crate::export::ExportOrchestrator>,
    /// Advanced stem queue wired to the same native project bounce graph as
    /// the ordinary UI/CLI render path.
    pub(crate) advanced_export: std::sync::Mutex<crate::advanced_export_engine::ExportOrchestrator>,
    /// SDK-neutral ARA2 document lifecycle. An external ARA2 adapter can bind
    /// its callbacks here without coupling the Core to a proprietary SDK.
    pub(crate) ara2_protocol: std::sync::Mutex<crate::ara2_protocol::Ara2ProtocolEndpoint>,
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

include!("lib_runtime_plugins.rs");
include!("lib_runtime_markers.rs");
include!("lib_runtime_init.rs");
include!("lib_runtime_sandbox.rs");

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

include!("aura_core_recording_methods.rs");
include!("aura_core_engine_methods.rs");
include!("aura_core_arrangement_methods.rs");
include!("aura_core_project_methods.rs");
include!("aura_core_ara2_methods.rs");
include!("ui_core_compat.rs");

#[cfg(test)]
mod tests {
    include!("lib_tests.rs");
}
