//! POSIX control-plane loop for the isolated plugin worker.
//!
//! Plugin formats remain behind the narrow backend callbacks, while Rust owns
//! mailbox sequencing, control commands, heartbeats, validation, and fault
//! injection. The mailbox itself is the single layout in `mailbox`, which the
//! engine-side host maps as the same shared object.

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
#[repr(C)]
struct WorkerRLimit {
    current: u64,
    maximum: u64,
}

#[cfg(unix)]
unsafe extern "C" {
    fn getrlimit(resource: i32, limit: *mut WorkerRLimit) -> i32;
    fn setrlimit(resource: i32, limit: *const WorkerRLimit) -> i32;
    fn close(fd: i32) -> i32;
    fn shm_open(name: *const c_char, flags: i32, mode: u32) -> i32;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

#[cfg(target_os = "linux")]
const RLIMIT_NOFILE: i32 = 7;
#[cfg(target_os = "macos")]
const RLIMIT_NOFILE: i32 = 8;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const RLIMIT_CORE: i32 = 4;
#[cfg(target_os = "linux")]
const RLIMIT_AS: i32 = 9;
#[cfg(target_os = "macos")]
const RLIMIT_AS: i32 = 5;

/// Apply the worker's existing process limits before loading untrusted plugin
/// code. The generous address-space cap remains Linux/macOS-only.
#[no_mangle]
pub extern "C" fn hirari_plugin_worker_apply_limits() -> bool {
    #[cfg(unix)]
    unsafe {
        let no_core = WorkerRLimit {
            current: 0,
            maximum: 0,
        };
        let _ = setrlimit(RLIMIT_CORE, &no_core);
        let mut files = WorkerRLimit {
            current: 0,
            maximum: 0,
        };
        if getrlimit(RLIMIT_NOFILE, &mut files) == 0 && files.current > 64 {
            let limited = WorkerRLimit {
                current: 64,
                maximum: files.maximum,
            };
            let _ = setrlimit(RLIMIT_NOFILE, &limited);
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let mut memory = WorkerRLimit {
                current: 0,
                maximum: 0,
            };
            let cap = 2u64 * 1024 * 1024 * 1024;
            if getrlimit(RLIMIT_AS, &mut memory) == 0 && memory.current > cap {
                let limited = WorkerRLimit {
                    current: cap,
                    maximum: memory.maximum,
                };
                let _ = setrlimit(RLIMIT_AS, &limited);
            }
        }
        true
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Validate and map the shared audio block, falling back to its named shared
/// memory object when the inherited descriptor is stale or undersized.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_map_shared(
    fd: i32,
    name: *const c_char,
    minimum_size: usize,
    error_kind: *mut u32,
) -> *mut c_void {
    #[cfg(unix)]
    unsafe {
        // A shared-memory descriptor reports ESPIPE to lseek, so the mapping
        // itself is the size check: mmap refuses an object smaller than the
        // mailbox. The inherited descriptor stays owned by the launcher, while
        // a descriptor opened here belongs to this operation and must close
        // once mmap has consumed it.
        let mut attempted = false;
        let mut fallback_fd = -1;
        let mut mapped = core::ptr::null_mut();
        if fd >= 0 {
            attempted = true;
            mapped = mmap(core::ptr::null_mut(), minimum_size, 1 | 2, 1, fd, 0);
        }
        if mapped as isize == -1 && !name.is_null() {
            let named_fd = shm_open(name, 2, 0);
            if named_fd >= 0 {
                attempted = true;
                fallback_fd = named_fd;
                mapped = mmap(core::ptr::null_mut(), minimum_size, 1 | 2, 1, named_fd, 0);
            }
        }
        if fallback_fd >= 0 {
            close(fallback_fd);
        }
        if mapped as isize == -1 {
            if !error_kind.is_null() {
                // Distinguish "nothing to map" from "the object is too small"
                // so the worker can report which handshake stage failed.
                *error_kind = if attempted { 2 } else { 1 };
            }
            core::ptr::null_mut()
        } else {
            if !error_kind.is_null() {
                *error_kind = 0;
            }
            mapped
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (fd, name, minimum_size, error_kind);
        core::ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_unmap_shared(
    shared: *mut c_void,
    mapped_size: usize,
) {
    #[cfg(unix)]
    if !shared.is_null() {
        unsafe {
            munmap(shared, mapped_size);
        }
    }
    #[cfg(not(unix))]
    let _ = (shared, mapped_size);
}

/// Publish a plugin state payload after its format adapter has filled the
/// caller-owned shared buffer. Metadata uses the mailbox's release ordering.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_publish_state(
    data: *const u8,
    size: u32,
    size_out: *mut c_void,
    version_out: *mut c_void,
    checksum_out: *mut c_void,
) -> u8 {
    if size as usize > crate::MAX_STATE_BYTES {
        return crate::STATE_ERROR_OVERSIZE;
    }
    if size != 0 && data.is_null()
        || size_out.is_null()
        || version_out.is_null()
        || checksum_out.is_null()
    {
        return crate::STATE_ERROR_CHECKSUM;
    }
    let size_out = unsafe { &*size_out.cast::<AtomicU32>() };
    let version_out = unsafe { &*version_out.cast::<AtomicU32>() };
    let checksum_out = unsafe { &*checksum_out.cast::<AtomicU64>() };
    let checksum = unsafe { crate::hirari_plugin_state_checksum(data, size as usize) };
    size_out.store(size, Ordering::Release);
    version_out.store(crate::STATE_PROTOCOL_VERSION, Ordering::Release);
    checksum_out.store(checksum, Ordering::Release);
    crate::STATE_ERROR_NONE
}

/// Validate the size, version and checksum from the shared mailbox before a
/// format adapter is allowed to deserialize plugin state.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_validate_state(
    data: *const u8,
    size_in: *const c_void,
    version_in: *const c_void,
    checksum_in: *const c_void,
) -> u8 {
    if size_in.is_null() || version_in.is_null() || checksum_in.is_null() {
        return crate::STATE_ERROR_CHECKSUM;
    }
    let size = unsafe { &*size_in.cast::<AtomicU32>() }.load(Ordering::Acquire);
    if size as usize > crate::MAX_STATE_BYTES {
        return crate::STATE_ERROR_OVERSIZE;
    }
    let version = unsafe { &*version_in.cast::<AtomicU32>() }.load(Ordering::Acquire);
    let checksum = unsafe { &*checksum_in.cast::<AtomicU64>() }.load(Ordering::Acquire);
    unsafe { crate::hirari_plugin_state_validate(data, size as usize, version, checksum) }
}

const COMMAND_SHUTDOWN: u8 = 0xA2;
const COMMAND_RESET: u8 = 0xA4;
const MAX_FRAMES: u32 = 8192;
const MAX_CHANNELS: u32 = 2;
const STATE_ERROR_PLUGIN: u8 = 4;
const STATE_ERROR_NONE: u8 = 0;
const STATE_MODE_SAVE: u8 = 1;
const STATE_MODE_LOAD: u8 = 2;

/// Parsed worker launch settings. String pointers borrow the process argv and
/// remain valid for the lifetime of `main`.
#[repr(C)]
pub struct WorkerLaunchOptions {
    pub control_fd: i32,
    pub status_fd: i32,
    pub shared_fd: i32,
    pub plugin_path: *const c_char,
    pub shared_name: *const c_char,
    /// 0=auto, 1=builtin, 2=clap, 3=vst3, 4=au.
    pub requested_format: u32,
    pub sample_rate: f64,
    pub min_frames: u32,
    pub max_frames: u32,
    pub channels: u32,
    pub crash_after_blocks: u64,
    pub delay_milliseconds: u32,
    pub path_is_clap: u8,
    pub path_is_vst3: u8,
    pub path_is_audio_unit: u8,
    pub builtin_passthrough: u8,
    /// Adapter bits resolved from the requested format and the plugin path.
    pub resolved_formats: u32,
}

fn option_value<'a>(value: *const c_char) -> Option<&'a [u8]> {
    if value.is_null() {
        None
    } else {
        Some(unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes())
    }
}

fn parse_worker_i32(value: &[u8]) -> Option<i32> {
    if value.is_empty()
        || !value
            .iter()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() || (index == 0 && *byte == b'-'))
    {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_worker_u32(value: &[u8]) -> Option<u32> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_worker_u64(value: &[u8]) -> Option<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn path_has_extension(path: &[u8], extension: &[u8]) -> bool {
    let Some(dot) = path.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    let suffix = &path[dot..];
    suffix.len() == extension.len()
        && suffix
            .iter()
            .zip(extension)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

/// Parses and validates the worker's command line and opt-in fault settings.
/// Return values match the launcher's historical exit code 2; `error_kind`
/// selects the specific diagnostic to preserve in the native executable.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_parse_args(
    argc: i32,
    argv: *const *const c_char,
    output: *mut WorkerLaunchOptions,
    error_kind: *mut u32,
) -> bool {
    if argv.is_null() || output.is_null() || error_kind.is_null() || argc < 1 {
        return false;
    }
    let mut parsed = WorkerLaunchOptions {
        control_fd: -1,
        status_fd: -1,
        shared_fd: -1,
        plugin_path: std::ptr::null(),
        shared_name: std::ptr::null(),
        requested_format: 0,
        sample_rate: 44_100.0,
        min_frames: 1,
        max_frames: MAX_FRAMES,
        channels: MAX_CHANNELS,
        crash_after_blocks: 0,
        delay_milliseconds: 0,
        path_is_clap: 0,
        path_is_vst3: 0,
        path_is_audio_unit: 0,
        builtin_passthrough: 0,
        resolved_formats: 0,
    };
    let args = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    let mut index = 1;
    while index < args.len() {
        let Some(option) = option_value(args[index]) else {
            unsafe { *error_kind = 2 };
            return false;
        };
        if index + 1 >= args.len() {
            unsafe { *error_kind = 1 };
            return false;
        }
        index += 1;
        let Some(value) = option_value(args[index]) else {
            unsafe { *error_kind = 2 };
            return false;
        };
        let parsed_ok = match option {
            b"--control-fd" => parse_worker_i32(value)
                .map(|v| parsed.control_fd = v)
                .is_some(),
            b"--status-fd" => parse_worker_i32(value)
                .map(|v| parsed.status_fd = v)
                .is_some(),
            b"--shared-fd" => parse_worker_i32(value)
                .map(|v| parsed.shared_fd = v)
                .is_some(),
            b"--shared-name" => {
                parsed.shared_name = args[index];
                true
            }
            b"--plugin" => {
                parsed.plugin_path = args[index];
                true
            }
            b"--format" => {
                parsed.requested_format = match value {
                    b"auto" => 0,
                    b"builtin" => 1,
                    b"clap" => 2,
                    b"vst3" => 3,
                    b"au" => 4,
                    _ => {
                        unsafe { *error_kind = 4 };
                        return false;
                    }
                };
                true
            }
            b"--sample-rate" => std::str::from_utf8(value)
                .ok()
                .and_then(|v| v.trim_start().parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .map(|v| parsed.sample_rate = v)
                .is_some(),
            b"--min-frames" => parse_worker_u32(value)
                .map(|v| parsed.min_frames = v)
                .is_some(),
            b"--max-frames" => parse_worker_u32(value)
                .map(|v| parsed.max_frames = v)
                .is_some(),
            b"--channels" => parse_worker_u32(value)
                .map(|v| parsed.channels = v)
                .is_some(),
            _ => {
                unsafe { *error_kind = 2 };
                return false;
            }
        };
        if !parsed_ok {
            unsafe { *error_kind = 2 };
            return false;
        }
        index += 1;
    }

    let Some(plugin_path) = option_value(parsed.plugin_path) else {
        unsafe { *error_kind = 3 };
        return false;
    };
    let shared_name_valid = option_value(parsed.shared_name).is_some_and(|name| !name.is_empty());
    if parsed.control_fd < 0
        || parsed.status_fd < 0
        || (parsed.shared_fd < 0 && !shared_name_valid)
        || plugin_path.is_empty()
    {
        unsafe { *error_kind = 3 };
        return false;
    }
    if !parsed.sample_rate.is_finite()
        || parsed.sample_rate <= 0.0
        || parsed.sample_rate > 384_000.0
        || parsed.min_frames == 0
        || parsed.min_frames > MAX_FRAMES
        || parsed.max_frames < parsed.min_frames
        || parsed.max_frames > MAX_FRAMES
        || parsed.channels == 0
        || parsed.channels > MAX_CHANNELS
    {
        unsafe { *error_kind = 5 };
        return false;
    }

    parsed.path_is_clap = u8::from(path_has_extension(plugin_path, b".clap"));
    parsed.path_is_vst3 = u8::from(path_has_extension(plugin_path, b".vst3"));
    parsed.path_is_audio_unit = u8::from(path_has_extension(plugin_path, b".component"));
    parsed.builtin_passthrough = u8::from(plugin_path.starts_with(b"builtin://"));
    parsed.resolved_formats = resolve_formats(
        parsed.requested_format,
        parsed.path_is_clap != 0,
        parsed.path_is_vst3 != 0,
        parsed.path_is_audio_unit != 0,
        parsed.builtin_passthrough != 0,
    );

    if std::env::var_os("HIRARI_PLUGIN_TEST_FAULTS").is_some() {
        if let Some(value) = std::env::var_os("HIRARI_PLUGIN_WORKER_CRASH_AFTER_BLOCKS") {
            parsed.crash_after_blocks =
                parse_worker_u64(value.to_string_lossy().as_bytes()).unwrap_or(0);
        }
        if let Some(value) = std::env::var_os("HIRARI_PLUGIN_WORKER_DELAY_MS") {
            parsed.delay_milliseconds =
                parse_worker_u32(value.to_string_lossy().as_bytes()).unwrap_or(0);
        }
    }

    unsafe {
        *error_kind = 0;
        output.write(parsed);
    }
    true
}

/// Diagnostic for the `error_kind` reported by
/// [`hirari_plugin_worker_parse_args`]. The worker logs this text verbatim
/// before exiting with status 2.
#[no_mangle]
pub extern "C" fn hirari_plugin_worker_parse_error_text(error_kind: u32) -> *const c_char {
    let message: &'static core::ffi::CStr = match error_kind {
        1 => c"missing command-line option value",
        3 => c"required worker argument is invalid",
        4 => c"requested plugin format is invalid",
        5 => c"requested audio configuration is invalid",
        _ => c"unknown or invalid command-line option",
    };
    message.as_ptr()
}

#[repr(C)]
struct SharedAudioPrefix {
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
    _padding: [u8; 3],
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
}

const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, request_sequence) == 8);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, heartbeat) == 24);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, frames) == 32);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, process_errors) == 72);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, state_request_sequence) == 80);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, state_checksum) == 136);
const _: () = assert!(core::mem::offset_of!(SharedAudioPrefix, state_error) == 150);
const _: () = assert!(core::mem::size_of::<SharedAudioPrefix>() == 152);

#[repr(C)]
pub struct WorkerBackend {
    pub reset: Option<unsafe extern "C" fn(*mut c_void) -> bool>,
    /// Return `(error << 8) | success`. Backend callbacks write plugin state
    /// bytes and metadata before returning; Rust publishes the acknowledgement.
    pub state: Option<unsafe extern "C" fn(*mut c_void, u8) -> u16>,
    /// Process one validated mailbox block. A false result is a backend error.
    pub process: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32) -> bool>,
}

#[repr(C)]
pub struct WorkerFaultConfig {
    pub crash_after_blocks: u64,
    pub delay_milliseconds: u32,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
    fn read(fd: i32, buffer: *mut c_void, count: usize) -> isize;
    fn _exit(status: i32) -> !;
}

/// Run the worker's mailbox/control loop until shutdown or a broken control
/// channel. The shared mapping remains owned and unmapped by the C++ launcher.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_run(
    control_fd: i32,
    shared: *mut c_void,
    backend_context: *mut c_void,
    backend: *const WorkerBackend,
    faults: *const WorkerFaultConfig,
) -> i32 {
    if control_fd < 0 || shared.is_null() || backend.is_null() || faults.is_null() {
        return 2;
    }
    // SAFETY: The launcher passes a live SharedAudioBlock mapping and static
    // callback table for the duration of this synchronous worker loop.
    let shared = unsafe { &*shared.cast::<SharedAudioPrefix>() };
    let backend = unsafe { &*backend };
    let faults = unsafe { &*faults };
    let mut processed_blocks = 0_u64;

    loop {
        let mut descriptor = PollFd {
            fd: control_fd,
            events: 1 | 8 | 16, // POLLIN | POLLERR | POLLHUP
            revents: 0,
        };
        // A failed poll is treated like an idle tick, matching the legacy loop
        // for transient EINTR while preserving periodic heartbeats.
        let poll_result = unsafe { poll(&mut descriptor, 1, 1) };
        if poll_result > 0 && descriptor.revents & (8 | 16) != 0 {
            break;
        }
        if poll_result > 0 && descriptor.revents & 1 != 0 {
            let mut command = 0_u8;
            let count = unsafe { read(control_fd, (&mut command as *mut u8).cast::<c_void>(), 1) };
            if count != 1 || command == COMMAND_SHUTDOWN {
                break;
            }
            if command == COMMAND_RESET {
                let reset_ok = backend
                    .reset
                    .map(|reset| unsafe { reset(backend_context) })
                    .unwrap_or(true);
                if !reset_ok {
                    shared.process_errors.fetch_add(1, Ordering::Release);
                }
            }
        }

        shared.heartbeat.fetch_add(1, Ordering::Relaxed);
        process_state_request(shared, backend, backend_context);

        let request = shared.request_sequence.load(Ordering::Acquire);
        let completed = shared.completed_sequence.load(Ordering::Relaxed);
        if request == 0 || request == completed {
            continue;
        }
        processed_blocks = processed_blocks.wrapping_add(1);
        if faults.crash_after_blocks != 0 && processed_blocks >= faults.crash_after_blocks {
            // This opt-in remains controlled by the launcher's test-only
            // environment gate, so production plugin settings cannot trigger it.
            unsafe { _exit(90) };
        }
        if faults.delay_milliseconds != 0 {
            thread::sleep(Duration::from_millis(u64::from(faults.delay_milliseconds)));
        }

        let channels = shared.channels.load(Ordering::Relaxed);
        let frames = shared.frames.load(Ordering::Relaxed);
        if channels == 0 || channels > MAX_CHANNELS || frames == 0 || frames > MAX_FRAMES {
            break;
        }
        let process_ok = backend
            .process
            .map(|process| unsafe {
                process(
                    backend_context,
                    shared as *const _ as *mut c_void,
                    channels,
                    frames,
                )
            })
            .unwrap_or(false);
        if !process_ok {
            shared.process_errors.fetch_add(1, Ordering::Release);
        }
        // Timed automation belongs to this request and must not be replayed.
        shared.parameter_changes.store(0, Ordering::Release);
        shared.completed_sequence.store(request, Ordering::Release);
    }
    0
}

fn process_state_request(
    shared: &SharedAudioPrefix,
    backend: &WorkerBackend,
    backend_context: *mut c_void,
) {
    let request = shared.state_request_sequence.load(Ordering::Acquire);
    if request == shared.state_completed_sequence.load(Ordering::Relaxed) {
        return;
    }
    let mode = shared.state_mode.load(Ordering::Relaxed);
    let packed = if mode == STATE_MODE_SAVE || mode == STATE_MODE_LOAD {
        backend
            .state
            .map(|state| unsafe { state(backend_context, mode) })
            .unwrap_or(u16::from(STATE_ERROR_PLUGIN) << 8)
    } else {
        u16::from(STATE_ERROR_PLUGIN) << 8
    };
    let success = packed & 0xff != 0;
    let error = (packed >> 8) as u8;
    if !success && mode == STATE_MODE_SAVE {
        shared.state_size.store(0, Ordering::Release);
        shared.state_version.store(0, Ordering::Release);
        shared.state_checksum.store(0, Ordering::Release);
    }
    shared.state_error.store(
        if success { STATE_ERROR_NONE } else { error },
        Ordering::Release,
    );
    shared
        .state_result
        .store(u8::from(success), Ordering::Release);
    shared
        .state_completed_sequence
        .store(request, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_kinds_report_their_diagnostic() {
        let text = |kind| unsafe {
            core::ffi::CStr::from_ptr(hirari_plugin_worker_parse_error_text(kind))
                .to_str()
                .unwrap()
        };
        assert_eq!(text(1), "missing command-line option value");
        assert_eq!(text(3), "required worker argument is invalid");
        assert_eq!(text(4), "requested plugin format is invalid");
        assert_eq!(text(5), "requested audio configuration is invalid");
        assert_eq!(text(2), "unknown or invalid command-line option");
        assert_eq!(text(0), "unknown or invalid command-line option");
        assert_eq!(text(200), "unknown or invalid command-line option");
    }
}

/// Adapter bits resolved by [`resolve_formats`].
pub const WORKER_FORMAT_CLAP: u32 = 1;
pub const WORKER_FORMAT_VST3: u32 = 2;
pub const WORKER_FORMAT_AUDIO_UNIT: u32 = 4;
/// Returned when the launcher asked for a format the path cannot provide.
pub const WORKER_FORMAT_UNSUPPORTED: u32 = u32::MAX;

/// Resolves which isolated adapter the worker may load.
///
/// `requested_format` follows the launcher's numeric contract: 0 auto,
/// 1 builtin, 2 CLAP, 3 VST3, 4 AU. Auto reports every adapter whose path
/// matched (possibly none), while an explicit request returns only that
/// adapter or [`WORKER_FORMAT_UNSUPPORTED`] so the worker never silently loads
/// a different format than the one the host selected.
fn resolve_formats(
    requested_format: u32,
    path_is_clap: bool,
    path_is_vst3: bool,
    path_is_audio_unit: bool,
    builtin_passthrough: bool,
) -> u32 {
    let clap = if path_is_clap { WORKER_FORMAT_CLAP } else { 0 };
    let vst3 = if path_is_vst3 { WORKER_FORMAT_VST3 } else { 0 };
    let audio_unit = if path_is_audio_unit {
        WORKER_FORMAT_AUDIO_UNIT
    } else {
        0
    };
    match requested_format {
        0 => clap | vst3 | audio_unit,
        1 if builtin_passthrough => 0,
        2 if path_is_clap => WORKER_FORMAT_CLAP,
        3 if path_is_vst3 => WORKER_FORMAT_VST3,
        4 if path_is_audio_unit => WORKER_FORMAT_AUDIO_UNIT,
        1 | 2 | 3 | 4 => WORKER_FORMAT_UNSUPPORTED,
        _ => 0,
    }
}

/// Sandbox error bytes reported for each `hirari_clap_instance_load` result.
/// These mirror `SandboxProtocol::kErrorLoad`/`kErrorAbi`/`kErrorInstance`.
const SANDBOX_ERROR_LOAD: u8 = 0xB0;
const SANDBOX_ERROR_ABI: u8 = 0xB1;
const SANDBOX_ERROR_INSTANCE: u8 = 0xB2;

/// Maps a CLAP instance-load result (0 ok, 1 load, 2 ABI, other instance) to
/// the worker's `(error << 8) | exit status` failure envelope.
#[no_mangle]
pub extern "C" fn hirari_plugin_worker_load_failure(load_result: i32) -> u32 {
    let (error, exit) = match load_result {
        1 => (SANDBOX_ERROR_LOAD, 6),
        2 => (SANDBOX_ERROR_ABI, 8),
        _ => (SANDBOX_ERROR_INSTANCE, 10),
    };
    (u32::from(error) << 8) | exit
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn auto_reports_only_the_matching_adapters() {
        assert_eq!(resolve_formats(0, false, false, false, false), 0);
        assert_eq!(
            resolve_formats(0, true, false, true, false),
            WORKER_FORMAT_CLAP | WORKER_FORMAT_AUDIO_UNIT
        );
    }

    #[test]
    fn explicit_requests_never_fall_back_to_another_format() {
        assert_eq!(
            resolve_formats(2, true, true, true, true),
            WORKER_FORMAT_CLAP
        );
        assert_eq!(
            resolve_formats(3, true, false, true, true),
            WORKER_FORMAT_UNSUPPORTED
        );
        assert_eq!(
            resolve_formats(4, false, false, true, false),
            WORKER_FORMAT_AUDIO_UNIT
        );
    }

    #[test]
    fn builtin_requests_require_the_builtin_scheme() {
        assert_eq!(resolve_formats(1, false, false, false, true), 0);
        assert_eq!(
            resolve_formats(1, true, false, false, false),
            WORKER_FORMAT_UNSUPPORTED
        );
    }

    #[test]
    fn load_failures_carry_error_and_exit_status() {
        assert_eq!(hirari_plugin_worker_load_failure(1), (0xB0 << 8) | 6);
        assert_eq!(hirari_plugin_worker_load_failure(2), (0xB1 << 8) | 8);
        assert_eq!(hirari_plugin_worker_load_failure(-1), (0xB2 << 8) | 10);
    }

    #[test]
    fn parse_args_publishes_the_resolved_formats() {
        let owned: std::vec::Vec<std::ffi::CString> = [
            "worker",
            "--control-fd",
            "3",
            "--status-fd",
            "4",
            "--shared-name",
            "block",
            "--plugin",
            "/plugins/demo.component",
            "--format",
            "auto",
        ]
        .iter()
        .map(|value| std::ffi::CString::new(*value).unwrap())
        .collect();
        let argv: std::vec::Vec<*const c_char> = owned.iter().map(|value| value.as_ptr()).collect();
        let mut options: WorkerLaunchOptions = unsafe { std::mem::zeroed() };
        let mut error_kind = 99;
        let parsed = unsafe {
            hirari_plugin_worker_parse_args(
                argv.len() as i32,
                argv.as_ptr(),
                &mut options,
                &mut error_kind,
            )
        };
        assert!(parsed);
        assert_eq!(error_kind, 0);
        assert_eq!(options.resolved_formats, WORKER_FORMAT_AUDIO_UNIT);
    }
}
