//! Isolated worker executable startup.
//!
//! The CLAP loader, AudioUnit host, mailbox mapping and control loop all live
//! in this crate, so the worker process itself is Rust. The only format that
//! still needs C++ is VST3, whose official ABI interfaces are C++-only; the
//! VST3 hooks below delegate to that adapter and never carry startup policy.

use core::ffi::{c_char, c_void, CStr};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::ffi::CString;

#[cfg(target_os = "macos")]
use crate::au_host::{
    hirari_au_instance_destroy, hirari_au_instance_load, hirari_au_instance_reset,
    hirari_au_process_block, hirari_au_state_block,
};
use crate::clap_events::{
    hirari_clap_input_context_create, hirari_clap_input_context_destroy, hirari_clap_process_block,
    hirari_clap_state_block,
};
use crate::clap_loader::{
    hirari_clap_instance_destroy, hirari_clap_instance_load, hirari_clap_instance_plugin,
    hirari_clap_instance_reset, hirari_clap_instance_state, hirari_plugin_resolve_image_path,
    hirari_plugin_resolved_image_free,
};
use crate::worker_runtime::{
    hirari_plugin_worker_apply_limits, hirari_plugin_worker_load_failure,
    hirari_plugin_worker_map_shared, hirari_plugin_worker_parse_args,
    hirari_plugin_worker_parse_error_text, hirari_plugin_worker_run,
    hirari_plugin_worker_unmap_shared, WorkerBackend, WorkerFaultConfig, WorkerLaunchOptions,
    WORKER_FORMAT_UNSUPPORTED,
};

const STATUS_READY: u8 = 0xA1;
const ERROR_LOAD: u8 = 0xB0;
const ERROR_ABI: u8 = 0xB1;
const ERROR_INSTANCE: u8 = 0xB2;
const ERROR_SECURITY: u8 = 0xB3;
const ERROR_UNSUPPORTED: u8 = 0xB4;
const AUDIO_BLOCK_PROTOCOL_VERSION: u32 = 5;
const STATE_ERROR_PLUGIN: u8 = 4;
const MAX_CHANNELS: u32 = 2;
const MAX_FRAMES: u32 = 8192;

const FORMAT_NONE: u8 = 0;
const FORMAT_CLAP: u8 = 1;
const FORMAT_AUDIO_UNIT: u8 = 2;
const FORMAT_VST3: u8 = 3;

/// Pointers into the shared mailbox, filled by the C++ protocol header owner so
/// this side never duplicates the `SharedAudioBlock` layout.
#[repr(C)]
pub struct WorkerMailboxView {
    pub fields: [*mut c_void; 18],
}

impl WorkerMailboxView {
    fn field(&self, index: usize) -> *mut c_void {
        self.fields[index]
    }
}

const MAILBOX_PROTOCOL_VERSION: usize = 0;
const MAILBOX_ACTIVE_SAMPLE_RATE: usize = 1;
const MAILBOX_ACTIVE_CHANNELS: usize = 2;
const MAILBOX_MIDI_INSTRUMENT: usize = 3;
const MAILBOX_OUTPUT_MIDI_EVENTS: usize = 4;
const MAILBOX_OUTPUT_MIDI_DROPPED: usize = 5;
const MAILBOX_OUTPUT_MIDI: usize = 6;
const MAILBOX_INPUT: usize = 7;
const MAILBOX_OUTPUT: usize = 8;
const MAILBOX_MIDI: usize = 9;
const MAILBOX_MIDI_EVENTS: usize = 10;
const MAILBOX_PARAMETER_CHANGE: usize = 11;
const MAILBOX_PARAMETER_CHANGES: usize = 12;
const MAILBOX_EXTENDED_MIDI: usize = 13;
const MAILBOX_STATE: usize = 14;
const MAILBOX_STATE_SIZE: usize = 15;
const MAILBOX_STATE_VERSION: usize = 16;
const MAILBOX_STATE_CHECKSUM: usize = 17;

unsafe extern "C" {
    fn hirari_plugin_worker_shared_bytes() -> usize;
    fn hirari_plugin_worker_mailbox_view(shared: *mut c_void, out: *mut WorkerMailboxView);
    fn write(fd: i32, buffer: *const c_void, count: usize) -> isize;

    fn hirari_worker_vst3_supported() -> bool;
    fn hirari_worker_vst3_load(
        path: *const c_char,
        sample_rate: f64,
        max_frames: u32,
        midi_instrument: *mut bool,
    ) -> *mut c_void;
    fn hirari_worker_vst3_destroy(instance: *mut c_void);
    fn hirari_worker_vst3_reset(
        instance: *mut c_void,
        sample_rate: f64,
        min_frames: u32,
        max_frames: u32,
    ) -> bool;
    fn hirari_worker_vst3_process(
        instance: *mut c_void,
        shared: *mut c_void,
        channels: u32,
        frames: u32,
    ) -> bool;
    fn hirari_worker_vst3_state(instance: *mut c_void, shared: *mut c_void, mode: u8) -> u16;
}

#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
}

/// Worker formatting calls are deliberately quiet unless the launcher asked for
/// the worker's debug trace.
fn debug_log(message: &str) {
    if std::env::var_os("HIRARI_PLUGIN_DEBUG").is_some() {
        eprintln!("[hirari-plugin-worker] {message}");
    }
}

unsafe fn send_status(fd: i32, byte: u8) -> bool {
    unsafe { write(fd, (&byte as *const u8).cast::<c_void>(), 1) == 1 }
}

fn capabilities_json() -> &'static str {
    let vst3 = unsafe { hirari_worker_vst3_supported() };
    match (cfg!(target_os = "macos"), vst3) {
        (true, true) => "{\"clap\":true,\"au\":true,\"vst3\":true}\n",
        (true, false) => "{\"clap\":true,\"au\":true,\"vst3\":false}\n",
        (false, _) => "{\"clap\":true,\"au\":false,\"vst3\":false}\n",
    }
}

struct WorkerContext {
    format: u8,
    shared: *mut c_void,
    view: WorkerMailboxView,
    clap_instance: *mut c_void,
    clap_plugin: *const c_void,
    clap_state: *const c_void,
    clap_input: *mut c_void,
    au_instance: *mut c_void,
    vst3_instance: *mut c_void,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
}

extern "C" fn backend_reset(opaque: *mut c_void) -> bool {
    let Some(context) = (unsafe { (opaque as *mut WorkerContext).as_mut() }) else {
        return false;
    };
    let mut reset_ok = true;
    if !context.clap_plugin.is_null() {
        reset_ok = unsafe {
            hirari_clap_instance_reset(
                context.clap_instance,
                context.sample_rate,
                context.min_frames,
                context.max_frames,
            )
        } && reset_ok;
    }
    if !context.vst3_instance.is_null() {
        reset_ok = unsafe {
            hirari_worker_vst3_reset(
                context.vst3_instance,
                context.sample_rate,
                context.min_frames,
                context.max_frames,
            )
        } && reset_ok;
    }
    #[cfg(target_os = "macos")]
    if !context.au_instance.is_null() {
        reset_ok = unsafe {
            hirari_au_instance_reset(
                context.au_instance,
                context.sample_rate,
                context.min_frames,
                context.max_frames,
            )
        } && reset_ok;
    }
    reset_ok
}

extern "C" fn backend_state(opaque: *mut c_void, mode: u8) -> u16 {
    let Some(context) = (unsafe { (opaque as *mut WorkerContext).as_mut() }) else {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    };
    let view = &context.view;
    if !context.clap_state.is_null() && !context.clap_plugin.is_null() {
        return unsafe {
            hirari_clap_state_block(
                context.clap_plugin,
                context.clap_state,
                context.clap_input,
                mode,
                view.field(MAILBOX_STATE).cast::<u8>(),
                view.field(MAILBOX_STATE_SIZE).cast::<AtomicU32>(),
                view.field(MAILBOX_STATE_VERSION).cast::<AtomicU32>(),
                view.field(MAILBOX_STATE_CHECKSUM).cast::<AtomicU64>(),
                context.sample_rate,
                context.min_frames,
                context.max_frames,
            )
        };
    }
    if !context.vst3_instance.is_null() {
        return unsafe { hirari_worker_vst3_state(context.vst3_instance, context.shared, mode) };
    }
    #[cfg(target_os = "macos")]
    if !context.au_instance.is_null() {
        return unsafe {
            hirari_au_state_block(
                context.au_instance,
                mode,
                view.field(MAILBOX_STATE).cast::<u8>(),
                view.field(MAILBOX_STATE_SIZE),
                view.field(MAILBOX_STATE_VERSION),
                view.field(MAILBOX_STATE_CHECKSUM),
            )
        };
    }
    u16::from(STATE_ERROR_PLUGIN) << 8
}

unsafe fn clear_output(context: &WorkerContext) {
    let output = context.view.field(MAILBOX_OUTPUT).cast::<f32>();
    if !output.is_null() {
        unsafe { output.write_bytes(0, (MAX_CHANNELS * MAX_FRAMES) as usize) };
    }
}

extern "C" fn backend_process(
    opaque: *mut c_void,
    _shared: *mut c_void,
    channels: u32,
    frames: u32,
) -> bool {
    let Some(context) = (unsafe { (opaque as *mut WorkerContext).as_mut() }) else {
        return false;
    };
    if context.format == FORMAT_NONE {
        return true;
    }
    let input = context.view.field(MAILBOX_INPUT).cast::<f32>();
    let output = context.view.field(MAILBOX_OUTPUT).cast::<f32>();
    if input.is_null() || output.is_null() {
        return false;
    }
    let samples = (channels as usize).saturating_mul(frames as usize);
    unsafe { core::ptr::copy_nonoverlapping(input, output, samples) };
    let midi_events = context.view.field(MAILBOX_MIDI_EVENTS).cast::<AtomicU32>();
    let parameter_changes = context
        .view
        .field(MAILBOX_PARAMETER_CHANGES)
        .cast::<AtomicU32>();
    match context.format {
        FORMAT_CLAP => {
            let midi_count = unsafe { (*midi_events).load(Ordering::Acquire) };
            let parameter_count = unsafe { (*parameter_changes).load(Ordering::Acquire) };
            unsafe {
                hirari_clap_process_block(
                    context.clap_plugin,
                    context.clap_input,
                    context.view.field(MAILBOX_INPUT).cast::<f32>(),
                    context.view.field(MAILBOX_OUTPUT).cast::<f32>(),
                    channels,
                    frames,
                    context.view.field(MAILBOX_MIDI),
                    midi_count,
                    context.view.field(MAILBOX_EXTENDED_MIDI),
                    context.view.field(MAILBOX_PARAMETER_CHANGE),
                    parameter_count,
                )
            }
        }
        #[cfg(target_os = "macos")]
        FORMAT_AUDIO_UNIT => {
            let midi_count = unsafe { (*midi_events).load(Ordering::Acquire) };
            let parameter_count = unsafe { (*parameter_changes).load(Ordering::Acquire) };
            let processed = unsafe {
                hirari_au_process_block(
                    context.au_instance,
                    context.view.field(MAILBOX_INPUT).cast::<f32>(),
                    context.view.field(MAILBOX_OUTPUT).cast::<f32>(),
                    channels,
                    frames,
                    context.view.field(MAILBOX_MIDI),
                    midi_count,
                    context.view.field(MAILBOX_PARAMETER_CHANGE),
                    parameter_count,
                )
            };
            if !processed {
                unsafe { clear_output(context) };
            }
            processed
        }
        FORMAT_VST3 => {
            let processed = unsafe {
                hirari_worker_vst3_process(context.vst3_instance, context.shared, channels, frames)
            };
            if !processed {
                unsafe { clear_output(context) };
            }
            processed
        }
        _ => true,
    }
}

unsafe fn resolve_plugin_image(plugin_path: *const c_char) -> Option<CString> {
    let resolved = unsafe { hirari_plugin_resolve_image_path(plugin_path) };
    if resolved.is_null() {
        return None;
    }
    let image = unsafe { CStr::from_ptr(resolved) }.to_owned();
    unsafe { hirari_plugin_resolved_image_free(resolved) };
    Some(image)
}

/// Worker entry point. Returns the process exit status the launcher observes.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_worker_main(argc: i32, argv: *const *const c_char) -> i32 {
    if argc == 2 && !argv.is_null() {
        let argument = unsafe { *argv.add(1) };
        if !argument.is_null()
            && unsafe { CStr::from_ptr(argument) }.to_bytes() == b"--capabilities"
        {
            let capabilities = capabilities_json();
            unsafe {
                write(
                    1,
                    capabilities.as_ptr().cast::<c_void>(),
                    capabilities.len(),
                )
            };
            return 0;
        }
    }
    let mut options: WorkerLaunchOptions = unsafe { core::mem::zeroed() };
    let mut parse_error = 0_u32;
    if !unsafe { hirari_plugin_worker_parse_args(argc, argv, &mut options, &mut parse_error) } {
        let text = unsafe { CStr::from_ptr(hirari_plugin_worker_parse_error_text(parse_error)) };
        debug_log(&String::from_utf8_lossy(text.to_bytes()));
        return 2;
    }
    let control_fd = options.control_fd;
    let status_fd = options.status_fd;
    let plugin_path = options.plugin_path;
    let sample_rate = options.sample_rate;
    let min_frames = options.min_frames;
    let max_frames = options.max_frames;
    let channels = options.channels;

    if !hirari_plugin_worker_apply_limits() {
        unsafe { send_status(status_fd, ERROR_SECURITY) };
        return 11;
    }

    let mut mapping_error = 0_u32;
    let shared_bytes = unsafe { hirari_plugin_worker_shared_bytes() };
    let shared = unsafe {
        hirari_plugin_worker_map_shared(
            options.shared_fd,
            options.shared_name,
            shared_bytes,
            &mut mapping_error,
        )
    };
    if shared.is_null() {
        if mapping_error == 2 {
            debug_log("shared audio block mapping failed");
            return 5;
        }
        debug_log("shared audio block descriptor and name are unavailable");
        return 4;
    }
    let mut view = WorkerMailboxView {
        fields: [core::ptr::null_mut(); 18],
    };
    unsafe { hirari_plugin_worker_mailbox_view(shared, &mut view) };
    let protocol_version = view.field(MAILBOX_PROTOCOL_VERSION).cast::<AtomicU32>();
    if unsafe { (*protocol_version).load(Ordering::Acquire) } != AUDIO_BLOCK_PROTOCOL_VERSION {
        unsafe {
            send_status(status_fd, ERROR_ABI);
            hirari_plugin_worker_unmap_shared(shared, shared_bytes);
        }
        return 12;
    }
    unsafe {
        (*view.field(MAILBOX_ACTIVE_SAMPLE_RATE).cast::<AtomicU32>())
            .store(sample_rate as u32, Ordering::Release);
        (*view.field(MAILBOX_ACTIVE_CHANNELS).cast::<AtomicU32>())
            .store(channels, Ordering::Release);
    }

    let mut context = WorkerContext {
        format: FORMAT_NONE,
        shared,
        view,
        clap_instance: core::ptr::null_mut(),
        clap_plugin: core::ptr::null(),
        clap_state: core::ptr::null(),
        clap_input: core::ptr::null_mut(),
        au_instance: core::ptr::null_mut(),
        vst3_instance: core::ptr::null_mut(),
        sample_rate,
        min_frames,
        max_frames,
    };
    let mut midi_instrument = false;
    let mut plugin_handle: *mut c_void = core::ptr::null_mut();
    let mut image = CString::new("").unwrap();

    let formats = options.resolved_formats;
    if formats == WORKER_FORMAT_UNSUPPORTED {
        unsafe { send_status(status_fd, ERROR_UNSUPPORTED) };
        unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
        return 12;
    }
    let is_clap = formats & 1 != 0;
    let is_vst3 = formats & 2 != 0;
    let is_audio_unit = formats & 4 != 0;
    let builtin_passthrough = options.builtin_passthrough != 0;

    if is_vst3 {
        if !unsafe { hirari_worker_vst3_supported() } {
            unsafe { send_status(status_fd, ERROR_UNSUPPORTED) };
            unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
            return 12;
        }
        context.vst3_instance = unsafe {
            hirari_worker_vst3_load(plugin_path, sample_rate, max_frames, &mut midi_instrument)
        };
        if context.vst3_instance.is_null() {
            debug_log("VST3 load failed");
            unsafe { send_status(status_fd, ERROR_INSTANCE) };
            unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
            return 10;
        }
    }
    if is_audio_unit {
        #[cfg(target_os = "macos")]
        {
            context.au_instance = unsafe {
                hirari_au_instance_load(
                    plugin_path,
                    sample_rate,
                    max_frames,
                    channels,
                    &mut midi_instrument,
                )
            };
            if context.au_instance.is_null() {
                unsafe { send_status(status_fd, ERROR_INSTANCE) };
                unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
                return 10;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            unsafe { send_status(status_fd, ERROR_UNSUPPORTED) };
            unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
            return 12;
        }
    }
    let vst3_sdk_loaded = is_vst3 && unsafe { hirari_worker_vst3_supported() };
    if !(builtin_passthrough || is_audio_unit || vst3_sdk_loaded) {
        if let Some(resolved) = unsafe { resolve_plugin_image(plugin_path) } {
            image = resolved;
        }
    }
    debug_log(if image.as_bytes().is_empty() {
        "plugin image empty"
    } else {
        "plugin image resolved"
    });
    if !(builtin_passthrough || is_audio_unit || vst3_sdk_loaded || is_clap) {
        #[cfg(unix)]
        {
            let image_ptr = if image.as_bytes().is_empty() {
                core::ptr::null()
            } else {
                image.as_ptr()
            };
            if !image_ptr.is_null() {
                plugin_handle = unsafe { dlopen(image_ptr, 2 | 4) };
            }
        }
    } else {
        plugin_handle = 1_usize as *mut c_void;
    }
    if plugin_handle.is_null() {
        debug_log("dlopen failed");
        unsafe { send_status(status_fd, ERROR_LOAD) };
        unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
        return 6;
    }
    #[cfg(unix)]
    if !(builtin_passthrough || is_audio_unit || vst3_sdk_loaded || is_clap) {
        let symbol = if is_vst3 {
            c"GetPluginFactory".as_ptr()
        } else {
            core::ptr::null()
        };
        let resolved = symbol.is_null() || unsafe { dlsym(plugin_handle, symbol) }.is_null();
        if resolved {
            unsafe { send_status(status_fd, ERROR_ABI) };
            unsafe {
                dlclose(plugin_handle);
                hirari_plugin_worker_unmap_shared(shared, shared_bytes);
            }
            return 7;
        }
    }
    if is_clap && !builtin_passthrough {
        let load_result = unsafe {
            hirari_clap_instance_load(
                image.as_ptr(),
                plugin_path,
                sample_rate,
                min_frames,
                max_frames,
                &mut context.clap_instance,
                &mut midi_instrument,
            )
        };
        if load_result != 0 {
            let failure = hirari_plugin_worker_load_failure(load_result);
            unsafe { send_status(status_fd, (failure >> 8) as u8) };
            unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
            return (failure & 0xff) as i32;
        }
        context.clap_plugin = unsafe { hirari_clap_instance_plugin(context.clap_instance) };
        context.clap_state = unsafe { hirari_clap_instance_state(context.clap_instance) };
        if context.clap_plugin.is_null() {
            unsafe {
                hirari_clap_instance_destroy(context.clap_instance);
                hirari_plugin_worker_unmap_shared(shared, shared_bytes);
            }
            return 10;
        }
    }

    if !context.clap_plugin.is_null() {
        context.clap_input = unsafe {
            hirari_clap_input_context_create(
                context.view.field(MAILBOX_OUTPUT_MIDI_EVENTS),
                context.view.field(MAILBOX_OUTPUT_MIDI_DROPPED),
                context.view.field(MAILBOX_OUTPUT_MIDI),
            )
        };
        if context.clap_input.is_null() {
            unsafe {
                send_status(status_fd, ERROR_INSTANCE);
                hirari_clap_instance_destroy(context.clap_instance);
                hirari_plugin_worker_unmap_shared(shared, shared_bytes);
            }
            return 10;
        }
    }
    context.format = if !context.clap_plugin.is_null() {
        FORMAT_CLAP
    } else if !context.au_instance.is_null() {
        FORMAT_AUDIO_UNIT
    } else if !context.vst3_instance.is_null() {
        FORMAT_VST3
    } else {
        FORMAT_NONE
    };
    unsafe {
        (*context
            .view
            .field(MAILBOX_MIDI_INSTRUMENT)
            .cast::<AtomicU8>())
        .store(u8::from(midi_instrument), Ordering::Release);
    }
    debug_log("sending ready");
    if !unsafe { send_status(status_fd, STATUS_READY) } {
        unsafe { destroy_worker_resources(&mut context, plugin_handle) };
        unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
        return 3;
    }

    let backend = WorkerBackend {
        reset: Some(backend_reset),
        state: Some(backend_state),
        process: Some(backend_process),
    };
    let faults = WorkerFaultConfig {
        crash_after_blocks: options.crash_after_blocks,
        delay_milliseconds: options.delay_milliseconds,
    };
    let _result = unsafe {
        hirari_plugin_worker_run(
            control_fd,
            shared,
            &mut context as *mut WorkerContext as *mut c_void,
            &backend,
            &faults,
        )
    };
    unsafe { destroy_worker_resources(&mut context, plugin_handle) };
    unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
    0
}

unsafe fn destroy_worker_resources(context: &mut WorkerContext, plugin_handle: *mut c_void) {
    unsafe {
        hirari_clap_input_context_destroy(context.clap_input);
        context.clap_input = core::ptr::null_mut();
        if !context.clap_instance.is_null() {
            hirari_clap_instance_destroy(context.clap_instance);
            context.clap_instance = core::ptr::null_mut();
        }
        if !context.vst3_instance.is_null() {
            hirari_worker_vst3_destroy(context.vst3_instance);
            context.vst3_instance = core::ptr::null_mut();
        }
        #[cfg(target_os = "macos")]
        if !context.au_instance.is_null() {
            hirari_au_instance_destroy(context.au_instance);
            context.au_instance = core::ptr::null_mut();
        }
        #[cfg(unix)]
        if !plugin_handle.is_null() && plugin_handle != 1_usize as *mut c_void {
            dlclose(plugin_handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_report_the_compiled_formats() {
        let json = capabilities_json();
        assert!(json.starts_with("{\"clap\":true"));
        assert!(json.ends_with("}\n"));
    }

    #[test]
    fn uninitialized_context_marks_no_adapter() {
        let context = WorkerContext {
            format: FORMAT_NONE,
            shared: core::ptr::null_mut(),
            view: WorkerMailboxView {
                fields: [core::ptr::null_mut(); 18],
            },
            clap_instance: core::ptr::null_mut(),
            clap_plugin: core::ptr::null(),
            clap_state: core::ptr::null(),
            clap_input: core::ptr::null_mut(),
            au_instance: core::ptr::null_mut(),
            vst3_instance: core::ptr::null_mut(),
            sample_rate: 48_000.0,
            min_frames: 1,
            max_frames: MAX_FRAMES,
        };
        assert_eq!(context.format, FORMAT_NONE);
    }
}
