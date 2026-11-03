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
        })
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
pub struct HostCore {
    backend: Box<dyn AudioBackend>,
    selected_device: u32,
    negotiated: Option<OpenFormat>,
}

impl HostCore {
    fn new(backend: Box<dyn AudioBackend>) -> Self {
        Self {
            backend,
            selected_device: 0,
            negotiated: None,
        }
    }

    fn default_request(&self, log: Option<LogCallback>, log_context: *mut c_void) -> FormatRequest {
        let fallback = FormatRequest {
            sample_rate: FALLBACK_SAMPLE_RATE,
            buffer_size: FALLBACK_BUFFER_SIZE,
        };
        // The format must be negotiated with the device `start` will open. An
        // explicit selection outranks the system default, otherwise a device
        // running at 96 kHz was asked to open at the default device's rate.
        let device = if self.selected_device != 0 {
            self.selected_device
        } else {
            match self.backend.default_output_device() {
                Ok(0) => {
                    log_message(
                        log,
                        log_context,
                        1,
                        &format!(
                            "Audio device query failed (status=0); using {} Hz/{} frames",
                            fallback.sample_rate, fallback.buffer_size
                        ),
                    );
                    return fallback;
                }
                Ok(device) => device,
                Err(status) => {
                    log_message(
                        log,
                        log_context,
                        1,
                        &format!(
                            "Audio device query failed (status={status}); using {} Hz/{} frames",
                            fallback.sample_rate, fallback.buffer_size
                        ),
                    );
                    return fallback;
                }
            }
        };

        let sample_rate = match self.backend.nominal_sample_rate(device) {
            Some(rate) if rate.is_finite() && rate > 0.0 => rate,
            _ => {
                log_message(
                    log,
                    log_context,
                    1,
                    "Audio sample-rate query failed; using 44100 Hz",
                );
                FALLBACK_SAMPLE_RATE
            }
        };
        let buffer_size = match self.backend.buffer_frame_size(device) {
            Some(size) if size > 0 => size,
            _ => {
                log_message(
                    log,
                    log_context,
                    1,
                    "Audio buffer-size query failed; using 512 frames",
                );
                FALLBACK_BUFFER_SIZE
            }
        };
        log_message(
            log,
            log_context,
            0,
            &format!("Audio device settings: {sample_rate} Hz/{buffer_size} frames"),
        );
        FormatRequest {
            sample_rate,
            buffer_size,
        }
    }

    fn start(
        &mut self,
        requested: Option<FormatRequest>,
        state: &AtomicU8,
        last_error: &AtomicI32,
        realtime: &Arc<RealtimeState>,
        log: Option<LogCallback>,
        log_context: *mut c_void,
    ) -> bool {
        if state.load(Ordering::Acquire) == STATE_RUNNING
            && realtime.running.load(Ordering::Acquire)
        {
            state.store(STATE_RUNNING, Ordering::Release);
            return true;
        }

        let request = match requested {
            Some(request) => match request.validate() {
                Ok(request) => request,
                Err(error) => {
                    return self.fail(
                        error,
                        state,
                        last_error,
                        realtime,
                        log,
                        log_context,
                    )
                }
            },
            None => self.default_request(log, log_context),
        };

        if self.backend.is_open() {
            self.backend.close();
            self.negotiated = None;
        }

        let resolved = if self.selected_device != 0 {
            self.selected_device
        } else {
            self.backend.default_output_device().unwrap_or(0)
        };
        if resolved == 0 {
            return self.fail(
                DriverError::DeviceUnavailable,
                state,
                last_error,
                realtime,
                log,
                log_context,
            );
        }

        // The backend may compose a private aggregate (default input device +
        // this output device) so that an output-only device can capture too.
        // The composition replaces `resolved` for the channel probe below;
        // only an actual composition changes what `open` receives, so every
        // path that composes nothing still opens exactly what it did before.
        let base_device = resolved;
        let resolved = self.backend.resolve_for_open(resolved, request);
        let requested_device = if resolved == base_device {
            self.selected_device
        } else {
            log_message(
                log,
                log_context,
                0,
                "Audio device has no capture side; composed a private aggregate with the default input device (microphone capture stays enabled while the engine runs)",
            );
            resolved
        };

        let dimensions = match self.backend.channel_counts(resolved) {
            Ok(dimensions) => dimensions,
            Err(error) => {
                return self.fail(error, state, last_error, realtime, log, log_context)
            }
        };
        let channels = match channel_map(dimensions.0, dimensions.1) {
            Ok(channels) => channels,
            Err(error) => {
                return self.fail(error, state, last_error, realtime, log, log_context)
            }
        };

        let device = match self.backend.open(
            request,
            requested_device,
            channels,
            Arc::clone(realtime),
        ) {
            Ok(device) => device,
            Err(error) => {
                return self.fail(error, state, last_error, realtime, log, log_context)
            }
        };

        if let Err(error) = self.backend.start() {
            return self.fail(error, state, last_error, realtime, log, log_context);
        }

        self.negotiated = Some(OpenFormat {
            sample_rate: request.sample_rate,
            buffer_size: request.buffer_size,
            channels,
            device,
        });
        last_error.store(0, Ordering::Release);
        state.store(STATE_RUNNING, Ordering::Release);
        true
    }

    fn fail(
        &mut self,
        error: DriverError,
        state: &AtomicU8,
        last_error: &AtomicI32,
        realtime: &Arc<RealtimeState>,
        log: Option<LogCallback>,
        log_context: *mut c_void,
    ) -> bool {
        let code = error.code();
        last_error.store(code, Ordering::Release);
        let message = if matches!(error, DriverError::InvalidConfiguration(_)) {
            format!("Audio driver start failed (OSStatus={code}): {error}")
        } else {
            format!("Audio driver start failed (OSStatus={code})")
        };
        log_message(log, log_context, 1, &message);
        realtime.running.store(false, Ordering::Release);
        self.backend.close();
        self.negotiated = None;
        realtime.queue.reset();
        state.store(STATE_START_FAILED, Ordering::Release);
        false
    }

    fn stop(&mut self, state: &AtomicU8, realtime: &Arc<RealtimeState>) {
        if self.backend.is_open() {
            self.backend.close();
            self.negotiated = None;
            realtime.queue.reset();
            state.store(STATE_STOPPED, Ordering::Release);
        } else if state.load(Ordering::Acquire) == STATE_RUNNING {
            state.store(STATE_STOPPED, Ordering::Release);
        }
    }

    fn reconfigure(
        &mut self,
        request: FormatRequest,
        state: &AtomicU8,
        last_error: &AtomicI32,
        realtime: &Arc<RealtimeState>,
        log: Option<LogCallback>,
        log_context: *mut c_void,
    ) -> bool {
        // The public contract rejects non-finite/zero requests before teardown
        // (old host guard); range rejection happens in `start`.
        if !request.sample_rate.is_finite()
            || request.sample_rate <= 0.0
            || request.buffer_size == 0
        {
            return false;
        }
        self.stop(state, realtime);
        self.start(Some(request), state, last_error, realtime, log, log_context)
    }

    fn select_device(
        &mut self,
        device: u32,
        request: FormatRequest,
        state: &AtomicU8,
        last_error: &AtomicI32,
        realtime: &Arc<RealtimeState>,
        log: Option<LogCallback>,
        log_context: *mut c_void,
    ) -> bool {
        if device == 0
            || !request.sample_rate.is_finite()
            || request.sample_rate <= 0.0
            || request.buffer_size == 0
        {
            return false;
        }
        let previous = self.selected_device;
        self.selected_device = device;
        self.stop(state, realtime);
        if self.start(
            Some(request),
            state,
            last_error,
            realtime,
            log,
            log_context,
        ) {
            return true;
        }
        self.selected_device = previous;
        let _ = self.start(
            Some(request),
            state,
            last_error,
            realtime,
            log,
            log_context,
        );
        false
    }

    fn try_reconnect(
        &mut self,
        state: &AtomicU8,
        last_error: &AtomicI32,
        realtime: &Arc<RealtimeState>,
        log: Option<LogCallback>,
        log_context: *mut c_void,
    ) {
        // Both flags have to be consumed even when the first one already
        // requests a restart, otherwise the second change is seen again later
        // and restarts a freshly opened device a second time.
        let output_changed = self.backend.take_default_device_change();
        let input_changed = self.backend.take_input_endpoint_change();
        let running = state.load(Ordering::Acquire) == STATE_RUNNING
            && realtime.running.load(Ordering::Acquire);
        if output_changed || input_changed || !running {
            self.stop(state, realtime);
            self.start(None, state, last_error, realtime, log, log_context);
        }
    }

    fn is_running(&self, state: &AtomicU8, realtime: &RealtimeState) -> bool {
        state.load(Ordering::Acquire) == STATE_RUNNING
            && realtime.running.load(Ordering::Acquire)
            && !realtime.stopping.load(Ordering::Acquire)
    }

    fn sample_rate(&self) -> f64 {
        self.negotiated
            .map(|format| format.sample_rate)
            .unwrap_or(FALLBACK_SAMPLE_RATE)
    }

    fn buffer_size(&self) -> u32 {
        self.negotiated
            .map(|format| format.buffer_size)
            .unwrap_or(FALLBACK_BUFFER_SIZE)
    }

    fn input_channel_count(&self) -> u32 {
        self.negotiated.map(|format| format.channels.inputs).unwrap_or(0)
    }

    fn output_channel_count(&self) -> u32 {
        self.negotiated
            .map(|format| format.channels.outputs)
            .unwrap_or(0)
    }

    fn selected_device_for_uid(&self, realtime: &RealtimeState) -> u32 {
        if self.is_running_state(realtime) {
            self.negotiated
                .map(|format| self.backend.input_endpoint_for(format.device))
                .unwrap_or(0)
        } else {
            self.selected_device
        }
    }

    fn is_running_state(&self, realtime: &RealtimeState) -> bool {
        self.negotiated.is_some() && realtime.running.load(Ordering::Acquire)
    }
}

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

    fn register_input_capture_sink(&self, sink: *const InputCaptureSink) -> bool {
        if sink.is_null() || unsafe { (*sink).callback.is_none() } {
            return false;
        }
        self.realtime
            .input_sink
            .store(sink.cast_mut(), Ordering::Release);
        true
    }

    fn unregister_input_capture_sink(&self) {
        self.realtime
            .input_sink
            .store(ptr::null_mut(), Ordering::Release);
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

#[repr(C)]
struct PlanarBufferList {
    number_buffers: u32,
    buffers: [sys::AudioBuffer; MAX_CHANNELS],
}

fn silence_buffers(data: *mut sys::AudioBufferList, frames: u32) {
    if data.is_null() {
        return;
    }
    unsafe {
        let list = &mut *data;
        for index in 0..list.mNumberBuffers as usize {
            let buffer = &mut *list.mBuffers.as_mut_ptr().add(index);
            if buffer.mData.is_null() || buffer.mNumberChannels == 0 {
                continue;
            }
            ptr::write_bytes(
                buffer.mData.cast::<f32>(),
                0,
                frames as usize * buffer.mNumberChannels as usize,
            );
        }
    }
}

/// Fan a planar device callback into the engine in bounded chunks. Returns
/// whether the process callback ran. Callers already own the in-flight guard.
unsafe fn dispatch_planar_outputs(
    state: &RealtimeState,
    engine_inputs: *const *const f32,
    engine_input_channels: u32,
    base_outputs: &[*mut f32; MAX_CHANNELS],
    output_channels: u32,
    frames: u32,
) -> bool {
    let Some(process) = state.process_callback() else {
        return false;
    };
    let context = state.process_context.load(Ordering::Acquire) as *mut c_void;
    let chunk_limit = state.buffer_size.load(Ordering::Acquire).max(1);
    let mut dispatched = false;

    for (offset, length) in split_frames(frames, chunk_limit) {
        let mut outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        for channel in 0..output_channels as usize {
            outputs[channel] = base_outputs[channel].add(offset as usize);
        }
        let mut inputs: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
        if engine_input_channels > 0 && !engine_inputs.is_null() {
            for channel in 0..engine_input_channels as usize {
                let input = *engine_inputs.add(channel);
                if !input.is_null() {
                    inputs[channel] = input.add(offset as usize);
                }
            }
        }
        process(
            if engine_input_channels > 0 {
                inputs.as_ptr()
            } else {
                ptr::null()
            },
            engine_input_channels,
            outputs.as_ptr(),
            output_channels,
            length,
            context,
        );
        dispatched = true;
    }

    if dispatched {
        let mut peak = 0.0f32;
        for channel in 0..output_channels as usize {
            let output = base_outputs[channel];
            for frame in 0..frames as usize {
                peak = peak.max(output.add(frame).read().abs());
            }
        }
        state.stats.store_peak(peak);
        state.stats.record_callback();
    }
    dispatched
}

unsafe extern "C" fn audio_unit_render_callback(
    ref_con: *mut c_void,
    action_flags: *mut sys::AudioUnitRenderActionFlags,
    time_stamp: *const sys::AudioTimeStamp,
    _bus_number: u32,
    frames: u32,
    data: *mut sys::AudioBufferList,
) -> sys::OSStatus {
    if ref_con.is_null() || data.is_null() {
        return 0;
    }
    let state = &*(ref_con as *const RealtimeState);
    state.callbacks_in_flight.fetch_add(1, Ordering::Acquire);
    struct CallbackGuard<'a>(&'a RealtimeState);
    impl Drop for CallbackGuard<'_> {
        fn drop(&mut self) {
            self.0.callbacks_in_flight.fetch_sub(1, Ordering::Release);
        }
    }
    let _guard = CallbackGuard(state);

    let list = &mut *data;
    if list.mNumberBuffers == 0 {
        return 0;
    }

    let mut engine_inputs: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
    let mut engine_input_channels = 0u32;

    let input_channels = state.input_channels.load(Ordering::Acquire) as usize;
    let input_capacity = state.input_buffer_frames.load(Ordering::Acquire);
    let input_buffer = state.input_buffer.load(Ordering::Acquire);
    let unit = state.unit.load(Ordering::Acquire).cast::<sys::ComponentInstanceRecord>();
    if !state.stopping.load(Ordering::Acquire)
        && state.running.load(Ordering::Acquire)
        && !input_buffer.is_null()
        && !unit.is_null()
        && input_channels > 0
        && frames <= input_capacity
    {
        let mut input_data = PlanarBufferList {
            number_buffers: input_channels as u32,
            buffers: [sys::AudioBuffer::default(); MAX_CHANNELS],
        };
        for channel in 0..input_channels {
            input_data.buffers[channel] = sys::AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: frames * size_of::<f32>() as u32,
                mData: input_buffer
                    .add(channel * input_capacity as usize)
                    .cast::<c_void>(),
            };
        }
        let render_status = sys::AudioUnitRender(
            unit,
            action_flags,
            time_stamp,
            1,
            frames,
            (&mut input_data as *mut PlanarBufferList).cast::<sys::AudioBufferList>(),
        );
        if render_status == 0 {
            let mut channels: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
            for channel in 0..input_channels {
                channels[channel] = input_data.buffers[channel].mData.cast::<f32>();
            }
            engine_inputs = channels;
            engine_input_channels = input_channels as u32;

            let _ = state.queue.push_planar(
                channels.as_ptr(),
                input_channels as u32,
                frames,
            );
            let sink = state.input_sink.load(Ordering::Acquire);
            if !sink.is_null() {
                if let Some(callback) = (*sink).callback {
                    callback(
                        (*sink).context,
                        channels.as_ptr(),
                        input_channels as u32,
                        frames,
                        time_stamp.cast::<c_void>(),
                    );
                }
            }
        }
    }

    if state.stopping.load(Ordering::Acquire)
        || !state.running.load(Ordering::Acquire)
        || state.process_callback().is_none()
    {
        silence_buffers(data, frames);
        return 0;
    }

    if frames > MAX_FRAMES_PER_SLICE {
        silence_buffers(data, frames);
        return 0;
    }

    let output_channels = state.output_channels.load(Ordering::Acquire);
    if output_channels > 0 && list.mNumberBuffers >= output_channels {
        let mut base_outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        let mut planar = true;
        for channel in 0..output_channels as usize {
            let buffer = &*list.mBuffers.as_ptr().add(channel);
            if buffer.mData.is_null() || buffer.mNumberChannels != 1 {
                planar = false;
                break;
            }
            base_outputs[channel] = buffer.mData.cast::<f32>();
        }
        if planar {
            let _ = dispatch_planar_outputs(
                state,
                engine_inputs.as_ptr(),
                engine_input_channels,
                &base_outputs,
                output_channels,
                frames,
            );
        } else {
            silence_buffers(data, frames);
        }
    } else if !list.mBuffers[0].mData.is_null() {
        let buffer = &mut *list.mBuffers.as_mut_ptr();
        ptr::write_bytes(
            buffer.mData.cast::<f32>(),
            0,
            frames as usize * buffer.mNumberChannels.max(1) as usize,
        );
    }

    0
}

fn property_address(
    selector: u32,
    scope: u32,
    element: u32,
) -> sys::AudioObjectPropertyAddress {
    sys::AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

unsafe fn read_property<T: Copy>(
    object: u32,
    selector: u32,
    scope: u32,
    element: u32,
) -> Result<T, i32> {
    let address = property_address(selector, scope, element);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    let status = sys::AudioObjectGetPropertyData(
        object,
        &address,
        0,
        ptr::null(),
        &mut size,
        value.as_mut_ptr().cast::<c_void>(),
    );
    if status != 0 || size < size_of::<T>() as u32 {
        Err(status)
    } else {
        Ok(value.assume_init())
    }
}

unsafe fn read_cfstring(object: u32, selector: u32, scope: u32, element: u32) -> Option<String> {
    let address = property_address(selector, scope, element);
    let mut value: sys::CFStringRef = ptr::null();
    let mut size = size_of::<sys::CFStringRef>() as u32;
    let status = sys::AudioObjectGetPropertyData(
        object,
        &address,
        0,
        ptr::null(),
        &mut size,
        (&mut value as *mut sys::CFStringRef).cast::<c_void>(),
    );
    if status != 0 || value.is_null() {
        return None;
    }
    cfstring_to_string(value)
}

unsafe fn cfstring_to_string(value: sys::CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 512];
    let converted = sys::CFStringGetCString(
        value,
        buffer.as_mut_ptr(),
        buffer.len() as sys::CFIndex,
        sys::kCFStringEncodingUTF8,
    );
    sys::CFRelease(value.cast::<c_void>());
    if converted == 0 {
        return None;
    }
    let text = CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Owned CoreFoundation reference. Every object built for the aggregate
/// description is released through this on scope exit, so no failure path can
/// leak one.
struct CfRef(sys::CFTypeRef);

impl CfRef {
    fn new(value: sys::CFTypeRef) -> Option<Self> {
        if value.is_null() {
            None
        } else {
            Some(Self(value))
        }
    }

    fn as_ptr(&self) -> sys::CFTypeRef {
        self.0
    }
}

impl Drop for CfRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                sys::CFRelease(self.0);
            }
        }
    }
}

unsafe fn cf_string(text: &str) -> Option<CfRef> {
    let c_text = CString::new(text).ok()?;
    CfRef::new(
        sys::CFStringCreateWithCString(ptr::null(), c_text.as_ptr(), sys::kCFStringEncodingUTF8)
            .cast::<c_void>(),
    )
}

unsafe fn cf_key(key: &[u8]) -> Option<CfRef> {
    let c_key = CStr::from_bytes_until_nul(key).ok()?;
    CfRef::new(
        sys::CFStringCreateWithCString(ptr::null(), c_key.as_ptr(), sys::kCFStringEncodingUTF8)
            .cast::<c_void>(),
    )
}

unsafe fn cf_dictionary(
    keys: &mut [sys::CFTypeRef],
    values: &mut [sys::CFTypeRef],
) -> Option<CfRef> {
    debug_assert_eq!(keys.len(), values.len());
    CfRef::new(
        sys::CFDictionaryCreate(
            ptr::null(),
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
            keys.len() as sys::CFIndex,
            &sys::kCFTypeDictionaryKeyCallBacks,
            &sys::kCFTypeDictionaryValueCallBacks,
        )
        .cast::<c_void>(),
    )
}

unsafe fn cf_array(values: &mut [sys::CFTypeRef]) -> Option<CfRef> {
    CfRef::new(
        sys::CFArrayCreate(
            ptr::null(),
            values.as_mut_ptr(),
            values.len() as sys::CFIndex,
            &sys::kCFTypeArrayCallBacks,
        )
        .cast::<c_void>(),
    )
}

unsafe fn cf_i32(value: i32) -> Option<CfRef> {
    // The SDK documents these aggregate flags as CFNumbers; a CFBoolean here
    // is rejected by the aggregate builder.
    CfRef::new(
        sys::CFNumberCreate(
            ptr::null(),
            sys::kCFNumberSInt32Type as sys::CFNumberType,
            (&value as *const i32).cast::<c_void>(),
        )
        .cast::<c_void>(),
    )
}

/// Private aggregate UIDs are per-process and never persist, but a restart
/// inside one process must not collide with an aggregate whose asynchronous
/// teardown has not landed yet, so each creation gets its own suffix.
fn private_aggregate_uid() -> String {
    static CREATION: AtomicU64 = AtomicU64::new(0);
    let sequence = CREATION.fetch_add(1, Ordering::Relaxed);
    format!("hirari-private-io-{}-{sequence}", std::process::id())
}

/// Compose a private aggregate device out of the two sub-devices named by the
/// plan and return its `AudioObjectID`. `AudioHardwareCreateAggregateDevice`
/// copies the description, so releasing every object here after the call is
/// correct; the caller owns the returned device and must destroy it with
/// `AudioHardwareDestroyAggregateDevice`.
unsafe fn create_private_aggregate(composition: &DuplexComposition) -> Option<u32> {
    // Sub-device entries: [{ "uid": <input uid>, "drift": 1 },
    // { "uid": <output uid> }]. The output is the clock master, so the input
    // must compensate for clock drift or capture slowly slips against playback.
    let subdevice_key = cf_key(sys::kAudioSubDeviceUIDKey)?;
    let drift_key = cf_key(sys::kAudioSubDeviceDriftCompensationKey)?;
    let drift = cf_i32(1)?;
    let input_uid = cf_string(&composition.input_uid)?;
    let output_uid = cf_string(&composition.output_uid)?;

    let mut input_keys = [subdevice_key.as_ptr(), drift_key.as_ptr()];
    let mut input_values = [input_uid.as_ptr(), drift.as_ptr()];
    let input_entry = cf_dictionary(&mut input_keys, &mut input_values)?;
    let mut output_keys = [subdevice_key.as_ptr()];
    let mut output_values = [output_uid.as_ptr()];
    let output_entry = cf_dictionary(&mut output_keys, &mut output_values)?;
    let mut entries = [input_entry.as_ptr(), output_entry.as_ptr()];
    let subdevices = cf_array(&mut entries)?;

    // Description: uid/name/private/subdevices, with the output device acting
    // as both master and clock so playback and capture share one timebase.
    let uid = cf_string(&private_aggregate_uid())?;
    let name = cf_string("Hirari I/O")?;
    let private = cf_i32(1)?;
    let uid_key = cf_key(sys::kAudioAggregateDeviceUIDKey)?;
    let name_key = cf_key(sys::kAudioAggregateDeviceNameKey)?;
    let private_key = cf_key(sys::kAudioAggregateDeviceIsPrivateKey)?;
    let subdevices_key = cf_key(sys::kAudioAggregateDeviceSubDeviceListKey)?;
    let master_key = cf_key(sys::kAudioAggregateDeviceMainSubDeviceKey)?;
    let clock_key = cf_key(sys::kAudioAggregateDeviceClockDeviceKey)?;

    let mut keys = [
        uid_key.as_ptr(),
        name_key.as_ptr(),
        private_key.as_ptr(),
        subdevices_key.as_ptr(),
        master_key.as_ptr(),
        clock_key.as_ptr(),
    ];
    let mut values = [
        uid.as_ptr(),
        name.as_ptr(),
        private.as_ptr(),
        subdevices.as_ptr(),
        output_uid.as_ptr(),
        output_uid.as_ptr(),
    ];
    let description = cf_dictionary(&mut keys, &mut values)?;

    let mut aggregate: u32 = 0;
    let status = sys::AudioHardwareCreateAggregateDevice(
        description.as_ptr().cast::<sys::__CFDictionary>(),
        &mut aggregate,
    );
    if status != 0 || aggregate == 0 {
        None
    } else {
        Some(aggregate)
    }
}

/// Best-effort: a sub-device that refuses the requested rate is left alone and
/// the aggregate simply inherits whatever clock the master device settles on.
unsafe fn set_nominal_sample_rate(device: u32, sample_rate: f64) -> Result<(), i32> {
    let address = property_address(
        sys::kAudioDevicePropertyNominalSampleRate,
        sys::kAudioObjectPropertyScopeGlobal,
        sys::kAudioObjectPropertyElementMain,
    );
    let status = sys::AudioObjectSetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        size_of::<f64>() as u32,
        (&sample_rate as *const f64).cast::<c_void>(),
    );
    if status != 0 {
        Err(status)
    } else {
        Ok(())
    }
}

/// Destroy a composed aggregate, if one is parked in the given slot.
fn destroy_aggregate(aggregate: u32) {
    if aggregate != 0 {
        unsafe {
            let _ = sys::AudioHardwareDestroyAggregateDevice(aggregate);
        }
    }
}

fn append_json_string(json: &mut String, value: &str) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                json.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => json.push(other),
        }
    }
    json.push('"');
}

unsafe fn stream_channel_count(device: u32, scope: u32) -> u32 {
    let address = property_address(sys::kAudioDevicePropertyStreamConfiguration, scope, sys::kAudioObjectPropertyElementMain);
    let mut size = 0u32;
    if sys::AudioObjectGetPropertyDataSize(
        device,
        &address,
        0,
        ptr::null(),
        &mut size,
    ) != 0
        || (size as usize) < offset_of!(sys::AudioBufferList, mBuffers)
        || size as usize > 64 * 1024
    {
        return 0;
    }
    let mut storage = vec![0u8; size as usize];
    if sys::AudioObjectGetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        &mut size,
        storage.as_mut_ptr().cast::<c_void>(),
    ) != 0
        || (size as usize) < offset_of!(sys::AudioBufferList, mBuffers)
    {
        return 0;
    }
    let list = &*(storage.as_ptr().cast::<sys::AudioBufferList>());
    let capacity = (size as usize - offset_of!(sys::AudioBufferList, mBuffers))
        / size_of::<sys::AudioBuffer>();
    if list.mNumberBuffers as usize > capacity {
        return 0;
    }
    let mut channels = 0u32;
    for index in 0..list.mNumberBuffers as usize {
        let buffer = &*list.mBuffers.as_ptr().add(index);
        let count = buffer.mNumberChannels;
        if count > 256u32.saturating_sub(channels) {
            return 0;
        }
        channels += count;
    }
    channels
}

unsafe fn channel_names_for_scope(
    device: u32,
    scope: u32,
    channel_count: u32,
    fallback_prefix: &str,
) -> Vec<String> {
    let mut names = Vec::with_capacity(channel_count as usize);
    for channel in 1..=channel_count {
        let name = read_cfstring(
            device,
            sys::kAudioObjectPropertyElementName,
            scope,
            channel,
        );
        names.push(name.unwrap_or_else(|| format!("{fallback_prefix} {channel}")));
    }
    names
}

/// Production backend: AudioUnit (AUHAL) plus CoreAudio HAL property access.
pub struct CoreAudioBackend {
    unit: usize,
    initialized: bool,
    current_device: u32,
    alive_listener_installed: bool,
    realtime: Option<Arc<RealtimeState>>,
    input_buffer: Option<Box<[f32]>>,
    default_device_changed: Arc<AtomicBool>,
    default_device_listener_installed: bool,
    default_input_changed: Arc<AtomicBool>,
    default_input_listener_installed: bool,
    /// Private aggregate composed by [`CoreAudioBackend::resolve_for_open`].
    /// Zero until one is composed; destroyed by `close` on every path.
    aggregate_device: AtomicU32,
    /// The default input device folded into [`Self::aggregate_device`]. This is
    /// the endpoint the UI and recording must see, not the aggregate.
    aggregate_input_device: AtomicU32,
    /// Nominal rates that composition changed, with the value to put back when
    /// the aggregate is torn down. Leaving a device at a project rate after
    /// quit silently changes how every other application records.
    composition_rates: Mutex<Vec<(u32, f64)>>,
}

impl CoreAudioBackend {
    pub fn new() -> Self {
        let default_device_changed = Arc::new(AtomicBool::new(false));
        let default_input_changed = Arc::new(AtomicBool::new(false));
        let mut backend = Self {
            unit: 0,
            initialized: false,
            current_device: 0,
            alive_listener_installed: false,
            realtime: None,
            input_buffer: None,
            default_device_changed,
            default_device_listener_installed: false,
            default_input_changed,
            default_input_listener_installed: false,
            aggregate_device: AtomicU32::new(0),
            aggregate_input_device: AtomicU32::new(0),
            composition_rates: Mutex::new(Vec::new()),
        };
        unsafe {
            let address = property_address(
                sys::kAudioHardwarePropertyDefaultOutputDevice,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            );
            let status = sys::AudioObjectAddPropertyListener(
                sys::kAudioObjectSystemObject,
                &address,
                Some(default_device_listener),
                Arc::as_ptr(&backend.default_device_changed) as *mut c_void,
            );
            backend.default_device_listener_installed = status == 0;

            let input_address = property_address(
                sys::kAudioHardwarePropertyDefaultInputDevice,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            );
            let input_status = sys::AudioObjectAddPropertyListener(
                sys::kAudioObjectSystemObject,
                &input_address,
                Some(default_device_listener),
                Arc::as_ptr(&backend.default_input_changed) as *mut c_void,
            );
            backend.default_input_listener_installed = input_status == 0;
        }
        backend
    }

    /// Drop a composed aggregate and the input endpoint recorded with it, so
    /// no code path can address a destroyed device. Rates changed to align the
    /// aggregate are put back here too, because the aggregate is the only
    /// reason they were changed.
    fn clear_composition(&self) {
        self.aggregate_input_device.store(0, Ordering::Release);
        destroy_aggregate(self.aggregate_device.swap(0, Ordering::AcqRel));
        let restore = {
            let mut saved = self
                .composition_rates
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *saved)
        };
        for (device, rate) in restore {
            unsafe {
                let _ = set_nominal_sample_rate(device, rate);
            }
        }
    }

    /// Align a sub-device with the aggregate clock. Returns the previous
    /// nominal rate when this call changed it, so a failed composition can
    /// put the hardware back; `None` means "nothing to undo".
    fn align_nominal_sample_rate(&self, device: u32, sample_rate: f64) -> Option<f64> {
        if std::env::var_os("HIRARI_NO_RATE_SET").is_some() {
            return None;
        }
        let current = self.nominal_sample_rate(device)?;
        if (current - sample_rate).abs() < 0.5 {
            return None;
        }
        unsafe { set_nominal_sample_rate(device, sample_rate) }
            .ok()
            .map(|()| current)
    }

    /// The system default capture device, or the failing HAL status.
    fn default_input_device(&self) -> Result<u32, i32> {
        unsafe {
            read_property::<u32>(
                sys::kAudioObjectSystemObject,
                sys::kAudioHardwarePropertyDefaultInputDevice,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            )
        }
    }

    unsafe fn clear_realtime(realtime: &RealtimeState) {
        realtime.unit.store(ptr::null_mut(), Ordering::Release);
        realtime.input_buffer.store(ptr::null_mut(), Ordering::Release);
        realtime.input_buffer_frames.store(0, Ordering::Release);
        realtime.running.store(false, Ordering::Release);
    }

    unsafe fn configure_unit(
        &mut self,
        unit: sys::AudioUnit,
        request: FormatRequest,
        requested_device: u32,
        channels: ChannelMap,
        realtime: &Arc<RealtimeState>,
    ) -> Result<u32, DriverError> {
        if requested_device != 0 {
            let status = au_set(
                unit,
                sys::kAudioOutputUnitProperty_CurrentDevice,
                Scope::Global,
                Element::Output,
                &requested_device,
            );
            if status != 0 {
                return Err(DriverError::OsStatus(status));
            }
        }

        // AUHAL starts with both directions disabled. Bus 0 is device output,
        // bus 1 is device input; EnableIO is addressed on the device-side
        // scope/element pair, exactly as the deleted implementation did.
        let enable = 1u32;
        let status = au_set(
            unit,
            sys::kAudioOutputUnitProperty_EnableIO,
            Scope::Output,
            Element::Output,
            &enable,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }
        let status = au_set(
            unit,
            sys::kAudioOutputUnitProperty_EnableIO,
            Scope::Input,
            Element::Input,
            &enable,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }

        let mut device = requested_device;
        if device == 0 {
            let status = au_get(
                unit,
                sys::kAudioOutputUnitProperty_CurrentDevice,
                Scope::Global,
                Element::Output,
                &mut device,
            );
            if status != 0 {
                device = 0;
            }
        }
        if device == 0 {
            return Err(DriverError::DeviceUnavailable);
        }

        let output_format = stream_format(request.sample_rate, channels.outputs);
        let status = au_set(
            unit,
            sys::kAudioUnitProperty_StreamFormat,
            Scope::Input,
            Element::Output,
            &output_format,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }
        let input_format = stream_format(request.sample_rate, channels.inputs);
        let status = au_set(
            unit,
            sys::kAudioUnitProperty_StreamFormat,
            Scope::Output,
            Element::Input,
            &input_format,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }

        let sample_count = channels.inputs as usize * request.buffer_size as usize;
        self.input_buffer = Some(vec![0.0f32; sample_count].into_boxed_slice());
        let input_ptr = self
            .input_buffer
            .as_mut()
            .expect("input buffer just stored")
            .as_mut_ptr();
        realtime.input_buffer.store(input_ptr, Ordering::Release);
        realtime
            .input_buffer_frames
            .store(request.buffer_size, Ordering::Release);
        realtime
            .input_channels
            .store(channels.inputs, Ordering::Release);
        realtime
            .output_channels
            .store(channels.outputs, Ordering::Release);
        realtime
            .buffer_size
            .store(request.buffer_size, Ordering::Release);
        realtime.unit.store(unit.cast::<c_void>(), Ordering::Release);

        let maximum_frames = request.buffer_size;
        let status = au_set(
            unit,
            sys::kAudioUnitProperty_MaximumFramesPerSlice,
            Scope::Global,
            Element::Output,
            &maximum_frames,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }

        let callback = sys::AURenderCallbackStruct {
            inputProc: Some(audio_unit_render_callback),
            inputProcRefCon: Arc::as_ptr(realtime) as *mut c_void,
        };
        let status = au_set(
            unit,
            sys::kAudioUnitProperty_SetRenderCallback,
            Scope::Input,
            Element::Output,
            &callback,
        );
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }

        let status = sys::AudioUnitInitialize(unit);
        if status != 0 {
            return Err(DriverError::OsStatus(status));
        }
        self.initialized = true;

        let address = property_address(
            sys::kAudioDevicePropertyDeviceIsAlive,
            sys::kAudioObjectPropertyScopeGlobal,
            sys::kAudioObjectPropertyElementMain,
        );
        let status = sys::AudioObjectAddPropertyListener(
            device,
            &address,
            Some(device_alive_listener),
            Arc::as_ptr(realtime) as *mut c_void,
        );
        self.alive_listener_installed = status == 0;
        self.current_device = device;
        realtime.device_lost.store(false, Ordering::Release);
        Ok(device)
    }
}

impl Default for CoreAudioBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CoreAudioBackend {
    fn drop(&mut self) {
        self.close();
        unsafe {
            if self.default_device_listener_installed {
                let address = property_address(
                    sys::kAudioHardwarePropertyDefaultOutputDevice,
                    sys::kAudioObjectPropertyScopeGlobal,
                    sys::kAudioObjectPropertyElementMain,
                );
                sys::AudioObjectRemovePropertyListener(
                    sys::kAudioObjectSystemObject,
                    &address,
                    Some(default_device_listener),
                    Arc::as_ptr(&self.default_device_changed) as *mut c_void,
                );
                self.default_device_listener_installed = false;
            }
        }
        self.default_input_changed
            .store(false, Ordering::Release);
        if self.default_input_listener_installed {
            unsafe {
                let address = property_address(
                    sys::kAudioHardwarePropertyDefaultInputDevice,
                    sys::kAudioObjectPropertyScopeGlobal,
                    sys::kAudioObjectPropertyElementMain,
                );
                sys::AudioObjectRemovePropertyListener(
                    sys::kAudioObjectSystemObject,
                    &address,
                    Some(default_device_listener),
                    Arc::as_ptr(&self.default_input_changed) as *mut c_void,
                );
                self.default_input_listener_installed = false;
            }
        }
    }
}

impl AudioBackend for CoreAudioBackend {
    fn default_output_device(&self) -> Result<u32, i32> {
        unsafe {
            let mut device = 0u32;
            let mut size = size_of::<u32>() as u32;
            let address = property_address(
                sys::kAudioHardwarePropertyDefaultOutputDevice,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            );
            let status = sys::AudioObjectGetPropertyData(
                sys::kAudioObjectSystemObject,
                &address,
                0,
                ptr::null(),
                &mut size,
                (&mut device as *mut u32).cast::<c_void>(),
            );
            if status != 0 {
                Err(status)
            } else {
                Ok(device)
            }
        }
    }

    fn nominal_sample_rate(&self, device: u32) -> Option<f64> {
        unsafe {
            read_property::<f64>(
                device,
                sys::kAudioDevicePropertyNominalSampleRate,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            )
            .ok()
        }
    }

    fn buffer_frame_size(&self, device: u32) -> Option<u32> {
        unsafe {
            read_property::<u32>(
                device,
                sys::kAudioDevicePropertyBufferFrameSize,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            )
            .ok()
        }
    }

    fn channel_counts(&self, device: u32) -> Result<(u32, u32), DriverError> {
        unsafe {
            Ok((
                stream_channel_count(device, sys::kAudioObjectPropertyScopeInput),
                stream_channel_count(device, sys::kAudioObjectPropertyScopeOutput),
            ))
        }
    }

    /// AUHAL can only be pointed at a single device, and a MacBook's default
    /// output device has no input side at all, so `channel_map` rejected it
    /// (`NoInputChannels`) and the engine silently fell back to offline.
    /// Composing a private aggregate of the default input device and this
    /// output device gives the one AUHAL a duplex endpoint on a single locked
    /// clock, which is exactly what the render callback (bus 1 input pulled
    /// inside the output callback) already expects.
    ///
    /// Every failure returns `output_device` untouched, so no path is worse
    /// than the behaviour before aggregates existed.
    fn resolve_for_open(&self, output_device: u32, request: FormatRequest) -> u32 {
        // A start that failed after composition can leave an aggregate parked
        // here (the unit never opened, so nothing else tore it down). Drop it
        // before every decision, including the early returns below.
        self.clear_composition();

        let Ok((output_input_channels, _)) = self.channel_counts(output_device) else {
            return output_device;
        };
        if output_input_channels > 0 {
            // Already a duplex device: nothing to compose.
            return output_device;
        }
        let input_device = match self.default_input_device() {
            Ok(device) if device != 0 => device,
            _ => return output_device,
        };
        let input_input_channels = self
            .channel_counts(input_device)
            .map(|(inputs, _)| inputs)
            .unwrap_or(0);
        let Some(composition) = plan_duplex_composition(
            output_device,
            output_input_channels,
            input_device,
            input_input_channels,
            self.device_uid(input_device).as_deref(),
            self.device_uid(output_device).as_deref(),
        ) else {
            return output_device;
        };

        // Align both ends before they are glued to one clock. A device that
        // refuses the rate keeps its own and the aggregate still composes. If
        // composition fails, undo the change so merely attempting to start
        // cannot leave the machine at a different hardware rate.
        let previous_output_rate = self.align_nominal_sample_rate(output_device, request.sample_rate);
        let previous_input_rate = self.align_nominal_sample_rate(input_device, request.sample_rate);

        match unsafe { create_private_aggregate(&composition) } {
            Some(aggregate) => {
                let mut restore = self
                    .composition_rates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                restore.clear();
                if let Some(rate) = previous_output_rate {
                    restore.push((output_device, rate));
                }
                if let Some(rate) = previous_input_rate {
                    restore.push((input_device, rate));
                }
                drop(restore);
                self.aggregate_input_device
                    .store(input_device, Ordering::Release);
                self.aggregate_device.store(aggregate, Ordering::Release);
                aggregate
            }
            None => {
                if let Some(rate) = previous_output_rate {
                    unsafe {
                        let _ = set_nominal_sample_rate(output_device, rate);
                    }
                }
                if let Some(rate) = previous_input_rate {
                    unsafe {
                        let _ = set_nominal_sample_rate(input_device, rate);
                    }
                }
                output_device
            }
        }
    }

    fn input_endpoint_for(&self, opened_device: u32) -> u32 {
        if self.aggregate_device.load(Ordering::Acquire) == opened_device {
            let input = self.aggregate_input_device.load(Ordering::Acquire);
            if input != 0 {
                return input;
            }
        }
        opened_device
    }

    fn open(
        &mut self,
        request: FormatRequest,
        requested_device: u32,
        channels: ChannelMap,
        realtime: Arc<RealtimeState>,
    ) -> Result<u32, DriverError> {
        unsafe {
            let description = sys::AudioComponentDescription {
                componentType: sys::kAudioUnitType_Output,
                componentSubType: sys::kAudioUnitSubType_HALOutput,
                componentManufacturer: sys::kAudioUnitManufacturer_Apple,
                componentFlags: 0,
                componentFlagsMask: 0,
            };
            let component = sys::AudioComponentFindNext(ptr::null_mut(), &description);
            if component.is_null() {
                return Err(DriverError::NoAudioComponent);
            }
            let mut unit: sys::AudioUnit = ptr::null_mut();
            let status = sys::AudioComponentInstanceNew(component, &mut unit);
            if status != 0 {
                return Err(DriverError::OsStatus(status));
            }
            match self.configure_unit(unit, request, requested_device, channels, &realtime) {
                Ok(device) => {
                    self.unit = unit as usize;
                    self.realtime = Some(realtime);
                    Ok(device)
                }
                Err(error) => {
                    Self::clear_realtime(&realtime);
                    self.input_buffer = None;
                    sys::AudioComponentInstanceDispose(unit);
                    Err(error)
                }
            }
        }
    }

    fn start(&mut self) -> Result<(), DriverError> {
        let Some(realtime) = self.realtime.as_ref() else {
            return Err(DriverError::DeviceUnavailable);
        };
        realtime.running.store(true, Ordering::Release);
        realtime.stopping.store(false, Ordering::Release);
        realtime.device_lost.store(false, Ordering::Release);
        unsafe {
            let status = sys::AudioOutputUnitStart(self.unit as sys::AudioUnit);
            if status != 0 {
                realtime.running.store(false, Ordering::Release);
                return Err(DriverError::OsStatus(status));
            }
        }
        Ok(())
    }

    fn close(&mut self) {
        // A composed aggregate can exist without an open unit: a start that
        // failed after composition still lands here through `fail`.
        let Some(realtime) = self.realtime.take() else {
            self.clear_composition();
            return;
        };
        unsafe {
            realtime.stopping.store(true, Ordering::Release);
            let was_running = realtime.running.swap(false, Ordering::Release);
            if self.alive_listener_installed {
                let address = property_address(
                    sys::kAudioDevicePropertyDeviceIsAlive,
                    sys::kAudioObjectPropertyScopeGlobal,
                    sys::kAudioObjectPropertyElementMain,
                );
                sys::AudioObjectRemovePropertyListener(
                    self.current_device,
                    &address,
                    Some(device_alive_listener),
                    Arc::as_ptr(&realtime) as *mut c_void,
                );
                self.alive_listener_installed = false;
            }
            realtime.wait_for_callbacks_to_quiesce();
            let unit = self.unit as sys::AudioUnit;
            if !unit.is_null() {
                if was_running {
                    let _ = sys::AudioOutputUnitStop(unit);
                }
                realtime.wait_for_callbacks_to_quiesce();
                if self.initialized {
                    let _ = sys::AudioUnitUninitialize(unit);
                    self.initialized = false;
                }
                let _ = sys::AudioComponentInstanceDispose(unit);
            }
            self.unit = 0;
            Self::clear_realtime(&realtime);
            self.input_buffer = None;
            self.current_device = 0;
            realtime.stopping.store(false, Ordering::Release);
        }
        // The AUHAL that referenced the composed device is gone by now, so
        // the aggregate can be torn down after it rather than under it.
        self.clear_composition();
    }

    fn is_open(&self) -> bool {
        self.unit != 0
    }

    fn device_uid(&self, device: u32) -> Option<String> {
        unsafe {
            read_cfstring(
                device,
                sys::kAudioDevicePropertyDeviceUID,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            )
        }
    }

    fn devices_json(&self) -> String {
        unsafe {
            let address = property_address(
                sys::kAudioHardwarePropertyDevices,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            );
            let mut size = 0u32;
            if sys::AudioObjectGetPropertyDataSize(
                sys::kAudioObjectSystemObject,
                &address,
                0,
                ptr::null(),
                &mut size,
            ) != 0
                || size == 0
                || size as usize > 1024 * size_of::<sys::AudioDeviceID>()
            {
                return "[]".to_owned();
            }
            let mut devices = vec![0u32; size as usize / size_of::<sys::AudioDeviceID>()];
            if sys::AudioObjectGetPropertyData(
                sys::kAudioObjectSystemObject,
                &address,
                0,
                ptr::null(),
                &mut size,
                devices.as_mut_ptr().cast::<c_void>(),
            ) != 0
            {
                return "[]".to_owned();
            }

            let mut json = String::from("[");
            let mut first = true;
            // The composed duplex aggregate is visible inside this process
            // like any other device, but it is a temporary implementation
            // detail: listing it would offer the user a device that vanishes
            // on stop and cannot be selected again.
            let composed_aggregate = self.aggregate_device.load(Ordering::Acquire);
            for device in devices {
                if device == composed_aggregate {
                    continue;
                }
                let Some(name) = read_cfstring(
                    device,
                    sys::kAudioObjectPropertyName,
                    sys::kAudioObjectPropertyScopeGlobal,
                    sys::kAudioObjectPropertyElementMain,
                ) else {
                    continue;
                };
                if !first {
                    json.push(',');
                }
                first = false;
                let uid = read_cfstring(
                    device,
                    sys::kAudioDevicePropertyDeviceUID,
                    sys::kAudioObjectPropertyScopeGlobal,
                    sys::kAudioObjectPropertyElementMain,
                )
                .unwrap_or_default();
                let input_channels =
                    stream_channel_count(device, sys::kAudioObjectPropertyScopeInput);
                let output_channels =
                    stream_channel_count(device, sys::kAudioObjectPropertyScopeOutput);
                let input_names = channel_names_for_scope(
                    device,
                    sys::kAudioObjectPropertyScopeInput,
                    input_channels,
                    "Input",
                );
                let output_names = channel_names_for_scope(
                    device,
                    sys::kAudioObjectPropertyScopeOutput,
                    output_channels,
                    "Output",
                );
                json.push_str(&format!("{{\"id\":{device},\"uid\":"));
                append_json_string(&mut json, &uid);
                json.push_str(",\"name\":");
                append_json_string(&mut json, &name);
                json.push_str(&format!(
                    ",\"input\":{},\"input_channels\":{},\"input_channel_names\":[",
                    input_channels > 0,
                    input_channels
                ));
                for (index, channel_name) in input_names.iter().enumerate() {
                    if index > 0 {
                        json.push(',');
                    }
                    append_json_string(&mut json, channel_name);
                }
                json.push_str(&format!(
                    "],\"output\":{},\"output_channels\":{},\"output_channel_names\":[",
                    output_channels > 0,
                    output_channels
                ));
                for (index, channel_name) in output_names.iter().enumerate() {
                    if index > 0 {
                        json.push(',');
                    }
                    append_json_string(&mut json, channel_name);
                }
                json.push_str("]}");
            }
            json.push(']');
            json
        }
    }

    fn take_default_device_change(&self) -> bool {
        self.default_device_changed.swap(false, Ordering::AcqRel)
    }

    fn take_input_endpoint_change(&self) -> bool {
        let changed = self.default_input_changed.swap(false, Ordering::AcqRel);
        // A physical duplex device keeps capturing from the same endpoint when
        // the system default changes; only a composed aggregate has to follow
        // the new default input. The flag is consumed either way so a change
        // made before composition is not replayed after it.
        changed && self.aggregate_device.load(Ordering::Acquire) != 0
    }
}

unsafe extern "C" fn default_device_listener(
    _object: sys::AudioObjectID,
    _address_count: u32,
    _addresses: *const sys::AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> sys::OSStatus {
    if !client_data.is_null() {
        let flag = &*(client_data as *const AtomicBool);
        flag.store(true, Ordering::Release);
    }
    0
}

unsafe extern "C" fn device_alive_listener(
    _object: sys::AudioObjectID,
    _address_count: u32,
    _addresses: *const sys::AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> sys::OSStatus {
    if !client_data.is_null() {
        let state = &*(client_data as *const RealtimeState);
        state.mark_device_lost();
    }
    0
}

fn stream_format(sample_rate: f64, channels: u32) -> sys::AudioStreamBasicDescription {
    sys::AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: sys::kAudioFormatLinearPCM,
        mFormatFlags: sys::kAudioFormatFlagIsFloat
            | sys::kAudioFormatFlagIsPacked
            | sys::kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

unsafe fn au_set<T>(
    unit: sys::AudioUnit,
    id: u32,
    scope: Scope,
    element: Element,
    value: &T,
) -> sys::OSStatus {
    sys::AudioUnitSetProperty(
        unit,
        id,
        scope as u32,
        element as u32,
        (value as *const T).cast::<c_void>(),
        size_of::<T>() as u32,
    )
}

unsafe fn au_get<T>(
    unit: sys::AudioUnit,
    id: u32,
    scope: Scope,
    element: Element,
    value: &mut T,
) -> sys::OSStatus {
    let mut size = size_of::<T>() as u32;
    sys::AudioUnitGetProperty(
        unit,
        id,
        scope as u32,
        element as u32,
        (value as *mut T).cast::<c_void>(),
        &mut size,
    )
}

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
pub fn view_status(handle: usize) -> &'static str {
    host_ref(handle as *mut c_void).map_or(status_name(STATE_UNAVAILABLE), |host| {
        status_name(host.state.load(Ordering::Acquire))
    })
}

pub fn view_is_running(handle: usize) -> bool {
    host_ref(handle as *mut c_void).is_some_and(MacAudioHost::is_running)
}

pub fn view_error_code(handle: usize) -> i32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::last_error_code)
}

pub fn view_output_peak(handle: usize) -> f32 {
    host_ref(handle as *mut c_void).map_or(0.0, MacAudioHost::output_peak)
}

pub fn view_callback_count(handle: usize) -> u64 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::callback_count)
}

pub fn view_input_channel_count(handle: usize) -> u32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::input_channel_count)
}

pub fn view_output_channel_count(handle: usize) -> u32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::output_channel_count)
}

pub fn view_dropped_input_blocks(handle: usize) -> u64 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::dropped_input_blocks)
}

/// An absent host answers with the empty catalog the deleted `!m_driver` branch
/// returned, which is not what a live host with no selected device reports.
pub fn view_list_devices_json(handle: usize) -> String {
    host_ref(handle as *mut c_void)
        .map_or_else(|| "[]".to_owned(), MacAudioHost::list_devices_json)
}

pub fn view_selected_device_uid(handle: usize) -> String {
    host_ref(handle as *mut c_void)
        .map_or_else(String::new, MacAudioHost::selected_device_uid)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_start(host: *mut c_void) -> bool {
    host_ref(host).is_some_and(|host| host.start(None))
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_stop(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.stop();
    }
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_is_running(host: *mut c_void) -> bool {
    host_ref(host).is_some_and(MacAudioHost::is_running)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_sample_rate(host: *mut c_void) -> f64 {
    host_ref(host).map_or(FALLBACK_SAMPLE_RATE, MacAudioHost::sample_rate)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_buffer_size(host: *mut c_void) -> u32 {
    host_ref(host).map_or(FALLBACK_BUFFER_SIZE, MacAudioHost::buffer_size)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_input_channels(host: *mut c_void) -> u32 {
    host_ref(host).map_or(0, MacAudioHost::input_channel_count)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_status(host: *mut c_void) -> *const c_char {
    host_ref(host).map_or_else(
        || status_c_string(STATE_UNAVAILABLE),
        |host| status_c_string(host.state.load(Ordering::Acquire)),
    )
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_last_error_code(host: *mut c_void) -> i32 {
    host_ref(host).map_or(0, MacAudioHost::last_error_code)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_try_reconnect(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.try_reconnect();
    }
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_reconfigure(
    host: *mut c_void,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.reconfigure(FormatRequest {
            sample_rate,
            buffer_size,
        })
    })
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_select_device(
    host: *mut c_void,
    device: u32,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.select_device(
            device,
            FormatRequest {
                sample_rate,
                buffer_size,
            },
        )
    })
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_set_process_callback(
    host: *mut c_void,
    process: Option<ProcessCallback>,
    context: *mut c_void,
) {
    if let Some(host) = host_ref(host) {
        host.set_process_callback(process, context);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mac_audio_host_poll_input_block(
    host: *mut c_void,
    destination: *const *mut f32,
    destination_channels: u32,
    destination_frames: u32,
    info: *mut BlockInfo,
    dropped: *mut u64,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.poll_input_block(
            destination,
            destination_channels,
            destination_frames,
            info,
            dropped,
        )
    })
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_discard_pending_input(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.discard_pending_input_blocks();
    }
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_register_input_sink(
    host: *mut c_void,
    sink: *const InputCaptureSink,
) -> bool {
    host_ref(host).is_some_and(|host| host.register_input_capture_sink(sink))
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_unregister_input_sink(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.unregister_input_capture_sink();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// Real-device contract for the CoreAudio lifecycle, ported from the
    /// `tests/macos_audio_device_contract.cpp` binary that used to drive it
    /// through the C++ adapter.
    ///
    /// `#[ignore]`d because CI hosts have no usable default output device;
    /// `scripts/run_macos_audio_device_contract.sh` runs it explicitly. Every
    /// rejection names itself: a silent failure is indistinguishable from "no
    /// device on this host".
    #[test]
    #[ignore]
    fn real_device_starts_renegotiates_and_stops() {
        static CALLBACKS: AtomicU64 = AtomicU64::new(0);
        static FRAMES: AtomicU64 = AtomicU64::new(0);

        unsafe extern "C" fn render(
            _inputs: *const *const f32,
            _input_channels: u32,
            outputs: *const *mut f32,
            output_channels: u32,
            frames: u32,
            _context: *mut c_void,
        ) {
            CALLBACKS.fetch_add(1, Ordering::Release);
            FRAMES.fetch_add(u64::from(frames), Ordering::Release);
            if outputs.is_null() {
                return;
            }
            for channel in 0..output_channels {
                let destination = *outputs.add(channel as usize);
                if destination.is_null() {
                    continue;
                }
                std::slice::from_raw_parts_mut(destination, frames as usize).fill(0.0);
            }
        }

        fn rejection(host: &MacAudioHost, reason: &str) -> String {
            format!(
                "macOS CoreAudio contract: {reason} (status={}, error={})",
                host.status(),
                host.last_error_code()
            )
        }

        // A rejection stops the device before naming itself, so the reported
        // status describes the settled driver and the host never leaves a live
        // AudioUnit behind for the next process.
        fn require(host: &MacAudioHost, accepted: bool, reason: &str) {
            if accepted {
                return;
            }
            host.stop();
            panic!("{}", rejection(host, reason));
        }

        let host = MacAudioHost::new(Box::new(CoreAudioBackend::new()), None, ptr::null_mut());
        host.set_process_callback(Some(render), ptr::null_mut());

        // First generation: negotiate 48k/128 against the real device.
        require(
            &host,
            host.reconfigure(FormatRequest {
                sample_rate: 48_000.0,
                buffer_size: 128,
            }),
            "cannot open the default output device at 48000/128",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while CALLBACKS.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        require(
            &host,
            host.is_running()
                && CALLBACKS.load(Ordering::Acquire) > 0
                && FRAMES.load(Ordering::Acquire) > 0,
            "the realtime callback never ran",
        );
        require(
            &host,
            host.sample_rate() == 48_000.0
                && host.buffer_size() == 128
                && host.output_channel_count() > 0,
            "the negotiated generation does not match the request",
        );

        // Second generation: renegotiate while the driver is live.
        require(
            &host,
            host.reconfigure(FormatRequest {
                sample_rate: 96_000.0,
                buffer_size: 512,
            }),
            "cannot renegotiate the default output device to 96000/512",
        );
        require(
            &host,
            host.sample_rate() == 96_000.0 && host.buffer_size() == 512,
            "the second generation does not match the request",
        );
        require(&host, host.is_running(), "the driver stopped after renegotiation");

        host.stop();
        require(&host, !host.is_running(), "stop left the driver running");
    }

    #[derive(Default)]
    struct FakeStats {
        opens: AtomicU32,
        closes: AtomicU32,
        starts: AtomicU32,
        default_changed: AtomicBool,
        input_changed: AtomicBool,
    }

    struct FakeBackend {
        stats: Arc<FakeStats>,
        default_device: Result<u32, i32>,
        nominal: Option<f64>,
        selected_nominal: Option<f64>,
        buffer: Option<u32>,
        channels: (u32, u32),
        opened: bool,
        realtime: Option<Arc<RealtimeState>>,
        fail_open: Option<DriverError>,
    }

    impl FakeBackend {
        fn healthy() -> (Self, Arc<FakeStats>, Arc<Mutex<Vec<String>>>) {
            let stats = Arc::new(FakeStats::default());
            let logs = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    stats: Arc::clone(&stats),
                    default_device: Ok(7),
                    nominal: Some(48_000.0),
                    selected_nominal: Some(96_000.0),
                    buffer: Some(256),
                    channels: (2, 2),
                    opened: false,
                    realtime: None,
                    fail_open: None,
                },
                stats,
                logs,
            )
        }
    }

    impl AudioBackend for FakeBackend {
        fn default_output_device(&self) -> Result<u32, i32> {
            self.default_device
        }

        fn nominal_sample_rate(&self, device: u32) -> Option<f64> {
            if self.default_device == Ok(device) {
                self.nominal
            } else {
                self.selected_nominal.or(self.nominal)
            }
        }

        fn buffer_frame_size(&self, _device: u32) -> Option<u32> {
            self.buffer
        }

        fn channel_counts(&self, _device: u32) -> Result<(u32, u32), DriverError> {
            Ok(self.channels)
        }

        fn open(
            &mut self,
            request: FormatRequest,
            _requested_device: u32,
            channels: ChannelMap,
            realtime: Arc<RealtimeState>,
        ) -> Result<u32, DriverError> {
            if let Some(error) = &self.fail_open {
                return Err(error.clone());
            }
            self.stats.opens.fetch_add(1, Ordering::Relaxed);
            self.opened = true;
            self.realtime = Some(Arc::clone(&realtime));
            realtime
                .buffer_size
                .store(request.buffer_size, Ordering::Release);
            realtime
                .output_channels
                .store(channels.outputs, Ordering::Release);
            realtime
                .input_channels
                .store(channels.inputs, Ordering::Release);
            Ok(7)
        }

        fn start(&mut self) -> Result<(), DriverError> {
            self.stats.starts.fetch_add(1, Ordering::Relaxed);
            if let Some(realtime) = &self.realtime {
                realtime.running.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn close(&mut self) {
            if self.opened {
                self.stats.closes.fetch_add(1, Ordering::Relaxed);
            }
            self.opened = false;
            if let Some(realtime) = self.realtime.take() {
                realtime.running.store(false, Ordering::Release);
            }
        }

        fn is_open(&self) -> bool {
            self.opened
        }

        fn device_uid(&self, _device: u32) -> Option<String> {
            Some("fake-uid".to_owned())
        }

        fn devices_json(&self) -> String {
            "[{\"id\":7,\"uid\":\"fake-uid\",\"name\":\"Fake Device\",\"input\":true,\"input_channels\":2,\"input_channel_names\":[\"Input 1\",\"Input 2\"],\"output\":true,\"output_channels\":2,\"output_channel_names\":[\"Output 1\",\"Output 2\"]}]".to_owned()
        }

        fn take_default_device_change(&self) -> bool {
            self.stats.default_changed.swap(false, Ordering::AcqRel)
        }

        fn take_input_endpoint_change(&self) -> bool {
            self.stats.input_changed.swap(false, Ordering::AcqRel)
        }
    }

    unsafe extern "C" fn test_log(
        context: *mut c_void,
        _level: u32,
        message: *const c_char,
    ) {
        if context.is_null() || message.is_null() {
            return;
        }
        let logs = &*(context as *const Mutex<Vec<String>>);
        let message = CStr::from_ptr(message).to_string_lossy().into_owned();
        logs.lock().unwrap().push(message);
    }

    fn fake_host() -> (MacAudioHost, Arc<FakeStats>, Arc<Mutex<Vec<String>>>) {
        let (backend, stats, logs) = FakeBackend::healthy();
        let host = MacAudioHost::new(
            Box::new(backend),
            Some(test_log),
            Arc::as_ptr(&logs) as *mut c_void,
        );
        (host, stats, logs)
    }

    #[test]
    fn start_and_stop_are_idempotent() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        assert!(host.start(None));
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 1);
        assert_eq!(stats.starts.load(Ordering::Relaxed), 1);

        host.stop();
        assert!(!host.is_running());
        assert_eq!(host.status(), "stopped");
        host.stop();
        assert_eq!(host.status(), "stopped");
        assert_eq!(stats.closes.load(Ordering::Relaxed), 1);

        assert!(host.start(None));
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn device_removal_during_run_recovers_through_try_reconnect() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        assert!(host.is_running());

        // The CoreAudio alive listener clears `running` when the selected
        // device disappears; the control plane notices and restarts.
        host.realtime.mark_device_lost();
        assert!(!host.is_running());
        host.try_reconnect();
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert_eq!(host.status(), "running");
    }

    #[test]
    fn default_device_change_forces_a_restart() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.default_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn composed_input_endpoint_change_forces_a_restart() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.input_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn both_device_change_flags_are_consumed_by_one_reconnect() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.default_changed.store(true, Ordering::Release);
        stats.input_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        // The input flag must not survive to restart the new generation.
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn invalid_format_is_rejected_with_a_clear_error() {
        let (host, stats, logs) = fake_host();
        assert!(!host.start(Some(FormatRequest {
            sample_rate: f64::NAN,
            buffer_size: 256,
        })));
        assert_eq!(host.status(), "start-failed");
        assert_eq!(host.last_error_code(), -1);
        assert_eq!(stats.opens.load(Ordering::Relaxed), 0);
        {
            let captured = logs.lock().unwrap();
            assert!(
                captured
                    .iter()
                    .any(|message| message.contains("invalid CoreAudio configuration")
                        && message.contains("sample rate")),
                "logs: {captured:?}"
            );
        }

        assert!(!host.start(Some(FormatRequest {
            sample_rate: 48_000.0,
            buffer_size: MAX_FRAMES_PER_SLICE + 1,
        })));
        assert_eq!(host.last_error_code(), -1);
        {
            let captured = logs.lock().unwrap();
            assert!(
                captured
                    .iter()
                    .any(|message| message.contains("buffer size") && message.contains("8192")),
                "logs: {captured:?}"
            );
        }

        // A valid request afterwards recovers the generation.
        assert!(host.start(Some(FormatRequest {
            sample_rate: 48_000.0,
            buffer_size: 256,
        })));
        assert_eq!(host.status(), "running");
        assert_eq!(host.last_error_code(), 0);
    }

    #[test]
    fn odd_frame_counts_split_without_losing_tail_frames() {
        assert_eq!(
            split_frames(1_000, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 488)]
        );
        assert_eq!(
            split_frames(1_023, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 511)]
        );
        assert_eq!(
            split_frames(513, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 1)]
        );
        assert_eq!(split_frames(256, 512).collect::<Vec<_>>(), vec![(0, 256)]);
        assert_eq!(split_frames(0, 512).collect::<Vec<_>>(), Vec::new());
    }

    struct ChunkRecorder {
        chunks: Mutex<Vec<(u32, usize)>>,
    }

    unsafe extern "C" fn record_chunks(
        _inputs: *const *const f32,
        _input_channels: u32,
        outputs: *const *mut f32,
        output_channels: u32,
        frames: u32,
        context: *mut c_void,
    ) {
        let recorder = &*(context as *const ChunkRecorder);
        recorder
            .chunks
            .lock()
            .unwrap()
            .push((frames, output_channels as usize));
        for channel in 0..output_channels as usize {
            let output = *outputs.add(channel);
            if !output.is_null() {
                for frame in 0..frames as usize {
                    output.add(frame).write(0.25);
                }
            }
        }
    }

    #[test]
    fn realtime_dispatch_splits_odd_callbacks_into_bounded_chunks() {
        let realtime = RealtimeState::new();
        realtime.running.store(true, Ordering::Release);
        realtime.buffer_size.store(512, Ordering::Release);
        realtime.output_channels.store(2, Ordering::Release);
        let recorder = Arc::new(ChunkRecorder {
            chunks: Mutex::new(Vec::new()),
        });
        realtime.set_process_callback(
            Some(record_chunks),
            Arc::as_ptr(&recorder) as *mut c_void,
        );

        let mut left = vec![0.0f32; 1_023];
        let mut right = vec![0.0f32; 1_023];
        let mut outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        outputs[0] = left.as_mut_ptr();
        outputs[1] = right.as_mut_ptr();
        let dispatched = unsafe {
            dispatch_planar_outputs(
                &realtime,
                ptr::null(),
                0,
                &outputs,
                2,
                1_023,
            )
        };
        assert!(dispatched);
        assert_eq!(
            *recorder.chunks.lock().unwrap(),
            vec![(512, 2), (511, 2)]
        );
        assert_eq!(left[1_022], 0.25);
        assert_eq!(right[1_022], 0.25);
        assert_eq!(realtime.stats.callback_count(), 1);
        assert!(realtime.stats.output_peak() >= 0.25);
    }

    #[test]
    fn channel_map_rejects_the_mono_device_the_deleted_cpp_contract_forced() {
        // The deleted `Hirari::Platform` device contract opened a mono output
        // to prove the backend did not assume two buffers. The live driver
        // negotiates channels from the device instead of letting the caller
        // pick them, and refuses anything that cannot cover the stereo engine
        // boundary, so the assertion now sits on the mapping policy.
        assert_eq!(channel_map(1, 1), Err(DriverError::NoOutputChannels(1)));
        assert_eq!(channel_map(0, 2), Err(DriverError::NoInputChannels(0)));
        assert_eq!(
            channel_map(40, 40),
            Ok(ChannelMap {
                inputs: 32,
                outputs: 32
            })
        );
    }

    // --- duplex composition (private aggregate) -------------------------

    #[test]
    fn a_duplex_output_device_is_opened_without_an_aggregate() {
        assert_eq!(
            plan_duplex_composition(4, 2, 9, 1, Some("BuiltInMic"), Some("Interface")),
            None
        );
        // Even the device being its own default input stays untouched.
        assert_eq!(
            plan_duplex_composition(4, 1, 4, 1, Some("Duplex"), Some("Duplex")),
            None
        );
    }

    #[test]
    fn a_missing_default_input_device_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 0, 1, None, Some("Speakers")),
            None
        );
        assert_eq!(
            plan_duplex_composition(4, 0, 0, 0, None, Some("Speakers")),
            None
        );
    }

    #[test]
    fn an_input_device_without_channels_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 0, Some("Useless"), Some("Speakers")),
            None
        );
    }

    #[test]
    fn an_output_device_without_a_uid_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 1, Some("Mic"), Some("")),
            None
        );
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 1, Some("Mic"), None),
            None
        );
    }

    #[test]
    fn an_output_only_device_composes_with_both_sub_device_uids() {
        let composition = plan_duplex_composition(
            4,
            0,
            9,
            1,
            Some("AppleHDAEngineInput:1B,0,18,1"),
            Some("AppleHDAEngineOutput:1B,0,19,3"),
        )
        .expect("an output-only device plus a real input device must compose");
        assert_eq!(composition.input_uid, "AppleHDAEngineInput:1B,0,18,1");
        assert_eq!(composition.output_uid, "AppleHDAEngineOutput:1B,0,19,3");
    }

    /// Backend whose default output device is output-only (0, 2) and only
    /// becomes openable once `resolve_for_open` has composed a device: the
    /// composed id reports (1, 2), and `open` records what it was handed.
    struct ComposingBackend {
        composed: u32,
        opened_with: Arc<Mutex<Option<u32>>>,
        probed: Arc<Mutex<Vec<u32>>>,
        opened: bool,
        realtime: Option<Arc<RealtimeState>>,
    }

    impl ComposingBackend {
        /// Builds a host plus the probes the assertions read back without
        /// having to downcast the trait object.
        fn host(
            composed: u32,
        ) -> (MacAudioHost, Arc<Mutex<Option<u32>>>, Arc<Mutex<Vec<u32>>>) {
            let opened_with = Arc::new(Mutex::new(None));
            let probed = Arc::new(Mutex::new(Vec::new()));
            let backend = Self {
                composed,
                opened_with: Arc::clone(&opened_with),
                probed: Arc::clone(&probed),
                opened: false,
                realtime: None,
            };
            let host = MacAudioHost::new(Box::new(backend), None, ptr::null_mut());
            (host, opened_with, probed)
        }
    }

    impl AudioBackend for ComposingBackend {
        fn default_output_device(&self) -> Result<u32, i32> {
            Ok(4)
        }

        fn nominal_sample_rate(&self, _device: u32) -> Option<f64> {
            Some(48_000.0)
        }

        fn buffer_frame_size(&self, _device: u32) -> Option<u32> {
            Some(256)
        }

        fn channel_counts(&self, device: u32) -> Result<(u32, u32), DriverError> {
            self.probed.lock().unwrap().push(device);
            if device == self.composed {
                Ok((1, 2))
            } else {
                Ok((0, 2))
            }
        }

        fn open(
            &mut self,
            _request: FormatRequest,
            requested_device: u32,
            _channels: ChannelMap,
            realtime: Arc<RealtimeState>,
        ) -> Result<u32, DriverError> {
            *self.opened_with.lock().unwrap() = Some(requested_device);
            self.opened = true;
            self.realtime = Some(realtime);
            Ok(requested_device)
        }

        fn start(&mut self) -> Result<(), DriverError> {
            if let Some(realtime) = &self.realtime {
                realtime.running.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn close(&mut self) {
            self.opened = false;
            if let Some(realtime) = self.realtime.take() {
                realtime.running.store(false, Ordering::Release);
            }
        }

        fn is_open(&self) -> bool {
            self.opened
        }

        fn device_uid(&self, device: u32) -> Option<String> {
            Some(format!("uid-{device}"))
        }

        fn devices_json(&self) -> String {
            "[]".to_owned()
        }

        fn take_default_device_change(&self) -> bool {
            false
        }

        fn resolve_for_open(&self, output_device: u32, _request: FormatRequest) -> u32 {
            if self.composed != 0 {
                self.composed
            } else {
                output_device
            }
        }

        fn input_endpoint_for(&self, opened_device: u32) -> u32 {
            if self.composed != 0 && opened_device == self.composed {
                9
            } else {
                opened_device
            }
        }
    }

    #[test]
    fn start_probes_and_opens_the_composed_device() {
        let (host, opened_with, probed) = ComposingBackend::host(42);
        assert!(host.start(None));
        assert_eq!(host.status(), "running");
        // The channel probe must see the composed device, otherwise the
        // output-only default would still reject with NoInputChannels.
        assert_eq!(*probed.lock().unwrap(), vec![42u32]);
        assert_eq!(*opened_with.lock().unwrap(), Some(42));
        host.stop();
        assert_eq!(host.status(), "stopped");
    }

    #[test]
    fn an_explicit_selection_negotiates_its_own_rate_not_the_defaults() {
        let (host, _stats, _logs) = fake_host();
        host.lock_core().selected_device = 11;
        assert!(host.start(None));
        assert_eq!(host.lock_core().sample_rate(), 96_000.0);
    }

    #[test]
    fn the_composed_aggregate_is_never_reported_as_the_input_endpoint() {
        let (host, _opened_with, _probed) = ComposingBackend::host(42);
        assert!(host.start(None));
        // The UI and recording key off the physical capture endpoint; the
        // private per-process aggregate UID would go stale on stop and can
        // never be re-selected or persisted.
        assert_eq!(host.selected_device_uid(), "uid-9");
        host.stop();
        assert_eq!(host.selected_device_uid(), "");
    }

    #[test]
    fn start_still_fails_exactly_as_before_when_nothing_is_composed() {
        let (host, opened_with, _probed) = ComposingBackend::host(0);
        assert!(!host.start(None));
        assert_eq!(host.status(), "start-failed");
        assert_eq!(host.last_error_code(), -1);
        // The identity path must not open anything: this is today's
        // output-only behaviour, rejection included.
        assert_eq!(*opened_with.lock().unwrap(), None);
    }
}
