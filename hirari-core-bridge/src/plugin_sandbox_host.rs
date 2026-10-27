//! Rust-owned process supervisor for the isolated plugin worker.
//!
//! The worker (a separate OS process) is the only component allowed to load
//! VST3/AU/CLAP binaries. This module owns everything on the host side of that
//! boundary: spawning and reaping the helper, the control/status pipes, the
//! shared-memory mailbox, request/response framing and validation, timeouts,
//! restart policy, and teardown.
//!
//! The C++ `PluginSandboxHost` is only a forwarding facade. It exists because
//! the engine and the native contracts name that type directly.

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

pub const MAX_FRAMES: usize = 8192;
pub const MAX_CHANNELS: usize = 2;
pub const MAX_MIDI_EVENTS: usize = 1024;
pub const MAX_PARAMETER_CHANGES: usize = 4096;
pub const MAX_MIDI_PAYLOAD_BYTES: usize = 256;
pub const AUDIO_BLOCK_PROTOCOL_VERSION: u32 = 5;

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

#[cxx::bridge(namespace = "Hirari::Core::Bridge::Sandbox")]
mod ffi {
    struct SandboxGeneration {
        project: u64,
        plugin: u64,
        audio: u64,
        state: u64,
    }

    extern "Rust" {
        type SandboxHost;

        fn new_sandbox_host(
            plugin_path: &str,
            sample_rate: f64,
            max_block_size: u32,
            channels: u32,
            requested_format: &str,
        ) -> Box<SandboxHost>;

        fn start(&self) -> bool;
        fn stop(&self);
        fn request_plugin_reset(&self) -> bool;
        fn is_alive(&self) -> bool;
        fn poll_health(&self) -> bool;
        fn restart(&self, force: bool) -> bool;
        fn auto_restart(&self) -> bool;
        fn reconfigure(&self, sample_rate: f64, max_block_size: u32) -> bool;
        fn failure(&self) -> u8;
        fn is_quarantined(&self) -> bool;
        fn restart_attempts(&self) -> u32;
        fn clear_quarantine(&self);
        fn can_retry(&self) -> bool;
        fn can_auto_retry(&self) -> bool;

        fn set_generation_context(
            &self,
            project: u64,
            plugin: u64,
            audio: u64,
            state: u64,
        ) -> bool;
        fn generation_context(&self) -> SandboxGeneration;

        fn enqueue_extended_midi(
            &self,
            sample_offset: u64,
            articulation_id: u8,
            data: &[u8],
        ) -> bool;
        fn enqueue_parameter_change(
            &self,
            parameter_id: u32,
            value: f64,
            sample_offset: u32,
        ) -> bool;

        fn reset_staged_midi(&self);
        fn stage_input_midi(
            &self,
            sample_offset: u64,
            articulation_id: u8,
            data: &[u8],
            frames: u32,
        ) -> bool;
        fn process(&self, left: &mut [f32], right: &mut [f32], channels: u32, frames: u32) -> u8;
        fn output_midi_count(&self) -> u32;
        fn copy_output_midi(
            &self,
            index: u32,
            sample_offset: &mut u64,
            articulation_id: &mut u8,
            data: &mut [u8],
        ) -> u32;
        fn take_last_process_overrun(&self) -> bool;
        fn take_completed_sequence_overrun(&self) -> bool;

        fn take_dropped_output_midi(&self) -> u32;
        fn take_mailbox_overruns(&self) -> u32;
        fn take_input_midi_truncations(&self) -> u32;
        fn record_input_midi_rejection(&self);
        fn take_process_errors(&self) -> u32;
        fn heartbeat(&self) -> u64;
        fn active_sample_rate(&self) -> u32;
        fn active_channels(&self) -> u32;
        fn is_midi_instrument(&self) -> bool;

        fn set_state(&self, data: &[u8]) -> bool;
        fn get_state(&self) -> Vec<u8>;
        fn state_error_code(&self) -> u8;
    }
}

pub use ffi::SandboxGeneration;

// The shared mailbox is a byte-for-byte mirror of `SandboxProtocol::SharedAudioBlock`.
// The worker (C++) and this host (Rust) map the same shm object, so any layout
// drift corrupts the protocol. `assert_shared_audio_block_layout` below locks
// the offsets measured from the C++ definition.
#[repr(C)]
#[derive(Clone, Copy)]
struct WireMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; MAX_MIDI_PAYLOAD_BYTES],
    articulation_id: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WireParameterChange {
    parameter_id: u32,
    sample_offset: u32,
    value: f64,
}

#[repr(C, align(64))]
struct ExtendedRingStorage {
    bytes: [u8; RING_STORAGE_BYTES],
}

#[repr(C, align(64))]
struct StereoBlock {
    samples: [[f32; MAX_FRAMES]; MAX_CHANNELS],
}

#[repr(C)]
struct SharedAudioBlock {
    protocol_version: AtomicU32,
    request_sequence: AtomicU64,
    completed_sequence: AtomicU64,
    heartbeat: AtomicU64,
    frames: AtomicU32,
    channels: AtomicU32,
    active_sample_rate: AtomicU32,
    active_channels: AtomicU32,
    midi_events: AtomicU32,
    parameter_changes: AtomicU32,
    output_midi_events: AtomicU32,
    output_midi_dropped: AtomicU32,
    mailbox_overruns: AtomicU32,
    input_midi_truncations: AtomicU32,
    process_errors: AtomicU32,
    midi_instrument: AtomicU8,
    state_request_sequence: AtomicU64,
    state_completed_sequence: AtomicU64,
    state_project_generation: AtomicU64,
    state_plugin_generation: AtomicU64,
    state_audio_generation: AtomicU64,
    state_generation: AtomicU64,
    state_size: AtomicU32,
    state_checksum: AtomicU64,
    state_version: AtomicU32,
    state_mode: AtomicU8,
    state_result: AtomicU8,
    state_error: AtomicU8,
    extended_midi: ExtendedRingStorage,
    input: StereoBlock,
    output: StereoBlock,
    midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    parameter_change: [WireParameterChange; MAX_PARAMETER_CHANGES],
    output_midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    state: [u8; MAX_STATE_BYTES],
}

// Offsets measured from `SandboxProtocol::SharedAudioBlock` in the C++ worker
// header. If this assertion fires after a protocol change, both sides must be
// updated together.
const _: () = assert!(std::mem::size_of::<SharedAudioBlock>() == 9_142_656);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, request_sequence) == 8);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, midi_instrument) == 76);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, state_request_sequence) == 80);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, state_size) == 128);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, state_checksum) == 136);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, state_error) == 150);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, extended_midi) == 192);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, input) == 4_194_688);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, output) == 4_260_224);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, midi) == 4_325_760);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, parameter_change) == 4_604_288);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, output_midi) == 4_669_824);
const _: () = assert!(std::mem::offset_of!(SharedAudioBlock, state) == 4_948_352);

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

struct StateControl {
    generation: Generation,
    sequence: u64,
    cache: Vec<u8>,
}

struct Control {
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

    fn start(&self) -> bool {
        let mut control = lock_control(&self.control);
        self.start_locked(&mut control)
    }

    #[cfg(not(unix))]
    fn start_locked(&self, _control: &mut Control) -> bool {
        self.failure
            .store(FAILURE_UNSUPPORTED_PLATFORM, Ordering::Release);
        false
    }

    #[cfg(unix)]
    fn start_locked(&self, control: &mut Control) -> bool {
        self.failure.store(FAILURE_NONE, Ordering::Release);
        if control.plugin_path.is_empty() {
            self.failure
                .store(FAILURE_INVALID_PLUGIN_PATH, Ordering::Release);
            return false;
        }
        // Direct sandbox users share the registry's admission boundary.
        // Built-in protocol URIs are resolved by the worker and are exempt.
        if !control.plugin_path.starts_with("builtin://") {
            let expected = match control.requested_format.as_str() {
                "vst3" | "VST3" => "VST3",
                "au" | "AU" => "AU",
                "clap" | "CLAP" => "CLAP",
                _ => "",
            };
            if !crate::plugin_admission::safe_candidate(
                Path::new(&control.plugin_path),
                expected,
            ) {
                self.failure
                    .store(FAILURE_INVALID_PLUGIN_PATH, Ordering::Release);
                return false;
            }
        }
        if self.is_alive() {
            return false;
        }
        let Some(helper_path) = resolve_helper_path() else {
            self.failure
                .store(FAILURE_MISSING_HELPER, Ordering::Release);
            return false;
        };

        if !self.create_shared_memory(control) {
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        let mut parent_to_child = [-1i32; 2];
        let mut child_to_parent = [-1i32; 2];
        if unsafe { libc::pipe(parent_to_child.as_mut_ptr()) } != 0
            || unsafe { libc::pipe(child_to_parent.as_mut_ptr()) } != 0
        {
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        // Move child-bound sources above the stable target descriptors so a
        // dup2 cannot clobber a later source that the actions still need.
        let spawn_control_fd = duplicate_spawn_source(parent_to_child[0]);
        let spawn_status_fd = duplicate_spawn_source(child_to_parent[1]);
        let spawn_shared_fd = duplicate_spawn_source(control.shared_fd);
        if spawn_control_fd < 0 || spawn_status_fd < 0 || spawn_shared_fd < 0 {
            close_fd(spawn_control_fd);
            close_fd(spawn_status_fd);
            close_fd(spawn_shared_fd);
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        const CONTROL_FD: i32 = 10;
        const STATUS_FD: i32 = 11;
        const SHARED_FD: i32 = 12;
        let mut actions = std::mem::MaybeUninit::<libc::posix_spawn_file_actions_t>::uninit();
        let mut actions_ok =
            unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) } == 0;
        if actions_ok {
            let actions = unsafe { actions.assume_init_mut() };
            actions_ok = unsafe {
                libc::posix_spawn_file_actions_adddup2(actions, spawn_control_fd, CONTROL_FD) == 0
                    && libc::posix_spawn_file_actions_adddup2(actions, spawn_status_fd, STATUS_FD)
                        == 0
                    && libc::posix_spawn_file_actions_adddup2(actions, spawn_shared_fd, SHARED_FD)
                        == 0
            };
        }

        let format = worker_format(&control.plugin_path, &control.requested_format);
        let mut args: Vec<CString> = Vec::with_capacity(22);
        for value in [
            helper_path.clone(),
            "--plugin".to_string(),
            control.plugin_path.clone(),
            "--format".to_string(),
            format,
            "--control-fd".to_string(),
            CONTROL_FD.to_string(),
            "--status-fd".to_string(),
            STATUS_FD.to_string(),
            "--shared-fd".to_string(),
            SHARED_FD.to_string(),
            "--shared-name".to_string(),
            control.shared_name.clone(),
            "--sample-rate".to_string(),
            // Match the previous std::to_string formatting for helpers that
            // log or compare the activation string.
            format!("{:.6}", control.sample_rate),
            "--min-frames".to_string(),
            "1".to_string(),
            "--max-frames".to_string(),
            control.max_block_size.to_string(),
            "--channels".to_string(),
            control.channels.to_string(),
        ] {
            args.push(CString::new(value).unwrap_or_default());
        }
        let mut argv: Vec<*mut c_char> = args
            .iter()
            .map(|value| value.as_ptr() as *mut c_char)
            .collect();
        argv.push(std::ptr::null_mut());
        let (environment_owned, environment) = spawn_environment();

        let mut child: libc::pid_t = -1;
        let helper_c = CString::new(helper_path).unwrap_or_default();
        let spawn_result = if actions_ok {
            unsafe {
                libc::posix_spawn(
                    &mut child,
                    helper_c.as_ptr(),
                    actions.as_ptr(),
                    std::ptr::null(),
                    argv.as_mut_ptr(),
                    environment.as_ptr(),
                )
            }
        } else {
            libc::EINVAL
        };
        drop(environment_owned);
        if actions_ok {
            unsafe {
                libc::posix_spawn_file_actions_destroy(actions.assume_init_mut());
            }
        }
        close_fd(spawn_control_fd);
        close_fd(spawn_status_fd);
        close_fd(spawn_shared_fd);

        if spawn_result != 0 {
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        close_fd(parent_to_child[0]);
        close_fd(child_to_parent[1]);
        control.control_fd = parent_to_child[1];
        control.status_fd = child_to_parent[0];
        let flags = unsafe { libc::fcntl(control.status_fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe { libc::fcntl(control.status_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        }
        control.pid = child;

        let timeout_ms = ready_timeout_ms();
        if !self.wait_for_ready(control, timeout_ms) {
            self.stop_locked(control);
            if self.failure() == FAILURE_NONE {
                self.failure
                    .store(FAILURE_HANDSHAKE_TIMEOUT, Ordering::Release);
            }
            return false;
        }

        self.audio_state_mut().reset_for_new_worker();
        self.process_alive.store(true, Ordering::SeqCst);
        let now = Instant::now();
        control.last_heartbeat = 0;
        control.last_completed_sequence = 0;
        control.last_heartbeat_time = now;
        control.last_progress_time = now;
        true
    }

    #[cfg(unix)]
    fn create_shared_memory(&self, control: &mut Control) -> bool {
        let counter = SHARED_NAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        control.shared_name = format!(
            "/hirari_plugin_{}_{}",
            std::process::id(),
            counter
        );
        let Ok(name) = CString::new(control.shared_name.clone()) else {
            return false;
        };
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            return false;
        }
        control.shared_fd = fd;
        let length = std::mem::size_of::<SharedAudioBlock>();
        if unsafe { libc::ftruncate(fd, length as libc::off_t) } != 0 {
            return false;
        }
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return false;
        }
        let mapped = mapped.cast::<SharedAudioBlock>();
        // Fresh shm is zero-filled; construct the protocol version and the
        // extended-MIDI ring, whose Rust-side layout the worker also uses.
        unsafe {
            (*mapped)
                .protocol_version
                .store(AUDIO_BLOCK_PROTOCOL_VERSION, Ordering::Release);
            let ring = std::ptr::addr_of_mut!((*mapped).extended_midi.bytes).cast::<c_void>();
            hirari_midi_extended_ring_init(ring, RING_STORAGE_BYTES);
        }
        control.mapped = mapped;
        control.mapped_len = length;
        self.shared.store(mapped, Ordering::Release);
        true
    }

    fn stop(&self) {
        let mut control = lock_control(&self.control);
        self.stop_locked(&mut control);
    }

    #[cfg(not(unix))]
    fn stop_locked(&self, _control: &mut Control) {
        self.process_alive.store(false, Ordering::SeqCst);
    }

    #[cfg(unix)]
    fn stop_locked(&self, control: &mut Control) {
        // Publish the stop before touching the mapping. The audio callback
        // observes this flag and leaves without dereferencing shared memory.
        self.process_alive.store(false, Ordering::SeqCst);
        while self.process_calls.load(Ordering::SeqCst) != 0
            || self.shared_users.load(Ordering::SeqCst) != 0
        {
            usleep(100);
        }
        if control.control_fd >= 0 {
            let command = [0xA2u8];
            unsafe {
                libc::write(control.control_fd, command.as_ptr().cast(), 1);
            }
            close_fd(control.control_fd);
            control.control_fd = -1;
        }
        if control.pid <= 0 {
            self.process_alive.store(false, Ordering::SeqCst);
            self.close_ipc_fds(control);
            self.release_shared(control);
            return;
        }
        let child = control.pid;
        unsafe {
            libc::kill(child, libc::SIGTERM);
        }
        let mut reaped = false;
        for _ in 0..100 {
            if let Some(finished) = wait_for_child(child, &mut reaped) {
                if finished {
                    reaped = true;
                    break;
                }
            }
            usleep(5000);
        }
        if !reaped {
            unsafe {
                libc::kill(child, libc::SIGKILL);
            }
            for _ in 0..100 {
                if let Some(finished) = wait_for_child(child, &mut reaped) {
                    if finished {
                        reaped = true;
                        break;
                    }
                }
                usleep(5000);
            }
        }
        if !reaped && self.failure() == FAILURE_NONE {
            self.failure
                .store(FAILURE_PROCESS_EXITED, Ordering::Release);
        }
        control.pid = -1;
        self.process_alive.store(false, Ordering::SeqCst);
        self.close_ipc_fds(control);
        self.release_shared(control);
    }

    #[cfg(unix)]
    fn close_ipc_fds(&self, control: &mut Control) {
        close_fd(control.control_fd);
        control.control_fd = -1;
        close_fd(control.status_fd);
        control.status_fd = -1;
    }

    #[cfg(unix)]
    fn close_pipes(&self, parent_to_child: &mut [i32; 2], child_to_parent: &mut [i32; 2]) {
        close_fd(parent_to_child[0]);
        close_fd(parent_to_child[1]);
        close_fd(child_to_parent[0]);
        close_fd(child_to_parent[1]);
        *parent_to_child = [-1, -1];
        *child_to_parent = [-1, -1];
    }

    #[cfg(unix)]
    fn release_shared(&self, control: &mut Control) {
        if !control.mapped.is_null() {
            self.shared.store(std::ptr::null_mut(), Ordering::Release);
            unsafe {
                libc::munmap(control.mapped.cast(), control.mapped_len);
            }
            control.mapped = std::ptr::null_mut();
        }
        close_fd(control.shared_fd);
        control.shared_fd = -1;
        if !control.shared_name.is_empty() {
            if let Ok(name) = CString::new(control.shared_name.clone()) {
                unsafe {
                    libc::shm_unlink(name.as_ptr());
                }
            }
            control.shared_name.clear();
        }
    }

    #[cfg(unix)]
    fn wait_for_ready(&self, control: &mut Control, timeout_ms: i32) -> bool {
        if control.status_fd < 0 {
            return false;
        }
        let mut descriptor = libc::pollfd {
            fd: control.status_fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result <= 0 {
            return false;
        }
        if descriptor.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            let mut status = 0;
            if control.pid > 0 && unsafe { libc::waitpid(control.pid, &mut status, libc::WNOHANG) }
                == control.pid
            {
                self.failure
                    .store(FAILURE_PROCESS_EXITED, Ordering::Release);
            }
            return false;
        }
        if descriptor.revents & libc::POLLIN == 0 {
            return false;
        }
        let mut response = 0u8;
        let read = unsafe { libc::read(control.status_fd, (&mut response as *mut u8).cast(), 1) };
        if read != 1 {
            return false;
        }
        let failure = match response {
            0xB0 => FAILURE_PLUGIN_LOAD_FAILED,
            0xB1 => FAILURE_PLUGIN_ABI_INVALID,
            0xB2 => FAILURE_PLUGIN_INSTANCE_FAILED,
            0xB3 => FAILURE_SECURITY_SETUP_FAILED,
            0xB4 => FAILURE_PLUGIN_FORMAT_UNSUPPORTED,
            0xAF => FAILURE_PLUGIN_LOAD_FAILED,
            _ => FAILURE_NONE,
        };
        if failure != FAILURE_NONE {
            self.failure.store(failure, Ordering::Release);
        }
        response == 0xA1
    }

    #[cfg(not(unix))]
    fn request_plugin_reset(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn request_plugin_reset(&self) -> bool {
        let control = lock_control(&self.control);
        if !self.is_alive() || control.control_fd < 0 {
            return false;
        }
        let command = [0xA4u8];
        unsafe { libc::write(control.control_fd, command.as_ptr().cast(), 1) == 1 }
    }

    fn poll_health(&self) -> bool {
        let mut control = lock_control(&self.control);
        self.poll_health_locked(&mut control)
    }

    #[cfg(not(unix))]
    fn poll_health_locked(&self, _control: &mut Control) -> bool {
        false
    }

    #[cfg(unix)]
    fn poll_health_locked(&self, control: &mut Control) -> bool {
        let child = control.pid;
        if !self.is_alive() || child <= 0 {
            return false;
        }
        let mut status = 0;
        let result = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if result == 0 {
            let shared = self.shared();
            if !shared.is_null() {
                let shared = unsafe { &*shared };
                let heartbeat = shared.heartbeat.load(Ordering::Acquire);
                if heartbeat != control.last_heartbeat {
                    control.last_heartbeat = heartbeat;
                    control.last_heartbeat_time = Instant::now();
                }
                let requested = shared.request_sequence.load(Ordering::Acquire);
                let completed = shared.completed_sequence.load(Ordering::Acquire);
                if completed != control.last_completed_sequence {
                    control.last_completed_sequence = completed;
                    control.last_progress_time = Instant::now();
                } else if requested != completed
                    && control.last_progress_time.elapsed() > Duration::from_millis(250)
                {
                    self.failure
                        .store(FAILURE_PROCESS_HUNG, Ordering::Release);
                    self.stop_locked(control);
                    return false;
                }
            }
            return true;
        }
        self.failure
            .store(FAILURE_PROCESS_EXITED, Ordering::Release);
        control.pid = -1;
        self.process_alive.store(false, Ordering::SeqCst);
        while self.process_calls.load(Ordering::SeqCst) != 0
            || self.shared_users.load(Ordering::SeqCst) != 0
        {
            usleep(100);
        }
        self.close_ipc_fds(control);
        self.release_shared(control);
        false
    }

    fn restart(&self, force: bool) -> bool {
        let mut control = lock_control(&self.control);
        self.restart_locked(&mut control, force)
    }

    fn restart_locked(&self, control: &mut Control, force: bool) -> bool {
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        if force && self.failure() == FAILURE_PLUGIN_QUARANTINED {
            control.restart_attempts = 0;
            self.failure.store(FAILURE_NONE, Ordering::Release);
        }
        if !force
            && control
                .next_restart_time
                .is_some_and(|next| Instant::now() < next)
        {
            return false;
        }
        if self.is_alive() && !self.poll_health_locked(control) {
            return false;
        }
        let saved = self.get_state_locked(control, &mut state);
        self.stop_locked(control);
        control.restart_attempts = control.restart_attempts.saturating_add(1);
        if !self.start_locked(control) {
            let shift = (control.restart_attempts.saturating_sub(1)).min(8);
            control.next_restart_time =
                Some(Instant::now() + Duration::from_millis(100u64 << shift));
            return false;
        }
        if !saved.is_empty() && !self.set_state_locked(control, &mut state, &saved) {
            self.stop_locked(control);
            self.failure
                .store(FAILURE_PLUGIN_INSTANCE_FAILED, Ordering::Release);
            control.next_restart_time = Some(Instant::now() + Duration::from_secs(30));
            return false;
        }
        control.next_restart_time = None;
        true
    }

    fn auto_restart(&self) -> bool {
        const MAX_AUTOMATIC_RESTARTS: u32 = 5;
        let mut control = lock_control(&self.control);
        if control.restart_attempts >= MAX_AUTOMATIC_RESTARTS {
            self.failure
                .store(FAILURE_PLUGIN_QUARANTINED, Ordering::Release);
            return false;
        }
        self.restart_locked(&mut control, false)
    }

    fn reconfigure(&self, sample_rate: f64, max_block_size: u32) -> bool {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || max_block_size == 0
            || max_block_size as usize > MAX_FRAMES
        {
            self.failure
                .store(FAILURE_PROCESS_FAILED, Ordering::Release);
            return false;
        }
        let saved = self.get_state_locked(&mut control, &mut state);
        let was_alive = self.is_alive();
        let previous_sample_rate = control.sample_rate;
        let previous_block_size = control.max_block_size;
        if was_alive {
            self.stop_locked(&mut control);
        }
        control.sample_rate = sample_rate;
        control.max_block_size = max_block_size;
        self.sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Release);
        if !was_alive {
            return true;
        }
        if !self.start_locked(&mut control) {
            // A rejected format must not strand an otherwise healthy plugin.
            control.sample_rate = previous_sample_rate;
            control.max_block_size = previous_block_size;
            self.sample_rate_bits
                .store(previous_sample_rate.to_bits(), Ordering::Release);
            if self.start_locked(&mut control)
                && !saved.is_empty()
                && !self.set_state_locked(&mut control, &mut state, &saved)
            {
                self.stop_locked(&mut control);
            }
            return false;
        }
        if !saved.is_empty() && !self.set_state_locked(&mut control, &mut state, &saved) {
            self.stop_locked(&mut control);
            self.failure
                .store(FAILURE_PLUGIN_INSTANCE_FAILED, Ordering::Release);
            control.sample_rate = previous_sample_rate;
            control.max_block_size = previous_block_size;
            self.sample_rate_bits
                .store(previous_sample_rate.to_bits(), Ordering::Release);
            if self.start_locked(&mut control)
                && !saved.is_empty()
                && !self.set_state_locked(&mut control, &mut state, &saved)
            {
                self.stop_locked(&mut control);
            }
            return false;
        }
        true
    }

    fn set_generation_context(
        &self,
        project: u64,
        plugin: u64,
        audio: u64,
        state: u64,
    ) -> bool {
        let context = Generation {
            project,
            plugin,
            audio,
            state,
        };
        if !context.valid() {
            return false;
        }
        let _control = lock_control(&self.control);
        let mut state_lock = lock_control(&self.state);
        state_lock.generation = context;
        true
    }

    fn generation_context(&self) -> SandboxGeneration {
        let state = lock_control(&self.state);
        SandboxGeneration {
            project: state.generation.project,
            plugin: state.generation.plugin,
            audio: state.generation.audio,
            state: state.generation.state,
        }
    }

    fn is_quarantined(&self) -> bool {
        self.failure() == FAILURE_PLUGIN_QUARANTINED
    }

    fn restart_attempts(&self) -> u32 {
        lock_control(&self.control).restart_attempts
    }

    fn clear_quarantine(&self) {
        let mut control = lock_control(&self.control);
        if self.is_quarantined() {
            control.restart_attempts = 0;
            self.failure.store(FAILURE_NONE, Ordering::Release);
        }
    }

    fn can_retry(&self) -> bool {
        matches!(
            self.failure(),
            FAILURE_PLUGIN_QUARANTINED
                | FAILURE_PROCESS_EXITED
                | FAILURE_PROCESS_HUNG
                | FAILURE_PROCESS_FAILED
                | FAILURE_PLUGIN_LOAD_FAILED
                | FAILURE_PLUGIN_ABI_INVALID
                | FAILURE_PLUGIN_INSTANCE_FAILED
        )
    }

    fn can_auto_retry(&self) -> bool {
        let failure = self.failure();
        failure != FAILURE_NONE && failure != FAILURE_PLUGIN_QUARANTINED && self.can_retry()
    }

    fn enqueue_extended_midi(
        &self,
        sample_offset: u64,
        articulation_id: u8,
        data: &[u8],
    ) -> bool {
        let Some(access) = self.acquire_shared_access() else {
            return false;
        };
        let shared = access.ptr;
        // SAFETY: Liveness was checked; lifecycle teardown drains process
        // calls before unmapping. The ring is the SPSC transport for the
        // control-thread producer.
        let ring = unsafe { std::ptr::addr_of_mut!((*shared).extended_midi.bytes).cast::<c_void>() };
        let ok = unsafe {
            hirari_midi_extended_ring_push(
                ring,
                sample_offset,
                articulation_id,
                data.as_ptr(),
                data.len(),
            )
        };
        if !ok {
            // SAFETY: same liveness argument as above.
            unsafe {
                (*shared)
                    .input_midi_truncations
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        ok
    }

    fn enqueue_parameter_change(&self, parameter_id: u32, value: f64, sample_offset: u32) -> bool {
        if !value.is_finite() || sample_offset as usize > MAX_FRAMES {
            return false;
        }
        let write = self.parameters.write.load(Ordering::Relaxed);
        let read = self.parameters.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= MAX_PARAMETER_CHANGES as u64 {
            return false;
        }
        let slot = (write % MAX_PARAMETER_CHANGES as u64) as usize;
        // SAFETY: The engine has a single producer for this queue; the index
        // was published after the previous slot write.
        unsafe {
            (*self.parameters.slots.get())[slot] = WireParameterChange {
                parameter_id,
                sample_offset,
                value,
            };
        }
        self.parameters
            .write
            .store(write.wrapping_add(1), Ordering::Release);
        true
    }

    fn reset_staged_midi(&self) {
        let Some(_access) = self.acquire_audio_access() else {
            return;
        };
        self.audio_state_mut().pending_midi_count = 0;
    }

    fn stage_input_midi(
        &self,
        sample_offset: u64,
        articulation_id: u8,
        data: &[u8],
        frames: u32,
    ) -> bool {
        let Some(_audio_access) = self.acquire_audio_access() else {
            return false;
        };
        let Some(access) = self.acquire_shared_access() else {
            return false;
        };
        // SAFETY: The access guard keeps the mapping alive until this event is
        // staged and prevents a state transition from overlapping the write.
        let shared = unsafe { &mut *access.ptr };
        let completed = shared.completed_sequence.load(Ordering::Acquire);
        let audio_state = self.audio_state();
        if audio_state.last_submitted != 0
            && shared.request_sequence.load(Ordering::Acquire) != completed
        {
            return false;
        }
        if sample_offset >= frames as u64 {
            shared
                .input_midi_truncations
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let audio = self.audio_state_mut();
        if data.len() > MAX_MIDI_PAYLOAD_BYTES {
            let ring = std::ptr::addr_of_mut!((*shared).extended_midi.bytes).cast::<c_void>();
            let ok = unsafe {
                hirari_midi_extended_ring_push(
                    ring,
                    sample_offset,
                    articulation_id,
                    data.as_ptr(),
                    data.len(),
                )
            };
            if !ok {
                shared
                    .input_midi_truncations
                    .fetch_add(1, Ordering::Relaxed);
            }
            return ok;
        }
        if audio.pending_midi_count as usize >= MAX_MIDI_EVENTS {
            shared
                .input_midi_truncations
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let event = &mut audio.pending_midi[audio.pending_midi_count as usize];
        event.sample_offset = sample_offset;
        event.size = data.len() as u32;
        event.articulation_id = articulation_id;
        event.data[..data.len()].copy_from_slice(data);
        audio.pending_midi_count += 1;
        true
    }

    fn process(&self, left: &mut [f32], right: &mut [f32], channels: u32, frames: u32) -> u8 {
        const PRODUCED: u8 = 1;
        const CLEAR_MIDI: u8 = 2;
        self.last_process_overrun.store(false, Ordering::Release);
        let Some(_audio_access) = self.acquire_audio_access() else {
            return CLEAR_MIDI;
        };
        let Some(_shared_access) = self.acquire_shared_access() else {
            return CLEAR_MIDI;
        };
        self.process_locked(left, right, channels, frames, PRODUCED, CLEAR_MIDI)
    }

    fn process_locked(
        &self,
        left: &mut [f32],
        right: &mut [f32],
        channels: u32,
        frames: u32,
        produced_flag: u8,
        clear_midi_flag: u8,
    ) -> u8 {
        self.audio_state_mut().output_midi_count = 0;
        if !self.is_alive() || self.state_transition.load(Ordering::Acquire) {
            return clear_midi_flag;
        }
        let shared_ptr = self.shared();
        if shared_ptr.is_null() {
            return clear_midi_flag;
        }
        let frames_usize = frames as usize;
        let channels_usize = channels as usize;
        if channels == 0
            || channels > self.configured_channels.load(Ordering::Acquire)
            || channels_usize > MAX_CHANNELS
            || frames == 0
            || frames_usize > MAX_FRAMES
            || left.len() < frames_usize
            || (channels_usize > 1 && right.len() < frames_usize)
        {
            return clear_midi_flag;
        }
        // SAFETY: Liveness was checked and the mapping outlives this call.
        let shared = unsafe { &mut *shared_ptr };
        let audio = self.audio_state_mut();
        let completed = shared.completed_sequence.load(Ordering::Acquire);
        let request_in_flight = audio.last_submitted != 0
            && shared.request_sequence.load(Ordering::Acquire) != completed;
        if request_in_flight {
            // This call cannot stage a new block. The stale pending audio is
            // resubmitted after collection (matching the one-block pipeline),
            // but MIDI and parameter automation belong to the rejected call
            // and must not be replayed with it.
            audio.pending_midi_count = 0;
            audio.pending_parameter_count = 0;
        } else {
            for channel in 0..channels_usize {
                let source = if channel == 0 { &*left } else { &*right };
                audio.pending_input[channel][..frames_usize]
                    .copy_from_slice(&source[..frames_usize]);
            }
            audio.pending_channels = channels;
            audio.pending_frames = frames;
            let write = self.parameters.write.load(Ordering::Acquire);
            let read = self.parameters.read.load(Ordering::Relaxed);
            let available = write.wrapping_sub(read);
            let count = available.min(MAX_PARAMETER_CHANGES as u64) as u32;
            for index in 0..count as usize {
                let slot = ((read + index as u64) % MAX_PARAMETER_CHANGES as u64) as usize;
                // SAFETY: Single-consumer queue; the producer publishes the
                // write index with Release after filling each slot.
                let change = unsafe { (*self.parameters.slots.get())[slot] };
                shared.parameter_change[index] = change;
            }
            self.parameters
                .read
                .store(read.wrapping_add(count as u64), Ordering::Release);
            audio.pending_parameter_count = count;
        }

        let mut produced = false;
        if audio.last_submitted != 0 && completed == audio.last_submitted {
            for channel in 0..channels_usize {
                let destination = if channel == 0 { &mut *left } else { &mut *right };
                destination[..frames_usize]
                    .copy_from_slice(&shared.output.samples[channel][..frames_usize]);
            }
            let output_count = shared
                .output_midi_events
                .load(Ordering::Acquire)
                .min(MAX_MIDI_EVENTS as u32);
            let mut kept = 0u32;
            for index in 0..output_count as usize {
                let event = &shared.output_midi[index];
                if (event.sample_offset as u32) < frames
                    && event.size > 0
                    && event.size as usize <= MAX_MIDI_PAYLOAD_BYTES
                {
                    audio.output_midi[kept as usize] = *event;
                    kept += 1;
                }
            }
            audio.output_midi_count = kept;
            produced = true;
            audio.completed_sequence_had_overrun = audio.sequence_had_overrun;
            audio.sequence_had_overrun = false;
            audio.last_submitted = 0;
            audio.pending_age_frames = 0;
            audio.overrun_reported_for_sequence = false;
            if shared.process_errors.swap(0, Ordering::AcqRel) > 0 {
                self.failure
                    .store(FAILURE_PROCESS_FAILED, Ordering::Release);
                return 0;
            }
        }

        if shared.request_sequence.load(Ordering::Acquire) != completed {
            // Audio-thread deadline accounting is expressed in frames rather
            // than wall-clock calls; hang detection stays on pollHealth().
            let deadline_frames = (self.sample_rate() * 0.005).max(1.0);
            audio.pending_age_frames = audio.pending_age_frames.saturating_add(frames as u64);
            if !audio.overrun_reported_for_sequence
                && audio.pending_age_frames as f64 >= deadline_frames
            {
                shared.mailbox_overruns.fetch_add(1, Ordering::Relaxed);
                self.last_process_overrun.store(true, Ordering::Release);
                audio.sequence_had_overrun = true;
                audio.overrun_reported_for_sequence = true;
            }
            return if produced { produced_flag } else { 0 };
        }

        let submit_channels = audio.pending_channels;
        let submit_frames = audio.pending_frames;
        if submit_channels == 0
            || submit_channels > MAX_CHANNELS as u32
            || submit_frames == 0
            || submit_frames > MAX_FRAMES as u32
        {
            return if produced { produced_flag } else { 0 };
        }
        shared.channels.store(submit_channels, Ordering::Relaxed);
        shared.frames.store(submit_frames, Ordering::Relaxed);
        for channel in 0..submit_channels as usize {
            shared.input.samples[channel][..submit_frames as usize].copy_from_slice(
                &audio.pending_input[channel][..submit_frames as usize],
            );
        }
        for index in 0..audio.pending_midi_count as usize {
            shared.midi[index] = audio.pending_midi[index];
        }
        shared
            .midi_events
            .store(audio.pending_midi_count, Ordering::Relaxed);
        shared
            .parameter_changes
            .store(audio.pending_parameter_count, Ordering::Release);
        shared.output_midi_events.store(0, Ordering::Relaxed);
        shared.output_midi_dropped.store(0, Ordering::Relaxed);
        audio.sequence = audio.sequence.wrapping_add(1);
        audio.last_submitted = audio.sequence;
        audio.pending_age_frames = 0;
        audio.overrun_reported_for_sequence = false;
        shared
            .request_sequence
            .store(audio.sequence, Ordering::Release);
        if produced {
            produced_flag
        } else {
            0
        }
    }

    fn output_midi_count(&self) -> u32 {
        let Some(_access) = self.acquire_audio_access() else {
            return 0;
        };
        self.audio_state().output_midi_count
    }

    fn copy_output_midi(
        &self,
        index: u32,
        sample_offset: &mut u64,
        articulation_id: &mut u8,
        data: &mut [u8],
    ) -> u32 {
        let Some(_access) = self.acquire_audio_access() else {
            return 0;
        };
        let audio = self.audio_state();
        if index >= audio.output_midi_count {
            return 0;
        }
        let event = &audio.output_midi[index as usize];
        let size = event.size as usize;
        if size == 0 || size > MAX_MIDI_PAYLOAD_BYTES || data.len() < size {
            return 0;
        }
        *sample_offset = event.sample_offset;
        *articulation_id = event.articulation_id;
        data[..size].copy_from_slice(&event.data[..size]);
        event.size
    }

    fn take_last_process_overrun(&self) -> bool {
        self.last_process_overrun.swap(false, Ordering::AcqRel)
    }

    fn take_completed_sequence_overrun(&self) -> bool {
        let Some(_access) = self.acquire_audio_access() else {
            return false;
        };
        let audio = self.audio_state_mut();
        let value = audio.completed_sequence_had_overrun;
        audio.completed_sequence_had_overrun = false;
        value
    }

    fn take_dropped_output_midi(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe {
            (*shared)
                .output_midi_dropped
                .swap(0, Ordering::AcqRel)
        }
    }

    fn take_mailbox_overruns(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe { (*shared).mailbox_overruns.swap(0, Ordering::AcqRel) }
    }

    fn take_input_midi_truncations(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe {
            (*shared)
                .input_midi_truncations
                .swap(0, Ordering::AcqRel)
        }
    }

    fn record_input_midi_rejection(&self) {
        if let Some(access) = self.acquire_shared_access() {
            // SAFETY: Telemetry counter read is valid while the mapping is owned.
            unsafe {
                (*access.ptr)
                    .input_midi_truncations
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn take_process_errors(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe { (*shared).process_errors.swap(0, Ordering::AcqRel) }
    }

    fn heartbeat(&self) -> u64 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry read is valid while the mapping is owned.
        unsafe { (*shared).heartbeat.load(Ordering::Acquire) }
    }

    fn active_sample_rate(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry read is valid while the mapping is owned.
        unsafe { (*shared).active_sample_rate.load(Ordering::Acquire) }
    }

    fn active_channels(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry read is valid while the mapping is owned.
        unsafe { (*shared).active_channels.load(Ordering::Acquire) }
    }

    fn is_midi_instrument(&self) -> bool {
        let Some(access) = self.acquire_shared_access() else { return false };
        let shared = access.ptr;
        // SAFETY: Telemetry read is valid while the mapping is owned.
        unsafe { (*shared).midi_instrument.load(Ordering::Acquire) != 0 }
    }

    fn set_state(&self, data: &[u8]) -> bool {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        self.last_state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Release);
        self.set_state_locked(&mut control, &mut state, data)
    }

    fn get_state(&self) -> Vec<u8> {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        self.last_state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Release);
        self.get_state_locked(&mut control, &mut state)
    }

    fn state_error_code(&self) -> u8 {
        self.last_state_error.load(Ordering::Acquire)
    }

    fn wait_for_audio_calls(&self) -> bool {
        for _ in 0..1000 {
            if self.process_calls.load(Ordering::SeqCst) == 0
                && self.shared_users.load(Ordering::SeqCst) == 0
            {
                return true;
            }
            usleep(100);
        }
        false
    }

    fn shared_ptr(&self) -> Option<*mut SharedAudioBlock> {
        let shared = self.shared();
        if shared.is_null() {
            None
        } else {
            Some(shared)
        }
    }

    fn set_state_locked(
        &self,
        control: &mut Control,
        state: &mut StateControl,
        data: &[u8],
    ) -> bool {
        if !self.is_alive() {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return false;
        }
        if data.len() > MAX_STATE_BYTES {
            self.last_state_error
                .store(STATE_ERROR_OVERSIZE, Ordering::Release);
            return false;
        }
        if !self.wait_for_audio_calls() {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return false;
        }
        // Do not create an exclusive reference to the shared block until all
        // audio calls that could access it have drained. `state_transition`
        // is already set by the caller, so new audio calls will leave before
        // dereferencing the mapping while this control operation proceeds.
        let Some(shared_ptr) = self.shared_ptr() else {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return false;
        };
        // SAFETY: `control` serializes unmapping, and the transition gate plus
        // the completed drain above excludes concurrent host audio access.
        let shared = unsafe { &mut *shared_ptr };
        let completed = shared.state_completed_sequence.load(Ordering::Acquire);
        if shared.state_request_sequence.load(Ordering::Acquire) != completed {
            self.last_state_error
                .store(STATE_ERROR_BUSY, Ordering::Release);
            return false;
        }
        let context = state.generation;
        if !context.valid() {
            self.last_state_error
                .store(STATE_ERROR_VERSION, Ordering::Release);
            return false;
        }
        shared.state[..data.len()].copy_from_slice(data);
        shared
            .state_size
            .store(data.len() as u32, Ordering::Relaxed);
        shared
            .state_version
            .store(STATE_PROTOCOL_VERSION, Ordering::Relaxed);
        shared
            .state_checksum
            .store(state_checksum(data), Ordering::Relaxed);
        shared
            .state_project_generation
            .store(context.project, Ordering::Relaxed);
        shared
            .state_plugin_generation
            .store(context.plugin, Ordering::Relaxed);
        shared
            .state_audio_generation
            .store(context.audio, Ordering::Relaxed);
        shared
            .state_generation
            .store(context.state, Ordering::Relaxed);
        shared.state_mode.store(2, Ordering::Relaxed);
        shared.state_result.store(0, Ordering::Relaxed);
        shared
            .state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Relaxed);
        state.sequence = state.sequence.wrapping_add(1);
        shared
            .state_request_sequence
            .store(state.sequence, Ordering::Release);
        let ok = self.wait_for_state(control, shared, state.sequence, false);
        let generation_ok = ok
            && shared.state_project_generation.load(Ordering::Acquire) == context.project
            && shared.state_plugin_generation.load(Ordering::Acquire) == context.plugin
            && shared.state_audio_generation.load(Ordering::Acquire) == context.audio
            && shared.state_generation.load(Ordering::Acquire) == context.state;
        let checksum_ok = generation_ok
            && shared.state_error.load(Ordering::Acquire) == STATE_ERROR_NONE
            && shared.state_checksum.load(Ordering::Acquire) == state_checksum(data);
        if checksum_ok {
            state.cache.clear();
            state.cache.extend_from_slice(data);
        }
        if !checksum_ok {
            let worker_error = shared.state_error.load(Ordering::Acquire);
            self.last_state_error.store(
                if worker_error == STATE_ERROR_NONE {
                    STATE_ERROR_TIMEOUT
                } else {
                    worker_error
                },
                Ordering::Release,
            );
        }
        checksum_ok
    }

    fn get_state_locked(&self, control: &mut Control, state: &mut StateControl) -> Vec<u8> {
        if !self.is_alive() {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return state.cache.clone();
        }
        if !self.wait_for_audio_calls() {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return state.cache.clone();
        }
        // See `set_state_locked`: `state_transition` prevents new audio calls
        // from touching shared memory, and existing calls must drain before
        // this exclusive reference is formed.
        let Some(shared_ptr) = self.shared_ptr() else {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return state.cache.clone();
        };
        // SAFETY: `control` serializes unmapping, and the transition gate plus
        // the completed drain above excludes concurrent host audio access.
        let shared = unsafe { &mut *shared_ptr };
        let completed = shared.state_completed_sequence.load(Ordering::Acquire);
        if shared.state_request_sequence.load(Ordering::Acquire) != completed {
            self.last_state_error
                .store(STATE_ERROR_BUSY, Ordering::Release);
            return state.cache.clone();
        }
        shared.state_size.store(0, Ordering::Relaxed);
        shared.state_version.store(0, Ordering::Relaxed);
        shared.state_mode.store(1, Ordering::Relaxed);
        shared.state_result.store(0, Ordering::Relaxed);
        shared
            .state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Relaxed);
        state.sequence = state.sequence.wrapping_add(1);
        shared
            .state_request_sequence
            .store(state.sequence, Ordering::Release);
        if !self.wait_for_state(control, shared, state.sequence, true) {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return state.cache.clone();
        }
        let size = shared.state_size.load(Ordering::Acquire) as usize;
        if size > MAX_STATE_BYTES {
            self.last_state_error
                .store(STATE_ERROR_OVERSIZE, Ordering::Release);
            return state.cache.clone();
        }
        let state_vec = shared.state[..size].to_vec();
        let worker_error = shared.state_error.load(Ordering::Acquire);
        let validation_error = validate_state(
            &state_vec,
            shared.state_version.load(Ordering::Acquire),
            shared.state_checksum.load(Ordering::Acquire),
        );
        if validation_error != STATE_ERROR_NONE || worker_error != STATE_ERROR_NONE {
            self.last_state_error.store(
                if worker_error != STATE_ERROR_NONE {
                    worker_error
                } else {
                    validation_error
                },
                Ordering::Release,
            );
            return state.cache.clone();
        }
        state.cache = state_vec.clone();
        state_vec
    }

    fn wait_for_state(
        &self,
        control: &mut Control,
        shared: &SharedAudioBlock,
        sequence: u64,
        allow_empty: bool,
    ) -> bool {
        for _ in 0..5000 {
            if !self.is_alive() {
                return false;
            }
            if sequence_reached(
                shared.state_completed_sequence.load(Ordering::Acquire),
                sequence,
            ) {
                let result = shared.state_result.load(Ordering::Acquire) != 0;
                return result
                    && (allow_empty
                        || shared.state_size.load(Ordering::Acquire) as usize <= MAX_STATE_BYTES);
            }
            usleep(1000);
        }
        // The worker may still be blocked inside a vendor state callback. It
        // must leave the graph, but unmapping here would race the unwinding
        // control call, so only kill; the serialized teardown path reaps.
        self.failure
            .store(FAILURE_PROCESS_HUNG, Ordering::Release);
        self.process_alive.store(false, Ordering::SeqCst);
        #[cfg(unix)]
        if control.pid > 0 {
            unsafe {
                libc::kill(control.pid, libc::SIGKILL);
            }
        }
        false
    }
}

struct TransitionGuard<'a> {
    flag: &'a AtomicBool,
}

struct SharedAccess<'a> {
    host: &'a SandboxHost,
    ptr: *mut SharedAudioBlock,
}

struct AudioAccess<'a> {
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
