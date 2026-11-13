//! AudioUnit v2 internals: host bag, block state and the CoreFoundation
//! keys, split out of au_host.rs.
use super::*;

pub(crate) const SHARED_CHANNEL_STRIDE: usize = 8192;
pub(crate) const MAX_CHANNELS: u32 = 2;
pub(crate) const MAX_MIDI_EVENTS: usize = 1024;
pub(crate) const MAX_PARAMETER_CHANGES: usize = 4096;
pub(crate) const STATE_ERROR_PLUGIN: u8 = 4;
pub(crate) const STATE_MODE_SAVE: u8 = 1;
pub(crate) const STATE_MODE_LOAD: u8 = 2;

#[repr(C)]
pub(crate) struct AudioBufferList2 {
    pub(crate) number_buffers: u32,
    pub(crate) buffers: [sys::AudioBuffer; MAX_CHANNELS as usize],
}

#[derive(Clone, Copy)]
pub(crate) struct BlockState {
    pub(crate) input: *const f32,
    pub(crate) channels: u32,
    pub(crate) frames: u32,
    pub(crate) parameter_error: bool,
    /// Planar sidechain input (channel-major, `SHARED_CHANNEL_STRIDE`
    /// stride, mirroring `input`). Null means no link this block.
    pub(crate) sidechain: *const f32,
    pub(crate) sidechain_frames: u32,
}

impl Default for BlockState {
    fn default() -> Self {
        Self {
            input: ptr::null(),
            channels: 0,
            frames: 0,
            parameter_error: false,
            sidechain: ptr::null(),
            sidechain_frames: 0,
        }
    }
}

pub(crate) struct AuHost {
    pub(crate) bundle: sys::CFBundleRef,
    pub(crate) unit: sys::AudioUnit,
    pub(crate) component_type: sys::OSType,
    pub(crate) sample_rate: f64,
    pub(crate) max_frames: u32,
    pub(crate) initialized: bool,
    pub(crate) render_notify: bool,
    /// Input bus 1 carries the sidechain detector on units that expose it.
    pub(crate) sidechain: bool,
    pub(crate) block: UnsafeCell<BlockState>,
    pub(crate) events: UnsafeCell<Vec<sys::AudioUnitParameterEvent>>,
}

impl AuHost {
    pub(crate) fn has_audio_input(&self) -> bool {
        self.component_type != sys::kAudioUnitType_MusicDevice as sys::OSType
            && self.component_type != sys::kAudioUnitType_MusicEffect as sys::OSType
    }
}

pub(crate) fn status_ok(status: sys::OSStatus) -> bool {
    status == sys::noErr as sys::OSStatus
}

pub(crate) fn cf_release(value: *const c_void) {
    if !value.is_null() {
        unsafe { sys::CFRelease(value.cast()) };
    }
}

pub(crate) unsafe fn cf_key(bytes: &'static [u8]) -> sys::CFStringRef {
    unsafe {
        sys::CFStringCreateWithCString(
            sys::kCFAllocatorDefault,
            bytes.as_ptr().cast::<c_char>(),
            sys::kCFStringEncodingUTF8,
        )
    }
}

pub(crate) unsafe fn dictionary_get(
    dictionary: sys::CFDictionaryRef,
    key: &'static [u8],
) -> sys::CFTypeRef {
    if dictionary.is_null() {
        return ptr::null();
    }
    let name = unsafe { cf_key(key) };
    if name.is_null() {
        return ptr::null();
    }
    let value = unsafe { sys::CFDictionaryGetValue(dictionary, name.cast()) };
    cf_release(name.cast());
    value
}

/// FourCC component identifiers may arrive as either a CFNumber or a
/// CFString in the bundle's `AudioComponents` array.
pub(crate) unsafe fn four_cc(value: sys::CFTypeRef) -> sys::OSType {
    if value.is_null() {
        return 0;
    }
    let type_id = unsafe { sys::CFGetTypeID(value) };
    if type_id == sys::CFNumberGetTypeID() {
        let mut number: i32 = 0;
        let read = unsafe {
            sys::CFNumberGetValue(
                value.cast::<sys::__CFNumber>(),
                sys::kCFNumberSInt32Type as sys::CFNumberType,
                (&mut number as *mut i32).cast(),
            )
        };
        return if read != 0 { number as sys::OSType } else { 0 };
    }
    if type_id != sys::CFStringGetTypeID() {
        return 0;
    }
    let mut text = [0 as c_char; 5];
    let read = unsafe {
        sys::CFStringGetCString(
            value.cast::<sys::__CFString>(),
            text.as_mut_ptr(),
            text.len() as sys::CFIndex,
            sys::kCFStringEncodingUTF8,
        )
    };
    if read == 0 {
        return 0;
    }
    (text[0] as u8 as u32) << 24
        | (text[1] as u8 as u32) << 16
        | (text[2] as u8 as u32) << 8
        | (text[3] as u8 as u32)
}

pub(crate) unsafe fn read_descriptions(
    bundle: sys::CFBundleRef,
) -> Option<Vec<sys::AudioComponentDescription>> {
    let info = unsafe { sys::CFBundleGetInfoDictionary(bundle) };
    if info.is_null() {
        return None;
    }
    let components = unsafe { dictionary_get(info, b"AudioComponents\0") };
    if components.is_null() || unsafe { sys::CFGetTypeID(components) } != sys::CFArrayGetTypeID() {
        return None;
    }
    let array = components.cast::<sys::__CFArray>();
    let count = unsafe { sys::CFArrayGetCount(array) };
    let mut results = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count {
        let entry = unsafe { sys::CFArrayGetValueAtIndex(array, index) };
        if entry.is_null() || unsafe { sys::CFGetTypeID(entry) } != sys::CFDictionaryGetTypeID() {
            continue;
        }
        let dictionary = entry.cast::<sys::__CFDictionary>();
        let candidate = sys::AudioComponentDescription {
            componentType: unsafe { four_cc(dictionary_get(dictionary, b"type\0")) },
            componentSubType: unsafe { four_cc(dictionary_get(dictionary, b"subtype\0")) },
            componentManufacturer: unsafe {
                four_cc(dictionary_get(dictionary, b"manufacturer\0"))
            },
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        if candidate.componentType != 0
            && candidate.componentSubType != 0
            && candidate.componentManufacturer != 0
        {
            results.push(candidate);
        }
    }
    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

/// Negotiates the non-interleaved float format and installs the render
/// callbacks. Instruments (`aumu`) and MIDI effects have no audio input bus,
/// so writing an input stream format on them makes them reject
/// `AudioUnitInitialize`.
pub(crate) unsafe fn configure_unit(host: *mut AuHost, channels: u32) -> bool {
    let host_ref = unsafe { &*host };
    let unit = host_ref.unit;
    if unit.is_null() {
        return false;
    }
    let format = sys::AudioStreamBasicDescription {
        mSampleRate: host_ref.sample_rate,
        mFormatID: sys::kAudioFormatLinearPCM as sys::AudioFormatID,
        mFormatFlags: (sys::kAudioFormatFlagIsFloat
            | sys::kAudioFormatFlagIsPacked
            | sys::kAudioFormatFlagIsNonInterleaved) as sys::AudioFormatFlags,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    };
    let set_stream_format = |scope: sys::AudioUnitScope| unsafe {
        sys::AudioUnitSetProperty(
            unit,
            sys::kAudioUnitProperty_StreamFormat as sys::AudioUnitPropertyID,
            scope,
            0,
            (&format as *const sys::AudioStreamBasicDescription).cast(),
            core::mem::size_of::<sys::AudioStreamBasicDescription>() as u32,
        )
    };
    let has_input = host_ref.has_audio_input();
    if has_input && !status_ok(set_stream_format(sys::kAudioUnitScope_Input)) {
        return false;
    }
    if !status_ok(set_stream_format(sys::kAudioUnitScope_Output)) {
        return false;
    }
    let max_frames = host_ref.max_frames;
    if !status_ok(unsafe {
        sys::AudioUnitSetProperty(
            unit,
            sys::kAudioUnitProperty_MaximumFramesPerSlice as sys::AudioUnitPropertyID,
            sys::kAudioUnitScope_Global,
            0,
            (&max_frames as *const u32).cast(),
            core::mem::size_of::<u32>() as u32,
        )
    }) {
        return false;
    }
    if has_input {
        let callback = sys::AURenderCallbackStruct {
            inputProc: Some(input_callback),
            inputProcRefCon: host.cast(),
        };
        if !status_ok(unsafe {
            sys::AudioUnitSetProperty(
                unit,
                sys::kAudioUnitProperty_SetRenderCallback as sys::AudioUnitPropertyID,
                sys::kAudioUnitScope_Input,
                0,
                (&callback as *const sys::AURenderCallbackStruct).cast(),
                core::mem::size_of::<sys::AURenderCallbackStruct>() as u32,
            )
        }) {
            return false;
        }
        // Sidechain detector: units with a second input bus conventionally
        // expose it on input element 1. Probe the bus count and hook the
        // sidechain callback with the same stream format as the main input.
        let mut bus_count = 0u32;
        let mut bus_size = core::mem::size_of::<u32>() as u32;
        let second_bus = unsafe {
            status_ok(sys::AudioUnitGetProperty(
                unit,
                sys::kAudioUnitProperty_BusCount as sys::AudioUnitPropertyID,
                sys::kAudioUnitScope_Input,
                0,
                (&mut bus_count as *mut u32).cast(),
                &mut bus_size,
            ))
        } && bus_count > 1;
        if second_bus
            && status_ok(unsafe {
                sys::AudioUnitSetProperty(
                    unit,
                    sys::kAudioUnitProperty_StreamFormat as sys::AudioUnitPropertyID,
                    sys::kAudioUnitScope_Input,
                    1,
                    (&format as *const sys::AudioStreamBasicDescription).cast(),
                    core::mem::size_of::<sys::AudioStreamBasicDescription>() as u32,
                )
            })
        {
            let sidechain_callback = sys::AURenderCallbackStruct {
                inputProc: Some(sidechain_callback),
                inputProcRefCon: host.cast(),
            };
            if status_ok(unsafe {
                sys::AudioUnitSetProperty(
                    unit,
                    sys::kAudioUnitProperty_SetRenderCallback as sys::AudioUnitPropertyID,
                    sys::kAudioUnitScope_Input,
                    1,
                    (&sidechain_callback as *const sys::AURenderCallbackStruct).cast(),
                    core::mem::size_of::<sys::AURenderCallbackStruct>() as u32,
                )
            }) {
                unsafe { (*host).sidechain = true };
            }
        }
    }
    if !status_ok(unsafe { sys::AudioUnitInitialize(unit) }) {
        return false;
    }
    unsafe { (*host).initialized = true };
    if !status_ok(unsafe { sys::AudioUnitAddRenderNotify(unit, Some(render_notify), host.cast()) })
    {
        return false;
    }
    unsafe { (*host).render_notify = true };
    true
}

pub(crate) unsafe fn shutdown_unit(host: *mut AuHost) {
    let state = unsafe { &mut *host };
    if !state.unit.is_null() {
        if state.render_notify {
            unsafe {
                sys::AudioUnitRemoveRenderNotify(state.unit, Some(render_notify), host.cast())
            };
            state.render_notify = false;
        }
        if state.initialized {
            unsafe { sys::AudioUnitUninitialize(state.unit) };
        }
        unsafe { sys::AudioComponentInstanceDispose(state.unit) };
        state.unit = ptr::null_mut();
    }
    state.initialized = false;
    state.sidechain = false;
    unsafe {
        (*state.block.get()).input = ptr::null();
        (*state.block.get()).sidechain = ptr::null();
        (*state.block.get()).sidechain_frames = 0;
    }
}

unsafe extern "C" fn input_callback(
    ref_con: *mut c_void,
    _flags: *mut sys::AudioUnitRenderActionFlags,
    _timestamp: *const sys::AudioTimeStamp,
    _bus: u32,
    frames: u32,
    io_data: *mut sys::AudioBufferList,
) -> sys::OSStatus {
    let Some(host) = (unsafe { ref_con.cast::<AuHost>().as_ref() }) else {
        return -50;
    };
    let block = unsafe { &*host.block.get() };
    if block.input.is_null() || io_data.is_null() || frames > block.frames {
        return -50;
    }
    unsafe {
        (*io_data).mNumberBuffers = block.channels;
        let buffers = (*io_data).mBuffers.as_mut_ptr();
        for channel in 0..block.channels as usize {
            buffers.add(channel).write(sys::AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: frames * 4,
                mData: (block.input as *mut f32)
                    .add(channel * SHARED_CHANNEL_STRIDE)
                    .cast(),
            });
        }
    }
    sys::noErr as sys::OSStatus
}

/// Pull callback for input bus 1 (sidechain). Serves the mailbox sidechain
/// arrays published for this block; an absent link renders as silence.
unsafe extern "C" fn sidechain_callback(
    ref_con: *mut c_void,
    _flags: *mut sys::AudioUnitRenderActionFlags,
    _timestamp: *const sys::AudioTimeStamp,
    _bus: u32,
    frames: u32,
    io_data: *mut sys::AudioBufferList,
) -> sys::OSStatus {
    let Some(host) = (unsafe { ref_con.cast::<AuHost>().as_ref() }) else {
        return -50;
    };
    let block = unsafe { &*host.block.get() };
    if io_data.is_null() || frames > SHARED_CHANNEL_STRIDE as u32 {
        return -50;
    }
    static SILENT: [f32; SHARED_CHANNEL_STRIDE * 2] = [0.0; SHARED_CHANNEL_STRIDE * 2];
    unsafe {
        (*io_data).mNumberBuffers = block.channels.max(1);
        let buffers = (*io_data).mBuffers.as_mut_ptr();
        let frames_usize = frames as usize;
        for channel in 0..(*io_data).mNumberBuffers as usize {
            let channel_index = channel.min(1) * SHARED_CHANNEL_STRIDE;
            if block.sidechain.is_null() {
                buffers.add(channel).write(sys::AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: frames * 4,
                    mData: (SILENT.as_ptr().add(channel_index)) as *mut c_void,
                });
                continue;
            }
            let available = (block.sidechain_frames.min(frames)) as usize;
            // Zero anything the link did not publish so short/no detector
            // reads as silence; the host rewrites the arrays every block.
            if available < frames_usize {
                (block.sidechain as *mut f32)
                    .add(channel_index)
                    .add(available)
                    .write_bytes(0, frames_usize - available);
            }
            buffers.add(channel).write(sys::AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: frames * 4,
                mData: (block.sidechain as *mut f32).add(channel_index) as *mut c_void,
            });
        }
    }
    sys::noErr as sys::OSStatus
}

unsafe extern "C" fn render_notify(
    ref_con: *mut c_void,
    flags: *mut sys::AudioUnitRenderActionFlags,
    _timestamp: *const sys::AudioTimeStamp,
    _bus: u32,
    _frames: u32,
    _data: *mut sys::AudioBufferList,
) -> sys::OSStatus {
    let Some(host) = (unsafe { ref_con.cast::<AuHost>().as_ref() }) else {
        return sys::noErr as sys::OSStatus;
    };
    if flags.is_null() {
        return sys::noErr as sys::OSStatus;
    }
    let events = unsafe { &*host.events.get() };
    if unsafe { *flags }
        & (sys::kAudioUnitRenderAction_PreRender as sys::AudioUnitRenderActionFlags)
        == 0
        || events.is_empty()
    {
        return sys::noErr as sys::OSStatus;
    }
    if status_ok(unsafe {
        sys::AudioUnitScheduleParameters(host.unit, events.as_ptr(), events.len() as u32)
    }) {
        return sys::noErr as sys::OSStatus;
    }
    // Some legacy AUs do not advertise scheduled parameter events. Keep audio
    // flowing and apply the final value at the block boundary.
    let block = unsafe { &mut *host.block.get() };
    for event in events.iter() {
        let value = unsafe { event.eventValues.immediate.value };
        if !status_ok(unsafe {
            sys::AudioUnitSetParameter(
                host.unit,
                event.parameter,
                event.scope,
                event.element,
                value,
                0,
            )
        }) {
            block.parameter_error = true;
        }
    }
    sys::noErr as sys::OSStatus
}
