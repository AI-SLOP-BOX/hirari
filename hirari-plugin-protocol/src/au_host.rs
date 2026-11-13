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
#[path = "au_host_core.rs"]
mod core_;
pub(crate) use core_::*;

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
        sidechain: false,
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

/// Number of parameters advertised by the unit.
#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_parameter_count(instance: *mut c_void) -> u32 {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return 0;
    };
    if host.unit.is_null() {
        return 0;
    }
    let mut first_id = 0u32;
    let mut size = core::mem::size_of::<u32>() as u32;
    let status = unsafe {
        sys::AudioUnitGetProperty(
            host.unit,
            sys::kAudioUnitProperty_ParameterList as sys::AudioUnitPropertyID,
            sys::kAudioUnitScope_Global,
            0,
            (&mut first_id as *mut u32).cast(),
            &mut size,
        )
    };
    if !status_ok(status) || size % 4 != 0 {
        return 0;
    }
    size / 4
}

/// Fills `out` for the `index`-th parameter in the unit's list.
#[no_mangle]
pub unsafe extern "C" fn hirari_au_instance_parameter_info(
    instance: *mut c_void,
    index: u32,
    out: *mut crate::mailbox::PluginParameterInfo,
) -> bool {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return false;
    };
    if host.unit.is_null() || out.is_null() {
        return false;
    }
    let count = hirari_au_instance_parameter_count(instance);
    if index >= count {
        return false;
    }
    let mut ids = vec![0u32; count as usize];
    let mut size = (count * 4) as u32;
    let status = unsafe {
        sys::AudioUnitGetProperty(
            host.unit,
            sys::kAudioUnitProperty_ParameterList as sys::AudioUnitPropertyID,
            sys::kAudioUnitScope_Global,
            0,
            ids.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if !status_ok(status) {
        return false;
    }
    let id = ids[index as usize];
    let mut info = sys::AudioUnitParameterInfo::default();
    let mut info_size = core::mem::size_of::<sys::AudioUnitParameterInfo>() as u32;
    let status = unsafe {
        sys::AudioUnitGetProperty(
            host.unit,
            sys::kAudioUnitProperty_ParameterInfo as sys::AudioUnitPropertyID,
            sys::kAudioUnitScope_Global,
            id,
            (&mut info as *mut sys::AudioUnitParameterInfo).cast(),
            &mut info_size,
        )
    };
    if !status_ok(status) {
        return false;
    }
    let filled = unsafe { &mut *out };
    filled.id = id;
    filled.min_value = f64::from(info.minValue);
    filled.max_value = f64::from(info.maxValue);
    filled.default_value = f64::from(info.defaultValue);
    filled.flags = if info.flags & sys::kAudioUnitParameterFlag_IsWritable != 0 {
        1
    } else {
        2
    };
    let name_bytes = info
        .name
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .take(crate::mailbox::PARAMETER_NAME_BYTES - 1)
        .collect::<Vec<_>>();
    filled.name[..name_bytes.len()].copy_from_slice(&name_bytes);
    if !info.unitName.is_null() {
        let mut buffer = [0u8; crate::mailbox::PARAMETER_UNIT_BYTES];
        let ok = unsafe {
            sys::CFStringGetCString(
                info.unitName,
                buffer.as_mut_ptr().cast::<i8>(),
                buffer.len() as i64,
                sys::kCFStringEncodingUTF8,
            )
        };
        if ok != 0 {
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            filled.unit[..len].copy_from_slice(&buffer[..len]);
        }
    }
    true
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

/// Attaches the mailbox sidechain arrays for the next render. Null means
/// no detector this block (the sidechain callback serves silence).
#[no_mangle]
pub unsafe extern "C" fn hirari_au_set_sidechain_context(
    instance: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
) {
    let Some(host) = (unsafe { instance.cast::<AuHost>().as_ref() }) else {
        return;
    };
    if left.is_null() || right.is_null() {
        return;
    }
    // The mailbox stores planar planes (stride `SHARED_CHANNEL_STRIDE`);
    // only the left base is kept and the right plane follows by stride.
    let _ = right;
    unsafe {
        let block = &mut *host.block.get();
        // Callers attach one contiguous planar region starting at left, and
        // the right plane sits one stride later in the mailbox layout.
        block.sidechain = left;
        block.sidechain_frames = frames;
    }
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
