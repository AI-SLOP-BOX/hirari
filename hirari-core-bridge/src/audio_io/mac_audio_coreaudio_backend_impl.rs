//! AudioBackend trait implementation, split out of mac_audio_coreaudio_backend.rs.
use super::*;

impl AudioBackend for CoreAudioBackend {
    /// Measures the input-path latency in seconds: device + unit latency
    /// properties plus one callback buffer of staging. Previously the
    /// driver never queried either property, so recordings ran late by
    /// the whole I/O round-trip with no compensation possible.
    fn io_latency_seconds(
        &self,
        device: u32,
        unit: sys::AudioUnit,
        sample_rate: f64,
        buffer_frames: u32,
    ) -> f64 {
        let device_seconds = unsafe {
            read_property::<f64>(
                device,
                sys::kAudioDevicePropertyLatency,
                sys::kAudioObjectPropertyScopeGlobal,
                sys::kAudioObjectPropertyElementMain,
            )
            .unwrap_or(0.0)
        };
        let mut unit_seconds = 0.0f64;
        let status = unsafe {
            au_get(
                unit,
                sys::kAudioUnitProperty_Latency,
                Scope::Global,
                Element::Output,
                &mut unit_seconds,
            )
        };
        if status != 0 || !unit_seconds.is_finite() {
            unit_seconds = 0.0;
        }
        let buffer_seconds = if sample_rate > 0.0 {
            f64::from(buffer_frames) / sample_rate
        } else {
            0.0
        };
        (device_seconds.max(0.0) + unit_seconds.max(0.0) + buffer_seconds).clamp(0.0, 1.0)
    }
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
