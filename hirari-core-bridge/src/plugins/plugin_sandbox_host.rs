//! Rust-owned process supervisor for the isolated plugin worker.
//!
//! The worker (a separate OS process) is the only component allowed to load
//! VST3/AU/CLAP binaries. This module owns everything on the host side of that
//! boundary: spawning and reaping the helper, the control/status pipes, the
//! shared-memory mailbox, request/response framing and validation, timeouts,
//! restart policy, and teardown.
//!
//! The C++ side is the engine's `ProcessSandboxProcessor` DSP boundary only; it
//! names this host directly instead of forwarding through another C++ wrapper.

use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hirari_plugin_protocol::{
    hirari_midi_extended_ring_init, hirari_midi_extended_ring_push, state_checksum, validate_state,
    MAX_STATE_BYTES, RING_STORAGE_BYTES, STATE_ERROR_NONE, STATE_PROTOCOL_VERSION,
};

pub use hirari_plugin_protocol::mailbox::{
    MAX_CHANNELS, MAX_FRAMES, MAX_MIDI_EVENTS, MAX_MIDI_PAYLOAD_BYTES, MAX_PARAMETER_CHANGES,
};
pub use hirari_plugin_protocol::mailbox::AUDIO_BLOCK_PROTOCOL_VERSION;

pub const FAILURE_NONE: u8 = 0;
pub const FAILURE_MISSING_HELPER: u8 = 1;
pub const FAILURE_INVALID_PLUGIN_PATH: u8 = 2;
pub const FAILURE_SPAWN_FAILED: u8 = 3;
pub const FAILURE_HANDSHAKE_TIMEOUT: u8 = 4;
pub const FAILURE_PLUGIN_LOAD_FAILED: u8 = 5;
pub const FAILURE_PLUGIN_ABI_INVALID: u8 = 6;
pub const FAILURE_PLUGIN_INSTANCE_FAILED: u8 = 7;
pub const FAILURE_SECURITY_SETUP_FAILED: u8 = 8;
pub const FAILURE_PLUGIN_FORMAT_UNSUPPORTED: u8 = 9;
pub const FAILURE_PLUGIN_QUARANTINED: u8 = 10;
pub const FAILURE_PROCESS_EXITED: u8 = 11;
pub const FAILURE_PROCESS_HUNG: u8 = 12;
pub const FAILURE_PROCESS_FAILED: u8 = 13;
pub const FAILURE_UNSUPPORTED_PLATFORM: u8 = 14;

pub const STATE_ERROR_NONE_CODE: u8 = 0;
pub const STATE_ERROR_OVERSIZE: u8 = 1;
pub const STATE_ERROR_VERSION: u8 = 2;
pub const STATE_ERROR_CHECKSUM: u8 = 3;
pub const STATE_ERROR_PLUGIN: u8 = 4;
pub const STATE_ERROR_UNAVAILABLE: u8 = 5;
pub const STATE_ERROR_BUSY: u8 = 6;
pub const STATE_ERROR_TIMEOUT: u8 = 7;

// The mailbox layout has exactly one owner: `hirari_plugin_protocol::mailbox`.
// The host and the isolated worker map the same shm object, so a second
// definition here could drift silently and corrupt the protocol.
pub use hirari_plugin_protocol::mailbox::{
    SharedAudioBlock, WireMidiEvent, WireParameterChange,
};

#[derive(Clone, Copy)]
struct Generation {
    project: u64,
    plugin: u64,
    audio: u64,
    state: u64,
}

impl Generation {
    fn valid(self) -> bool {
        self.project != 0 && self.plugin != 0 && self.audio != 0 && self.state != 0
    }
}

struct AudioState {
    sequence: u64,
    last_submitted: u64,
    pending_input: [[f32; MAX_FRAMES]; MAX_CHANNELS],
    pending_midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    pending_midi_count: u32,
    pending_parameter_count: u32,
    pending_channels: u32,
    pending_frames: u32,
    pending_age_frames: u64,
    overrun_reported_for_sequence: bool,
    sequence_had_overrun: bool,
    completed_sequence_had_overrun: bool,
    output_midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    output_midi_count: u32,
}

impl AudioState {
    fn zeroed() -> Self {
        // All fields are integer/float/bool aggregates for which the all-zero
        // bit pattern is a valid, fully initialized value.
        unsafe { std::mem::zeroed() }
    }

    fn reset_for_new_worker(&mut self) {
        self.sequence = 0;
        self.last_submitted = 0;
        self.pending_midi_count = 0;
        self.pending_parameter_count = 0;
        self.pending_channels = 0;
        self.pending_frames = 0;
        self.pending_age_frames = 0;
        self.overrun_reported_for_sequence = false;
        self.sequence_had_overrun = false;
        self.completed_sequence_had_overrun = false;
        self.output_midi_count = 0;
    }
}

struct ParameterQueue {
    slots: UnsafeCell<[WireParameterChange; MAX_PARAMETER_CHANGES]>,
    write: AtomicU64,
    read: AtomicU64,
}

pub(crate) struct StateControl {
    generation: Generation,
    sequence: u64,
    cache: Vec<u8>,
}

pub(crate) struct Control {
    plugin_path: String,
    requested_format: String,
    sample_rate: f64,
    max_block_size: u32,
    channels: u32,
    pid: i32,
    control_fd: i32,
    status_fd: i32,
    shared_fd: i32,
    shared_name: String,
    mapped: *mut SharedAudioBlock,
    mapped_len: usize,
    restart_attempts: u32,
    next_restart_time: Option<Instant>,
    last_heartbeat: u64,
    last_completed_sequence: u64,
    last_heartbeat_time: Instant,
    last_progress_time: Instant,
}

/// Opaque host handle. The engine's audio thread calls `process`/`stage_*`
/// through a shared reference while control-thread lifecycle methods are
/// serialized by `control`; all cross-thread state is atomic or guarded.
pub struct SandboxHost {
    control: Mutex<Control>,
    state: Mutex<StateControl>,
    shared: AtomicPtr<SharedAudioBlock>,
    process_alive: AtomicBool,
    process_calls: AtomicU32,
    shared_users: AtomicU32,
    state_transition: AtomicBool,
    failure: AtomicU8,
    last_state_error: AtomicU8,
    last_process_overrun: AtomicBool,
    sample_rate_bits: AtomicU64,
    configured_channels: AtomicU32,
    audio: UnsafeCell<AudioState>,
    parameters: ParameterQueue,
    // Lifecycle recovery state. The processor object in C++ only forwards the
    // facts (late block, missing output); the streak, the latch and the
    // prepared configuration are decided here.
    recovery_failed: AtomicBool,
    consecutive_overruns: AtomicU32,
    prepared_rate_bits: AtomicU64,
    prepared_block: AtomicU32,
    /// Processor latency in samples, published by the worker at load time
    /// and read by the PDC solve through the chain node.
    latency_samples: AtomicU32,
    /// Sidechain delivery context: engine gate plus the (destination track,
    /// plugin index) link key. Null gate disables sidechain input.
    sidechain_gate: AtomicPtr<c_void>,
    sidechain_track: AtomicU32,
    sidechain_plugin: AtomicU32,
    parameter_cache: crate::plugin_parameter_snapshot::Snapshot,
}

// SAFETY: The audio-thread-owned `audio` cell is only touched by `process`,
// `reset_staged_midi`, `stage_input_midi`, and the output accessors, which the
// engine calls from its single audio callback. Control-thread lifecycle
// operations serialize through `control`; shared-memory operations also pin
// the mapping with `shared_users` until their access ends.
unsafe impl Send for SandboxHost {}
unsafe impl Sync for SandboxHost {}

static SHARED_NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

fn lock_control<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn sequence_reached(completed: u64, requested: u64) -> bool {
    (completed.wrapping_sub(requested) as i64) >= 0
}

impl SandboxHost {
    fn new(
        plugin_path: String,
        sample_rate: f64,
        max_block_size: u32,
        channels: u32,
        requested_format: String,
    ) -> Box<Self> {
        let now = Instant::now();
        let normalized_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        Box::new(Self {
            control: Mutex::new(Control {
                plugin_path,
                requested_format,
                sample_rate: normalized_rate,
                max_block_size: max_block_size.clamp(1, MAX_FRAMES as u32),
                channels: channels.clamp(1, MAX_CHANNELS as u32),
                pid: -1,
                control_fd: -1,
                status_fd: -1,
                shared_fd: -1,
                shared_name: String::new(),
                mapped: std::ptr::null_mut(),
                mapped_len: std::mem::size_of::<SharedAudioBlock>(),
                restart_attempts: 0,
                next_restart_time: None,
                last_heartbeat: 0,
                last_completed_sequence: 0,
                last_heartbeat_time: now,
                last_progress_time: now,
            }),
            state: Mutex::new(StateControl {
                generation: Generation {
                    project: 0,
                    plugin: 0,
                    audio: 0,
                    state: 0,
                },
                sequence: 0,
                cache: Vec::new(),
            }),
            shared: AtomicPtr::new(std::ptr::null_mut()),
            process_alive: AtomicBool::new(false),
            process_calls: AtomicU32::new(0),
            shared_users: AtomicU32::new(0),
            state_transition: AtomicBool::new(false),
            failure: AtomicU8::new(FAILURE_NONE),
            last_state_error: AtomicU8::new(STATE_ERROR_NONE_CODE),
            last_process_overrun: AtomicBool::new(false),
            sample_rate_bits: AtomicU64::new(normalized_rate.to_bits()),
            configured_channels: AtomicU32::new(channels.clamp(1, MAX_CHANNELS as u32)),
            audio: UnsafeCell::new(AudioState::zeroed()),
            parameters: ParameterQueue {
                slots: UnsafeCell::new([WireParameterChange {
                    parameter_id: 0,
                    sample_offset: 0,
                    value: 0.0,
                }; MAX_PARAMETER_CHANGES]),
                write: AtomicU64::new(0),
                read: AtomicU64::new(0),
            },
            recovery_failed: AtomicBool::new(false),
            consecutive_overruns: AtomicU32::new(0),
            prepared_rate_bits: AtomicU64::new(normalized_rate.to_bits()),
            prepared_block: AtomicU32::new(max_block_size.clamp(1, MAX_FRAMES as u32)),
            latency_samples: AtomicU32::new(0),
            sidechain_gate: AtomicPtr::new(std::ptr::null_mut()),
            sidechain_track: AtomicU32::new(0),
            sidechain_plugin: AtomicU32::new(0),
            parameter_cache: crate::plugin_parameter_snapshot::Snapshot::new(),
        })
    }

    #[inline]
    fn audio_state(&self) -> &AudioState {
        // SAFETY: Audio-thread-only state; see the `SandboxHost` comment.
        unsafe { &*self.audio.get() }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)] // audio-thread-only interior state by design
    fn audio_state_mut(&self) -> &mut AudioState {
        // SAFETY: Audio-thread-only state; see the `SandboxHost` comment.
        unsafe { &mut *self.audio.get() }
    }

    #[inline]
    fn shared(&self) -> *mut SharedAudioBlock {
        self.shared.load(Ordering::Acquire)
    }

    fn acquire_shared_access(&self) -> Option<SharedAccess<'_>> {
        // Keep access registration and gate checks in one total order with
        // teardown's close-and-drain sequence.
        self.shared_users.fetch_add(1, Ordering::SeqCst);
        if !self.process_alive.load(Ordering::SeqCst)
            || self.state_transition.load(Ordering::SeqCst)
        {
            self.shared_users.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let ptr = self.shared.load(Ordering::Acquire);
        if ptr.is_null() {
            self.shared_users.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(SharedAccess { host: self, ptr })
    }

    fn acquire_audio_access(&self) -> Option<AudioAccess<'_>> {
        self.process_calls.fetch_add(1, Ordering::SeqCst);
        if !self.process_alive.load(Ordering::SeqCst)
            || self.state_transition.load(Ordering::SeqCst)
        {
            self.process_calls.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(AudioAccess { host: self })
    }

    #[inline]
    fn is_alive(&self) -> bool {
        self.process_alive.load(Ordering::Acquire)
    }

    #[inline]
    fn failure(&self) -> u8 {
        self.failure.load(Ordering::Acquire)
    }

    #[inline]
    fn sample_rate(&self) -> f64 {
        f64::from_bits(self.sample_rate_bits.load(Ordering::Acquire))
    }
}

#[path = "plugin_sandbox_lifecycle.rs"]
mod lifecycle;
#[path = "plugin_sandbox_process.rs"]
mod process;

pub(crate) struct TransitionGuard<'a> {
    flag: &'a AtomicBool,
}

pub(crate) struct SharedAccess<'a> {
    host: &'a SandboxHost,
    ptr: *mut SharedAudioBlock,
}

pub(crate) struct AudioAccess<'a> {
    host: &'a SandboxHost,
}

impl Drop for AudioAccess<'_> {
    fn drop(&mut self) {
        self.host.process_calls.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for SharedAccess<'_> {
    fn drop(&mut self) {
        self.host.shared_users.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<'a> TransitionGuard<'a> {
    fn enter(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[cfg(unix)]
fn usleep(microseconds: u32) {
    unsafe {
        libc::usleep(microseconds as libc::c_uint);
    }
}

#[cfg(not(unix))]
fn usleep(_microseconds: u32) {}

#[cfg(unix)]
fn close_fd(fd: i32) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(unix)]
fn duplicate_spawn_source(descriptor: i32) -> i32 {
    unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 100) }
}

#[cfg(unix)]
fn wait_for_child(child: i32, reaped: &mut bool) -> Option<bool> {
    let mut status = 0;
    let result = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
    if result == child {
        *reaped = true;
        return Some(true);
    }
    if result == 0 {
        return Some(false);
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if error == libc::EINTR || error == libc::ECHILD {
        None
    } else {
        Some(false)
    }
}

#[cfg(unix)]
fn spawn_environment() -> (Vec<CString>, Vec<*mut c_char>) {
    use std::os::unix::ffi::OsStrExt;
    let mut owned: Vec<CString> = Vec::new();
    for (key, value) in std::env::vars_os() {
        let mut entry = Vec::new();
        entry.extend_from_slice(key.as_os_str().as_bytes());
        entry.push(b'=');
        entry.extend_from_slice(value.as_os_str().as_bytes());
        if let Ok(entry) = CString::new(entry) {
            owned.push(entry);
        }
    }
    let mut pointers: Vec<*mut c_char> = owned
        .iter()
        .map(|entry| entry.as_ptr() as *mut c_char)
        .collect();
    pointers.push(std::ptr::null_mut());
    (owned, pointers)
}

#[cfg(unix)]
fn resolve_helper_path() -> Option<String> {
    if let Ok(configured) = std::env::var("HIRARI_PLUGIN_HOST_BIN") {
        if !configured.is_empty() && is_executable(&configured) {
            return Some(configured);
        }
    }
    // Match the previous `_NSGetExecutablePath` behavior on macOS and the
    // sibling-lookup contract on other Unix targets.
    let executable = std::env::current_exe().ok()?;
    let bundled = executable.with_file_name("hirari-plugin-host-worker");
    let bundled = bundled.to_str()?;
    if is_executable(bundled) {
        Some(bundled.to_string())
    } else {
        None
    }
}

#[cfg(unix)]
fn is_executable(path: &str) -> bool {
    let Ok(name) = CString::new(path) else {
        return false;
    };
    unsafe { libc::access(name.as_ptr(), libc::X_OK) == 0 }
}

fn ready_timeout_ms() -> i32 {
    const DEFAULT_READY_TIMEOUT_MS: i64 = 30_000;
    match std::env::var("HIRARI_PLUGIN_READY_TIMEOUT_MS") {
        Ok(configured) => configured
            .parse::<i64>()
            .ok()
            .map(|value| value.clamp(1000, 120_000) as i32)
            .unwrap_or(DEFAULT_READY_TIMEOUT_MS as i32),
        Err(_) => DEFAULT_READY_TIMEOUT_MS as i32,
    }
}

fn format_for_path(path: &str) -> &'static str {
    let normalized = path.to_ascii_lowercase();
    if normalized.starts_with("builtin://") {
        "builtin"
    } else if normalized.ends_with(".clap") {
        "clap"
    } else if normalized.ends_with(".vst3") {
        "vst3"
    } else if normalized.ends_with(".component") {
        "au"
    } else {
        "auto"
    }
}

const MAX_SANDBOX_PLUGIN_PATH_BYTES: usize = 4096;

/// Byte-level counterpart of `format_for_path` for engine callers: the path
/// may be non-UTF-8, so acceptance only compares ASCII prefixes/suffixes.
fn supported_bundle_path(bytes: &[u8]) -> bool {
    if bytes.is_empty()
        || bytes.len() > MAX_SANDBOX_PLUGIN_PATH_BYTES
        || bytes.contains(&0)
    {
        return false;
    }
    let ends_with = |suffix: &[u8]| {
        bytes.len() >= suffix.len() && bytes[bytes.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    };
    (bytes.len() >= b"builtin://".len()
        && bytes[..b"builtin://".len()].eq_ignore_ascii_case(b"builtin://"))
        || ends_with(b".vst3")
        || ends_with(b".component")
        || ends_with(b".clap")
}

/// Reports whether the engine should attempt to load this path as a sandboxed
/// plugin bundle. This module owns the accepted formats, so the engine no
/// longer normalises and re-checks the suffix itself.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_path_supported(
    path: *const u8,
    path_len: usize,
) -> bool {
    if path.is_null() || path_len == 0 {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, path_len) };
    supported_bundle_path(bytes)
}

#[cfg(test)]
#[path = "plugin_sandbox_host_path_policy_tests.rs"]
mod plugin_sandbox_host_path_policy_tests;


/// Parses one `--params` probe document into validated parameter records.
/// Malformed entries are skipped; the probe stays fail-open (an empty list
/// just means "no discovery", never a load failure).
pub(crate) fn parse_discovered_parameters(
    json: &str,
) -> Vec<crate::plugin_parameters::PluginParameter> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(parameters) = document.get("parameters").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    parameters
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_u64()? as u32;
            let name = entry.get("name")?.as_str()?.to_owned();
            let unit = entry.get("unit")?.as_str().unwrap_or("").to_owned();
            let min = entry.get("min")?.as_f64().unwrap_or(0.0);
            let max = entry.get("max")?.as_f64().unwrap_or(1.0);
            let default = entry.get("default")?.as_f64().unwrap_or(0.0);
            let flags = entry.get("flags")?.as_u64().unwrap_or(0);
            let parameter = crate::plugin_parameters::PluginParameter {
                id,
                name,
                unit,
                min,
                max,
                default,
                automatable: flags & 1 != 0,
                read_only: flags & 2 != 0,
            };
            parameter.validate().then_some(parameter)
        })
        .collect()
}

fn worker_format(plugin_path: &str, requested_format: &str) -> String {
    if requested_format.is_empty() || requested_format == "auto" {
        format_for_path(plugin_path).to_string()
    } else {
        requested_format.to_ascii_lowercase()
    }
}

fn new_sandbox_host(
    plugin_path: &str,
    sample_rate: f64,
    max_block_size: u32,
    channels: u32,
    requested_format: &str,
) -> Box<SandboxHost> {
    SandboxHost::new(
        plugin_path.to_string(),
        sample_rate,
        max_block_size,
        channels,
        requested_format.to_string(),
    )
}

/// Chain-owned worker node: the effect chain holds one of these per isolated
/// plugin slot and calls it directly. The C++ engine used to wrap the same host
/// in a `DSP::IProcessor` subclass; the marshalling below is that wrapper.
#[path = "plugin_sandbox_chain.rs"]
pub mod chain;
