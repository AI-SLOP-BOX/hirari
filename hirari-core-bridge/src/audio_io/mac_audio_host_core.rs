//! Host-side engine orchestration core, split out of mac_audio_driver.rs.
use super::*;

pub(crate) struct HostCore {
    pub(crate) backend: Box<dyn AudioBackend>,
    pub(crate) selected_device: u32,
    negotiated: Option<OpenFormat>,
}

impl HostCore {
    pub(crate) fn new(backend: Box<dyn AudioBackend>) -> Self {
        Self {
            backend,
            selected_device: 0,
            negotiated: None,
        }
    }

    pub(crate) fn default_request(&self, log: Option<LogCallback>, log_context: *mut c_void) -> FormatRequest {
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

    pub(crate) fn start(
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

    pub(crate) fn fail(
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

    pub(crate) fn stop(&mut self, state: &AtomicU8, realtime: &Arc<RealtimeState>) {
        if self.backend.is_open() {
            self.backend.close();
            self.negotiated = None;
            realtime.queue.reset();
            state.store(STATE_STOPPED, Ordering::Release);
        } else if state.load(Ordering::Acquire) == STATE_RUNNING {
            state.store(STATE_STOPPED, Ordering::Release);
        }
    }

    pub(crate) fn reconfigure(
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

    pub(crate) fn select_device(
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

    pub(crate) fn try_reconnect(
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

    pub(crate) fn is_running(&self, state: &AtomicU8, realtime: &RealtimeState) -> bool {
        state.load(Ordering::Acquire) == STATE_RUNNING
            && realtime.running.load(Ordering::Acquire)
            && !realtime.stopping.load(Ordering::Acquire)
    }

    pub(crate) fn sample_rate(&self) -> f64 {
        self.negotiated
            .map(|format| format.sample_rate)
            .unwrap_or(FALLBACK_SAMPLE_RATE)
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        self.negotiated
            .map(|format| format.buffer_size)
            .unwrap_or(FALLBACK_BUFFER_SIZE)
    }

    pub(crate) fn input_channel_count(&self) -> u32 {
        self.negotiated.map(|format| format.channels.inputs).unwrap_or(0)
    }

    pub(crate) fn output_channel_count(&self) -> u32 {
        self.negotiated
            .map(|format| format.channels.outputs)
            .unwrap_or(0)
    }

    pub(crate) fn selected_device_for_uid(&self, realtime: &RealtimeState) -> u32 {
        if self.is_running_state(realtime) {
            self.negotiated
                .map(|format| self.backend.input_endpoint_for(format.device))
                .unwrap_or(0)
        } else {
            self.selected_device
        }
    }

    pub(crate) fn is_running_state(&self, realtime: &RealtimeState) -> bool {
        self.negotiated.is_some() && realtime.running.load(Ordering::Acquire)
    }
}
