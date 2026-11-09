//! macOS AudioUnit v2 hosting for the isolated plugin worker.
//!
//! Bundle discovery, component selection, stream-format negotiation, the
//! render/parameter/MIDI callback bridge and the ClassInfo state envelope all
//! live here. The worker's C++ layer only marshals the shared mailbox.
//!
//! AudioUnit callbacks are invoked synchronously by `AudioUnitRender` on this
//! thread, so per-block state is shared with them through `UnsafeCell` instead
//! of a Rust reference that would alias the renderer's `&mut`.

use crate::clap_events::{MailboxMidiEvent, MailboxParameterChange};
use core::cell::UnsafeCell;
use core::ffi::{c_char, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};
use std::boxed::Box;
use std::ffi::CStr;
use std::vec::Vec;

use coreaudio_sys as sys;

// The AudioToolbox/CoreFoundation symbols come from the macOS frameworks.
// Declaring them here keeps standalone test binaries linkable without the
// worker script's explicit framework flags.
#[link(name = "AudioToolbox", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {}

/// Channel-plane stride of `SharedAudioBlock::input` / `output`. The mailbox
/// fixes it at `kMaxFrames`; the worker never passes a different layout.
const SHARED_CHANNEL_STRIDE: usize = 8192;
const MAX_CHANNELS: u32 = 2;
const MAX_MIDI_EVENTS: usize = 1024;
const MAX_PARAMETER_CHANGES: usize = 4096;
const STATE_ERROR_PLUGIN: u8 = 4;
const STATE_MODE_SAVE: u8 = 1;
const STATE_MODE_LOAD: u8 = 2;

#[repr(C)]
struct AudioBufferList2 {
    number_buffers: u32,
    buffers: [sys::AudioBuffer; MAX_CHANNELS as usize],
}

#[derive(Clone, Copy)]
struct BlockState {
    input: *const f32,
    channels: u32,
    frames: u32,
    parameter_error: bool,
}

impl Default for BlockState {
    fn default() -> Self {
        Self {
            input: ptr::null(),
            channels: 0,
            frames: 0,
            parameter_error: false,
        }
    }
}

struct AuHost {
    bundle: sys::CFBundleRef,
    unit: sys::AudioUnit,
    component_type: sys::OSType,
    sample_rate: f64,
    max_frames: u32,
    initialized: bool,
    render_notify: bool,
    block: UnsafeCell<BlockState>,
    events: UnsafeCell<Vec<sys::AudioUnitParameterEvent>>,
}

impl AuHost {
    fn has_audio_input(&self) -> bool {
        self.component_type != sys::kAudioUnitType_MusicDevice as sys::OSType
            && self.component_type != sys::kAudioUnitType_MusicEffect as sys::OSType
    }
}

fn status_ok(status: sys::OSStatus) -> bool {
    status == sys::noErr as sys::OSStatus
}

fn cf_release(value: *const c_void) {
    if !value.is_null() {
        unsafe { sys::CFRelease(value.cast()) };
    }
}

unsafe fn cf_key(bytes: &'static [u8]) -> sys::CFStringRef {
    unsafe {
        sys::CFStringCreateWithCString(
            sys::kCFAllocatorDefault,
            bytes.as_ptr().cast::<c_char>(),
            sys::kCFStringEncodingUTF8,
        )
    }
}

unsafe fn dictionary_get(dictionary: sys::CFDictionaryRef, key: &'static [u8]) -> sys::CFTypeRef {
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
unsafe fn four_cc(value: sys::CFTypeRef) -> sys::OSType {
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

unsafe fn read_descriptions(
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
unsafe fn configure_unit(host: *mut AuHost, channels: u32) -> bool {
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

unsafe fn shutdown_unit(host: *mut AuHost) {
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
    unsafe { (*state.block.get()).input = ptr::null() };
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

/// Loads the first instantiable component from an AU package. Returns null for
/// every rejected candidate: a component that can be instantiated may still
/// refuse its stream format or fail initialization.
#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_load(
    path: *const c_char,
    sample_rate: f64,
    max_frames: u32,
    channels: u32,
    midi_instrument: *mut bool,
) -> *mut c_void {
    if path.is_null()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || max_frames == 0
        || channels == 0
        || channels > MAX_CHANNELS
        || midi_instrument.is_null()
    {
        return ptr::null_mut();
    }
    let path_bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    if path_bytes.is_empty() {
        return ptr::null_mut();
    }
    let url = unsafe {
        sys::CFURLCreateFromFileSystemRepresentation(
            sys::kCFAllocatorDefault,
            path_bytes.as_ptr(),
            path_bytes.len() as sys::CFIndex,
            1,
        )
    };
    if url.is_null() {
        return ptr::null_mut();
    }
    let bundle = unsafe { sys::CFBundleCreate(sys::kCFAllocatorDefault, url) };
    cf_release(url.cast());
    if bundle.is_null() {
        return ptr::null_mut();
    }
    if unsafe { sys::CFBundleLoadExecutable(bundle) } == 0 {
        cf_release(bundle as *const c_void);
        return ptr::null_mut();
    }
    let Some(descriptions) = (unsafe { read_descriptions(bundle) }) else {
        unsafe { sys::CFBundleUnloadExecutable(bundle) };
        cf_release(bundle as *const c_void);
        return ptr::null_mut();
    };
    let mut host = Box::new(AuHost {
        bundle,
        unit: ptr::null_mut(),
        component_type: 0,
        sample_rate,
        max_frames,
        initialized: false,
        render_notify: false,
        block: UnsafeCell::new(BlockState::default()),
        events: UnsafeCell::new(Vec::with_capacity(MAX_PARAMETER_CHANGES)),
    });
    let host_ptr: *mut AuHost = &mut *host;
    let mut ready = false;
    for description in descriptions {
        let component = unsafe { sys::AudioComponentFindNext(ptr::null_mut(), &description) };
        if component.is_null() {
            continue;
        }
        let mut unit: sys::AudioUnit = ptr::null_mut();
        if !status_ok(unsafe { sys::AudioComponentInstanceNew(component, &mut unit) })
            || unit.is_null()
        {
            continue;
        }
        host.unit = unit;
        host.component_type = description.componentType;
        if unsafe { configure_unit(host_ptr, channels) } {
            ready = true;
            break;
        }
        unsafe { shutdown_unit(host_ptr) };
    }
    if !ready {
        unsafe { sys::CFBundleUnloadExecutable(bundle) };
        cf_release(bundle as *const c_void);
        return ptr::null_mut();
    }
    unsafe {
        *midi_instrument = host.component_type == sys::kAudioUnitType_MusicDevice as sys::OSType
    };
    Box::into_raw(host).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_destroy(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    let host_ptr = instance.cast::<AuHost>();
    unsafe { shutdown_unit(host_ptr) };
    let bundle = unsafe { (*host_ptr).bundle };
    if !bundle.is_null() {
        unsafe {
            sys::CFBundleUnloadExecutable(bundle);
            sys::CFRelease(bundle as *const c_void);
            (*host_ptr).bundle = ptr::null_mut();
        }
    }
    drop(unsafe { Box::from_raw(host_ptr) });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_is_midi(instance: *const c_void) -> bool {
    unsafe { instance.cast::<AuHost>().as_ref() }
        .is_some_and(|host| host.component_type == sys::kAudioUnitType_MusicDevice as sys::OSType)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_reset(
    instance: *mut c_void,
    _sample_rate: f64,
    _min_frames: u32,
    _max_frames: u32,
) -> bool {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return false;
    };
    if host.unit.is_null() {
        return false;
    }
    status_ok(unsafe { sys::AudioUnitReset(host.unit, sys::kAudioUnitScope_Global, 0) })
}

/// Processor latency in samples via `kAudioUnitProperty_Latency` (seconds).
#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_latency(instance: *mut c_void) -> u32 {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return 0;
    };
    if host.unit.is_null() || !host.sample_rate.is_finite() || host.sample_rate <= 0.0 {
        return 0;
    }
    let mut seconds = 0.0f64;
    let mut size = core::mem::size_of::<f64>() as u32;
    let status = unsafe {
        sys::AudioUnitGetProperty(
            host.unit,
            sys::kAudioUnitProperty_Latency as sys::AudioUnitPropertyID,
            sys::kAudioUnitScope_Global,
            0,
            (&mut seconds as *mut f64).cast(),
            &mut size,
        )
    };
    if !status_ok(status) || !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * host.sample_rate).round() as u32
}

/// Projects one shared-mailbox block into the AudioUnit callbacks. Parameter
/// changes are scheduled at their exact offsets immediately before
/// `AudioUnitRender`; the input callback only fills channel pointers.
#[no_mangle]
pub unsafe extern "C" fn hirari_au_process_block(
    instance: *mut c_void,
    input_audio: *const f32,
    output_audio: *mut f32,
    channels: u32,
    frames: u32,
    midi_events: *const c_void,
    midi_count: u32,
    parameter_changes: *const c_void,
    parameter_count: u32,
) -> bool {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return false;
    };
    if host.unit.is_null()
        || input_audio.is_null()
        || output_audio.is_null()
        || channels == 0
        || channels > MAX_CHANNELS
        || frames == 0
        || frames > host.max_frames
    {
        return false;
    }
    unsafe {
        let block = &mut *host.block.get();
        block.input = input_audio;
        block.channels = channels;
        block.frames = frames;
        block.parameter_error = false;
    }

    let events = unsafe { &mut *host.events.get() };
    events.clear();
    if !parameter_changes.is_null() && parameter_count != 0 {
        let count = (parameter_count as usize).min(MAX_PARAMETER_CHANGES);
        let changes = unsafe {
            core::slice::from_raw_parts(parameter_changes.cast::<MailboxParameterChange>(), count)
        };
        for change in changes {
            if change.sample_offset >= frames || !change.value.is_finite() {
                continue;
            }
            events.push(sys::AudioUnitParameterEvent {
                scope: sys::kAudioUnitScope_Global,
                element: 0,
                parameter: change.parameter_id,
                eventType: sys::kParameterEvent_Immediate as sys::AUParameterEventType,
                eventValues: sys::AudioUnitParameterEvent__bindgen_ty_1 {
                    immediate: sys::AudioUnitParameterEvent__bindgen_ty_1__bindgen_ty_2 {
                        bufferOffset: change.sample_offset,
                        value: change.value as f32,
                    },
                },
            });
        }
    }

    // Instruments receive MIDI through the MusicDevice API rather than the
    // render callback; forward the bounded mailbox before rendering so the
    // note state belongs to this block.
    if host.component_type == sys::kAudioUnitType_MusicDevice as sys::OSType
        || host.component_type == sys::kAudioUnitType_MusicEffect as sys::OSType
    {
        if !midi_events.is_null() && midi_count != 0 {
            let count = (midi_count as usize).min(MAX_MIDI_EVENTS);
            let messages = unsafe {
                core::slice::from_raw_parts(midi_events.cast::<MailboxMidiEvent>(), count)
            };
            for message in messages {
                if (3..=message.data.len()).contains(&(message.size as usize))
                    && message.sample_offset < u64::from(frames)
                {
                    let _ = unsafe {
                        sys::MusicDeviceMIDIEvent(
                            host.unit,
                            u32::from(message.data[0]),
                            u32::from(message.data[1]),
                            u32::from(message.data[2]),
                            message.sample_offset as u32,
                        )
                    };
                }
            }
        }
    }

    let mut list = AudioBufferList2 {
        number_buffers: channels,
        buffers: [sys::AudioBuffer::default(); MAX_CHANNELS as usize],
    };
    for channel in 0..channels as usize {
        list.buffers[channel] = sys::AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: frames * 4,
            mData: unsafe { output_audio.add(channel * SHARED_CHANNEL_STRIDE) }.cast(),
        };
    }
    let mut flags: sys::AudioUnitRenderActionFlags = 0;
    let mut timestamp: sys::AudioTimeStamp = unsafe { core::mem::zeroed() };
    timestamp.mFlags = sys::kAudioTimeStampSampleTimeValid as sys::AudioTimeStampFlags;
    let status = unsafe {
        sys::AudioUnitRender(
            host.unit,
            &mut flags,
            &timestamp,
            0,
            frames,
            (&mut list as *mut AudioBufferList2).cast(),
        )
    };
    let parameter_ok = unsafe {
        let block = &mut *host.block.get();
        block.input = ptr::null();
        !block.parameter_error
    };
    status_ok(status) && parameter_ok
}

/// Performs the ClassInfo save/load transaction, including mailbox
/// publication and checksum validation, mirroring `hirari_clap_state_block`.
#[no_mangle]
pub unsafe extern "C" fn hirari_au_state_block(
    instance: *mut c_void,
    mode: u8,
    state: *mut u8,
    state_size: *mut c_void,
    state_version: *mut c_void,
    state_checksum: *mut c_void,
) -> u16 {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    };
    if host.unit.is_null()
        || state.is_null()
        || state_size.is_null()
        || state_version.is_null()
        || state_checksum.is_null()
    {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    }
    if mode == STATE_MODE_SAVE {
        let mut class_info: sys::CFPropertyListRef = ptr::null();
        let mut size = core::mem::size_of::<sys::CFPropertyListRef>() as u32;
        let read = unsafe {
            sys::AudioUnitGetProperty(
                host.unit,
                sys::kAudioUnitProperty_ClassInfo as sys::AudioUnitPropertyID,
                sys::kAudioUnitScope_Global,
                0,
                (&mut class_info as *mut sys::CFPropertyListRef).cast(),
                &mut size,
            )
        };
        if !status_ok(read) || class_info.is_null() {
            return u16::from(STATE_ERROR_PLUGIN) << 8;
        }
        let encoded = unsafe {
            sys::CFPropertyListCreateData(
                sys::kCFAllocatorDefault,
                class_info,
                sys::kCFPropertyListBinaryFormat_v1_0 as sys::CFPropertyListFormat,
                0,
                ptr::null_mut(),
            )
        };
        cf_release(class_info);
        if encoded.is_null() {
            return u16::from(STATE_ERROR_PLUGIN) << 8;
        }
        let length = unsafe { sys::CFDataGetLength(encoded) };
        let fits = length > 0
            && length as u64 <= crate::MAX_STATE_BYTES as u64
            && length <= u32::MAX as sys::CFIndex;
        if !fits {
            cf_release(encoded.cast());
            return u16::from(STATE_ERROR_PLUGIN) << 8;
        }
        unsafe {
            ptr::copy_nonoverlapping(sys::CFDataGetBytePtr(encoded), state, length as usize);
        }
        cf_release(encoded.cast());
        let error = unsafe {
            crate::worker_runtime::hirari_plugin_worker_publish_state(
                state,
                length as u32,
                state_size,
                state_version,
                state_checksum,
            )
        };
        return (u16::from(error) << 8) | u16::from(error == crate::STATE_ERROR_NONE);
    }
    if mode == STATE_MODE_LOAD {
        let mut error = unsafe {
            crate::worker_runtime::hirari_plugin_worker_validate_state(
                state,
                state_size,
                state_version,
                state_checksum,
            )
        };
        if error != crate::STATE_ERROR_NONE {
            return u16::from(error) << 8;
        }
        let length = unsafe { &*state_size.cast::<AtomicU32>() }.load(Ordering::Acquire) as usize;
        let encoded =
            unsafe { sys::CFDataCreate(sys::kCFAllocatorDefault, state, length as sys::CFIndex) };
        if encoded.is_null() {
            return u16::from(STATE_ERROR_PLUGIN) << 8;
        }
        let class_info = unsafe {
            sys::CFPropertyListCreateWithData(
                sys::kCFAllocatorDefault,
                encoded,
                sys::kCFPropertyListMutableContainersAndLeaves as sys::CFOptionFlags,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        cf_release(encoded.cast());
        if class_info.is_null() {
            return u16::from(STATE_ERROR_PLUGIN) << 8;
        }
        let status = unsafe {
            sys::AudioUnitSetProperty(
                host.unit,
                sys::kAudioUnitProperty_ClassInfo as sys::AudioUnitPropertyID,
                sys::kAudioUnitScope_Global,
                0,
                (&class_info as *const sys::CFPropertyListRef).cast(),
                core::mem::size_of::<sys::CFPropertyListRef>() as u32,
            )
        };
        cf_release(class_info);
        let ok = status_ok(status);
        if !ok {
            error = STATE_ERROR_PLUGIN;
        }
        return (u16::from(error) << 8) | u16::from(ok);
    }
    0
}

const _: () = assert!(
    core::mem::size_of::<AudioBufferList2>() >= core::mem::size_of::<sys::AudioBufferList>()
);
