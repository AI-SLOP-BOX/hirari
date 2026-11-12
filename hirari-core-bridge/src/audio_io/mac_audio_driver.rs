//! Rust-owned macOS CoreAudio device host.
//!
//! The C++ engine keeps its callback boundary (`MacAudioDriverHost`), but the
//! device lifecycle, HAL/AudioUnit plumbing, format negotiation, buffer-size
//! policy and the realtime callback fan-out live here.  The testable policy is
//! expressed against [`AudioBackend`] so the state machine can be exercised
//! with a fake backend and no audio hardware.

use crate::audio_input_queue::AudioInputBlockQueue;
use coreaudio::audio_unit::{Element, Scope};
use coreaudio_sys as sys;
use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::{offset_of, size_of};
use std::ptr;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
use std::sync::{Arc, Mutex};

pub const FALLBACK_SAMPLE_RATE: f64 = 44_100.0;
pub const FALLBACK_BUFFER_SIZE: u32 = 512;
pub const MAX_FRAMES_PER_SLICE: u32 = 8_192;
pub const MAX_SAMPLE_RATE: f64 = 384_000.0;
pub const MAX_CHANNELS: usize = 32;

const STATE_UNAVAILABLE: u8 = 0;
const STATE_INITIALIZED: u8 = 1;
const STATE_RUNNING: u8 = 2;
const STATE_START_FAILED: u8 = 3;
const STATE_STOPPED: u8 = 4;

/// Realtime process callback. This is the engine callback boundary that stays
/// in C++; Rust only fans device buffers into it.
pub type ProcessCallback = unsafe extern "C" fn(
    inputs: *const *const f32,
    input_channel_count: u32,
    outputs: *const *mut f32,
    output_channel_count: u32,
    frames: u32,
    context: *mut c_void,
);

/// Diagnostic sink. The C++ host forwards it to `LogBuffer`, preserving the
/// messages the UI already reads.
pub type LogCallback =
    unsafe extern "C" fn(context: *mut c_void, level: u32, message: *const c_char);

pub type InputCaptureCallback = unsafe extern "C" fn(
    context: *mut c_void,
    channels: *const *const f32,
    channel_count: u32,
    frame_count: u32,
    timestamp: *const c_void,
);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InputCaptureSink {
    pub callback: Option<InputCaptureCallback>,
    pub context: *mut c_void,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormatRequest {
    pub sample_rate: f64,
    pub buffer_size: u32,
}

impl FormatRequest {
    /// Reject a bad device generation before touching CoreAudio. The bounds
    /// match the deleted C++ driver exactly: finite, (0, 384000], 1..=8192.
    pub fn validate(self) -> Result<Self, DriverError> {
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || self.sample_rate > MAX_SAMPLE_RATE
        {
            return Err(DriverError::InvalidConfiguration(format!(
                "sample rate {} Hz must be finite and within 1..={} Hz",
                self.sample_rate, MAX_SAMPLE_RATE as u32
            )));
        }
        if self.buffer_size == 0 || self.buffer_size > MAX_FRAMES_PER_SLICE {
            return Err(DriverError::InvalidConfiguration(format!(
                "buffer size {} must be within 1..={} frames",
                self.buffer_size, MAX_FRAMES_PER_SLICE
            )));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DriverError {
    InvalidConfiguration(String),
    OsStatus(i32),
    NoAudioComponent,
    DeviceUnavailable,
    NoInputChannels(u32),
    NoOutputChannels(u32),
}

impl DriverError {
    /// OSStatus surfaced through `last_error_code()`. Configuration-shaped
    /// failures use -1, which is what the deleted driver's `failStart()`
    /// default reported.
    pub fn code(&self) -> i32 {
        match self {
            DriverError::OsStatus(status) => *status,
            _ => -1,
        }
    }

    pub fn message(&self) -> String {
        match self {
            DriverError::InvalidConfiguration(detail) => {
                format!("invalid CoreAudio configuration: {detail}")
            }
            DriverError::OsStatus(status) => format!("CoreAudio returned OSStatus {status}"),
            DriverError::NoAudioComponent => {
                "CoreAudio HAL output component is unavailable".to_owned()
            }
            DriverError::DeviceUnavailable => "requested audio device is unavailable".to_owned(),
            DriverError::NoInputChannels(count) => {
                format!("device exposes no input channels (reported {count})")
            }
            DriverError::NoOutputChannels(count) => {
                format!("device exposes fewer than two output channels (reported {count})")
            }
        }
    }
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelMap {
    pub inputs: u32,
    pub outputs: u32,
}

/// Channel mapping policy. Inputs are optional; the host still refuses a
/// device with no capture endpoint because the engine prepares input monitor
/// and talkback from it. Outputs must cover the stereo engine boundary.
pub fn channel_map(raw_inputs: u32, raw_outputs: u32) -> Result<ChannelMap, DriverError> {
    let inputs = raw_inputs.min(MAX_CHANNELS as u32);
    let outputs = raw_outputs.min(MAX_CHANNELS as u32);
    if inputs == 0 {
        return Err(DriverError::NoInputChannels(raw_inputs));
    }
    if outputs < 2 {
        return Err(DriverError::NoOutputChannels(raw_outputs));
    }
    Ok(ChannelMap { inputs, outputs })
}

/// Composition plan for [`AudioBackend::resolve_for_open`]: the output device
/// cannot capture, so it is folded together with the default input device into
/// one private aggregate device addressed by these two sub-device UIDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DuplexComposition {
    pub input_uid: String,
    pub output_uid: String,
}

/// Pure "compose vs. reuse" decision for the duplex fix. Returns `Some` only
/// when a private aggregate is both necessary (the output device has no input
/// channels) and possible (a distinct default input device with input
/// channels, and a UID for both ends). Everything else reuses `output_device`
/// exactly as the driver did before aggregates existed.
pub(crate) fn plan_duplex_composition(
    output_device: u32,
    output_input_channels: u32,
    input_device: u32,
    input_input_channels: u32,
    input_device_uid: Option<&str>,
    output_device_uid: Option<&str>,
) -> Option<DuplexComposition> {
    if output_device == 0 || output_input_channels > 0 {
        return None;
    }
    if input_device == 0 || input_device == output_device || input_input_channels == 0 {
        return None;
    }
    let input_uid = input_device_uid.unwrap_or_default();
    let output_uid = output_device_uid.unwrap_or_default();
    if input_uid.is_empty() || output_uid.is_empty() {
        return None;
    }
    Some(DuplexComposition {
        input_uid: input_uid.to_owned(),
        output_uid: output_uid.to_owned(),
    })
}

/// Split a device callback into engine-sized chunks. A normal callback
/// (`frames <= max_chunk`) produces exactly one chunk, so the common path is
/// unchanged. Odd tails are preserved: 1023/512 -> 512 + 511.
pub fn split_frames(frames: u32, max_chunk: u32) -> FrameChunks {
    FrameChunks {
        offset: 0,
        remaining: frames,
        max_chunk: if max_chunk == 0 { frames.max(1) } else { max_chunk },
    }
}

pub struct FrameChunks {
    offset: u32,
    remaining: u32,
    max_chunk: u32,
}

impl Iterator for FrameChunks {
    type Item = (u32, u32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let len = self.remaining.min(self.max_chunk);
        let offset = self.offset;
        self.offset = self.offset.saturating_add(len);
        self.remaining -= len;
        Some((offset, len))
    }
}

#[derive(Default)]
pub struct CallbackStats {
    output_peak: AtomicU32,
    callback_count: AtomicU64,
}

impl CallbackStats {
    pub fn output_peak(&self) -> f32 {
        f32::from_bits(self.output_peak.load(Ordering::Acquire))
    }

    pub fn callback_count(&self) -> u64 {
        self.callback_count.load(Ordering::Acquire)
    }

    fn store_peak(&self, peak: f32) {
        self.output_peak.store(peak.to_bits(), Ordering::Release);
    }

    fn record_callback(&self) {
        self.callback_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Shared control/realtime state. Control-plane fields that the IOProc reads
/// are atomics; the queue is the fixed-capacity SPSC queue used by the capture
/// worker. No realtime path allocates or takes the host lifecycle mutex.
pub struct RealtimeState {
    running: AtomicBool,
    stopping: AtomicBool,
    device_lost: AtomicBool,
    buffer_size: AtomicU32,
    input_channels: AtomicU32,
    output_channels: AtomicU32,
    input_buffer: AtomicPtr<f32>,
    input_buffer_frames: AtomicU32,
    unit: AtomicPtr<c_void>,
    callbacks_in_flight: AtomicU32,
    process: AtomicUsize,
    process_context: AtomicUsize,
    input_sink: AtomicPtr<InputCaptureSink>,
    queue: Arc<AudioInputBlockQueue>,
    pub stats: Arc<CallbackStats>,
    /// Measured input-path latency in seconds (f64 bits): device + unit
    /// latency properties plus one callback buffer. Written once per
    /// stream start; read by recording compensation and the UI.
    io_latency_secs: AtomicU64,
}

// The realtime callback only touches atomics, immutable bytes owned by C++, and
// the SPSC queue (which has its own producer/consumer discipline).
unsafe impl Send for RealtimeState {}
unsafe impl Sync for RealtimeState {}

impl RealtimeState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            running: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            device_lost: AtomicBool::new(false),
            buffer_size: AtomicU32::new(FALLBACK_BUFFER_SIZE),
            input_channels: AtomicU32::new(0),
            output_channels: AtomicU32::new(0),
            input_buffer: AtomicPtr::new(ptr::null_mut()),
            input_buffer_frames: AtomicU32::new(0),
            unit: AtomicPtr::new(ptr::null_mut()),
            callbacks_in_flight: AtomicU32::new(0),
            process: AtomicUsize::new(0),
            process_context: AtomicUsize::new(0),
            input_sink: AtomicPtr::new(ptr::null_mut()),
            queue: Arc::new(AudioInputBlockQueue::new()),
            stats: Arc::new(CallbackStats::default()),
            io_latency_secs: AtomicU64::new(0.0f64.to_bits()),
        })
    }

    pub fn io_latency_secs(&self) -> f64 {
        f64::from_bits(self.io_latency_secs.load(Ordering::Relaxed))
    }

    fn set_io_latency_secs(&self, secs: f64) {
        self.io_latency_secs
            .store(secs.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn set_process_callback(&self, callback: Option<ProcessCallback>, context: *mut c_void) {
        self.process.store(
            callback.map_or(0, |callback| callback as usize),
            Ordering::Release,
        );
        self.process_context
            .store(context as usize, Ordering::Release);
    }

    fn process_callback(&self) -> Option<ProcessCallback> {
        let address = self.process.load(Ordering::Acquire);
        if address == 0 {
            None
        } else {
            // SAFETY: only `set_process_callback` publishes this value, and it
            // always publishes a valid `ProcessCallback` function pointer.
            Some(unsafe { std::mem::transmute::<usize, ProcessCallback>(address) })
        }
    }

    fn wait_for_callbacks_to_quiesce(&self) {
        while self.callbacks_in_flight.load(Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
    }

    /// Test/backend hook used when CoreAudio reports that the selected device
    /// is no longer alive.
    fn mark_device_lost(&self) {
        self.device_lost.store(true, Ordering::Release);
        self.running.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OpenFormat {
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub channels: ChannelMap,
    pub device: u32,
}

/// Small HAL surface. Only this trait is exercised by the policy/state tests;
/// the production implementation is [`CoreAudioBackend`].
pub trait AudioBackend: Send {
    fn default_output_device(&self) -> Result<u32, i32>;
    fn nominal_sample_rate(&self, device: u32) -> Option<f64>;
    fn buffer_frame_size(&self, device: u32) -> Option<u32>;
    /// Measured input-path latency in seconds for a stream about to be
    /// started (device + unit properties plus one callback buffer).
    fn io_latency_seconds(
        &self,
        device: u32,
        unit: sys::AudioUnit,
        sample_rate: f64,
        buffer_frames: u32,
    ) -> f64;
    fn channel_counts(&self, device: u32) -> Result<(u32, u32), DriverError>;
    fn open(
        &mut self,
        request: FormatRequest,
        requested_device: u32,
        channels: ChannelMap,
        realtime: Arc<RealtimeState>,
    ) -> Result<u32, DriverError>;
    fn start(&mut self) -> Result<(), DriverError>;
    fn close(&mut self);
    fn is_open(&self) -> bool;
    fn device_uid(&self, device: u32) -> Option<String>;
    fn devices_json(&self) -> String;
    fn take_default_device_change(&self) -> bool;
    /// True when the capture endpoint behind the open device changed. A
    /// physical duplex device needs no action, but a composed aggregate folds
    /// in the default input, so it has to be rebuilt around the new endpoint.
    fn take_input_endpoint_change(&self) -> bool {
        false
    }
    /// The device `open` should receive for this generation.
    ///
    /// The default is the identity so a backend that composes nothing behaves
    /// exactly as before. [`CoreAudioBackend`] overrides it to fold the
    /// default input device and this output device into one private aggregate,
    /// which is what lets an output-only default device capture as well.
    fn resolve_for_open(&self, output_device: u32, request: FormatRequest) -> u32 {
        let _ = (output_device, request);
        output_device
    }
    /// The real capture endpoint behind the device `open` was handed.
    ///
    /// A composed duplex device is a backend implementation detail; callers
    /// that report the input device to the UI must not see its private,
    /// per-process UID. The default is the identity so a backend that opens a
    /// physical device keeps reporting that device.
    fn input_endpoint_for(&self, opened_device: u32) -> u32 {
        opened_device
    }
}

/// Testable lifecycle and format policy. Device selection order is preserved:
/// an explicit selection wins, otherwise the system default output device is
/// opened. A failed switch restores the previous selection and makes one
/// best-effort restart before reporting failure.
#[cfg(target_os = "macos")]
#[path = "mac_audio_host_core.rs"]
mod host_core;
pub(crate) use host_core::*;

fn log_message(
    log: Option<LogCallback>,
    context: *mut c_void,
    level: u32,
    message: &str,
) {
    let Some(log) = log else { return };
    let Ok(message) = CString::new(message) else {
        return;
    };
    // SAFETY: the C++ host owns the context for at least the host lifetime.
    unsafe { log(context, level, message.as_ptr()) };
}

fn status_name(state: u8) -> &'static str {
    match state {
        STATE_INITIALIZED => "initialized",
        STATE_RUNNING => "running",
        STATE_START_FAILED => "start-failed",
        STATE_STOPPED => "stopped",
        _ => "unavailable",
    }
}

fn status_c_string(state: u8) -> *const c_char {
    match status_name(state) {
        "initialized" => b"initialized\0".as_ptr().cast(),
        "running" => b"running\0".as_ptr().cast(),
        "start-failed" => b"start-failed\0".as_ptr().cast(),
        "stopped" => b"stopped\0".as_ptr().cast(),
        _ => b"unavailable\0".as_ptr().cast(),
    }
}

pub struct MacAudioHost {
    core: Mutex<HostCore>,
    state: AtomicU8,
    last_error: AtomicI32,
    realtime: Arc<RealtimeState>,
    log: Option<LogCallback>,
    log_context: *mut c_void,
}

unsafe impl Send for MacAudioHost {}
unsafe impl Sync for MacAudioHost {}

impl MacAudioHost {
    fn new(backend: Box<dyn AudioBackend>, log: Option<LogCallback>, log_context: *mut c_void) -> Self {
        Self {
            core: Mutex::new(HostCore::new(backend)),
            state: AtomicU8::new(STATE_UNAVAILABLE),
            last_error: AtomicI32::new(0),
            realtime: RealtimeState::new(),
            log,
            log_context,
        }
    }

    fn lock_core(&self) -> std::sync::MutexGuard<'_, HostCore> {
        self.core.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn start(&self, request: Option<FormatRequest>) -> bool {
        self.lock_core().start(
            request,
            &self.state,
            &self.last_error,
            &self.realtime,
            self.log,
            self.log_context,
        )
    }

    fn stop(&self) {
        self.lock_core().stop(&self.state, &self.realtime);
    }

    fn is_running(&self) -> bool {
        self.lock_core()
            .is_running(&self.state, &self.realtime)
    }

    fn sample_rate(&self) -> f64 {
        self.lock_core().sample_rate()
    }

    fn buffer_size(&self) -> u32 {
        self.lock_core().buffer_size()
    }

    fn io_latency_secs(&self) -> f64 {
        self.realtime.io_latency_secs()
    }

    fn input_channel_count(&self) -> u32 {
        self.lock_core().input_channel_count()
    }

    fn output_channel_count(&self) -> u32 {
        let core = self.lock_core();
        if core.is_running(&self.state, &self.realtime) {
            core.output_channel_count()
        } else {
            0
        }
    }

    fn last_error_code(&self) -> i32 {
        self.last_error.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn status(&self) -> &'static str {
        status_name(self.state.load(Ordering::Acquire))
    }

    fn reconfigure(&self, request: FormatRequest) -> bool {
        self.lock_core().reconfigure(
            request,
            &self.state,
            &self.last_error,
            &self.realtime,
            self.log,
            self.log_context,
        )
    }

    fn select_device(&self, device: u32, request: FormatRequest) -> bool {
        self.lock_core().select_device(
            device,
            request,
            &self.state,
            &self.last_error,
            &self.realtime,
            self.log,
            self.log_context,
        )
    }

    fn try_reconnect(&self) {
        self.lock_core().try_reconnect(
            &self.state,
            &self.last_error,
            &self.realtime,
            self.log,
            self.log_context,
        );
    }

    fn set_process_callback(&self, callback: Option<ProcessCallback>, context: *mut c_void) {
        self.realtime.set_process_callback(callback, context);
    }

    fn list_devices_json(&self) -> String {
        self.lock_core().backend.devices_json()
    }

    fn selected_device_uid(&self) -> String {
        let core = self.lock_core();
        let device = core.selected_device_for_uid(&self.realtime);
        if device == 0 {
            return String::new();
        }
        core.backend.device_uid(device).unwrap_or_default()
    }

    fn output_peak(&self) -> f32 {
        self.realtime.stats.output_peak()
    }

    fn callback_count(&self) -> u64 {
        self.realtime.stats.callback_count()
    }

    fn poll_input_block(
        &self,
        destination: *const *mut f32,
        destination_channels: u32,
        destination_frames: u32,
        info: *mut BlockInfo,
        dropped: *mut u64,
    ) -> bool {
        if info.is_null() || dropped.is_null() {
            return false;
        }
        let mut channel_count = 0u32;
        let mut frame_count = 0u32;
        let mut dropped_blocks = 0u64;
        let available = unsafe {
            self.realtime.queue.poll(
                destination,
                destination_channels,
                destination_frames,
                &mut channel_count,
                &mut frame_count,
                &mut dropped_blocks,
            )
        };
        unsafe {
            *info = BlockInfo {
                channel_count,
                frame_count,
            };
            *dropped = dropped_blocks;
        }
        available
    }

    fn discard_pending_input_blocks(&self) {
        self.realtime.queue.discard_pending();
    }

    fn dropped_input_blocks(&self) -> u64 {
        self.realtime.queue.dropped_blocks()
    }

        }

impl Drop for MacAudioHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BlockInfo {
    pub channel_count: u32,
    pub frame_count: u32,
}

#[cfg(target_os = "macos")]
#[path = "mac_audio_coreaudio_plane.rs"]
mod mac_audio_coreaudio_plane;
#[cfg(target_os = "macos")]
#[path = "mac_audio_coreaudio_cf.rs"]
mod mac_audio_coreaudio_cf;
#[cfg(target_os = "macos")]
#[path = "mac_audio_coreaudio_backend.rs"]
mod mac_audio_coreaudio_backend;
pub(in crate::mac_audio_driver) use mac_audio_coreaudio_plane::*;
pub(in crate::mac_audio_driver) use mac_audio_coreaudio_cf::*;
pub(crate) use mac_audio_coreaudio_backend::*;

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_create(
    process: Option<ProcessCallback>,
    process_context: *mut c_void,
    log: Option<LogCallback>,
    log_context: *mut c_void,
) -> *mut c_void {
    let host = MacAudioHost::new(Box::new(CoreAudioBackend::new()), log, log_context);
    host.set_process_callback(process, process_context);
    Box::into_raw(Box::new(host)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mac_audio_host_destroy(host: *mut c_void) {
    if !host.is_null() {
        drop(Box::from_raw(host.cast::<MacAudioHost>()));
    }
}

fn host_ref<'a>(host: *mut c_void) -> Option<&'a MacAudioHost> {
    unsafe { host.cast::<MacAudioHost>().as_ref() }
}

/// Read view over the host the engine publishes through
/// `AudioEngine::audio_driver_host()`.
///
/// The UI-facing status, telemetry and device catalog used to reach this state
/// through a `MacAudioDriverHost` method and an `AudioEngine` forwarder, one C++
/// layer per value. These land in the same Rust state directly. A zero handle is
/// exactly the host the deleted C++ null guards described: unavailable and
/// silent, with no channels, callbacks, peaks or catalog.

#[cfg(target_os = "macos")]
#[path = "mac_audio_views.rs"]
mod mac_audio_views;
pub use mac_audio_views::*;
