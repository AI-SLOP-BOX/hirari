//! CoreAudio AudioUnit backend for macOS.
use super::*;

pub(crate) struct CoreAudioBackend {
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

        let io_latency = self.io_latency_seconds(
            device,
            unit,
            request.sample_rate,
            request.buffer_size,
        );
        realtime.set_io_latency_secs(io_latency);

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

pub(super) unsafe extern "C" fn default_device_listener(
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

pub(super) unsafe extern "C" fn device_alive_listener(
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

pub(super) fn stream_format(sample_rate: f64, channels: u32) -> sys::AudioStreamBasicDescription {
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

pub(super) unsafe fn au_set<T>(
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

pub(super) unsafe fn au_get<T>(
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
