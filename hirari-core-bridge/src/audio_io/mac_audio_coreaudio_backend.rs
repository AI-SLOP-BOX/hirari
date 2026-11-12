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

#[cfg(target_os = "macos")]
#[path = "mac_audio_coreaudio_backend_impl.rs"]
mod backend_impl;

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
