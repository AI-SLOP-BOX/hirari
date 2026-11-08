//! Isolated worker executable startup.
//!
//! The CLAP loader, AudioUnit host, mailbox mapping and control loop all live
//! in this crate, and the mailbox layout itself is in `mailbox`, so the worker
//! process is Rust from `main` down. VST3 is compiled from the official SDK
//! adapter only when `HIRARI_VST3_SDK` is set; without it, a VST3 request
//! fails closed here rather than reaching a format adapter.

use core::ffi::{c_char, c_void, CStr};
use core::sync::atomic::Ordering;
use std::ffi::CString;

use crate::mailbox::{self, SharedAudioBlock};

#[cfg(target_os = "macos")]
use crate::au_host::{
    hirari_au_instance_destroy, hirari_au_instance_load, hirari_au_instance_reset,
    hirari_au_process_block, hirari_au_state_block,
};
#[cfg(vst3_sdk)]
extern "C" {
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
        opaque: *mut c_void,
        channels: u32,
        frames: u32,
    ) -> bool;
    fn hirari_worker_vst3_state(instance: *mut c_void, opaque: *mut c_void, mode: u8) -> u16;
}
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
const STATE_ERROR_PLUGIN: u8 = 4;
const MAX_CHANNELS: u32 = 2;
const MAX_FRAMES: u32 = 8192;
const _: () = assert!(MAX_CHANNELS as usize == mailbox::MAX_CHANNELS);
const _: () = assert!(MAX_FRAMES as usize == mailbox::MAX_FRAMES);

const FORMAT_NONE: u8 = 0;
const FORMAT_CLAP: u8 = 1;
const FORMAT_AUDIO_UNIT: u8 = 2;
const FORMAT_VST3: u8 = 3;

unsafe extern "C" {
    fn write(fd: i32, buffer: *const c_void, count: usize) -> isize;
}

#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
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
    if cfg!(vst3_sdk) {
        if cfg!(target_os = "macos") {
            "{\"clap\":true,\"au\":true,\"vst3\":true}\n"
        } else {
            "{\"clap\":true,\"au\":false,\"vst3\":true}\n"
        }
    } else if cfg!(target_os = "macos") {
        "{\"clap\":true,\"au\":true,\"vst3\":false}\n"
    } else {
        "{\"clap\":true,\"au\":false,\"vst3\":false}\n"
    }
}

struct WorkerContext {
    format: u8,
    mailbox: *mut SharedAudioBlock,
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

impl WorkerContext {
    /// The mailbox is mapped for the whole lifetime of the worker, so every
    /// backend callback may address it directly.
    fn mailbox(&self) -> &mut SharedAudioBlock {
        unsafe { &mut *self.mailbox }
    }
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
    #[cfg(vst3_sdk)]
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
    reset_ok
}

extern "C" fn backend_state(opaque: *mut c_void, mode: u8) -> u16 {
    let Some(context) = (unsafe { (opaque as *mut WorkerContext).as_mut() }) else {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    };
    if context.mailbox.is_null() {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    }
    let block = context.mailbox();
    if !context.clap_state.is_null() && !context.clap_plugin.is_null() {
        return unsafe {
            hirari_clap_state_block(
                context.clap_plugin,
                context.clap_state,
                context.clap_input,
                mode,
                block.state.as_mut_ptr(),
                core::ptr::addr_of_mut!(block.state_size),
                core::ptr::addr_of_mut!(block.state_version),
                core::ptr::addr_of_mut!(block.state_checksum),
                context.sample_rate,
                context.min_frames,
                context.max_frames,
            )
        };
    }
    #[cfg(target_os = "macos")]
    if !context.au_instance.is_null() {
        return unsafe {
            hirari_au_state_block(
                context.au_instance,
                mode,
                block.state.as_mut_ptr(),
                core::ptr::from_mut(&mut block.state_size).cast::<c_void>(),
                core::ptr::from_mut(&mut block.state_version).cast::<c_void>(),
                core::ptr::from_mut(&mut block.state_checksum).cast::<c_void>(),
            )
        };
    }
    #[cfg(vst3_sdk)]
    if !context.vst3_instance.is_null() {
        return unsafe {
            hirari_worker_vst3_state(context.vst3_instance, block as *mut _ as *mut c_void, mode)
        };
    }
    u16::from(STATE_ERROR_PLUGIN) << 8
}

unsafe fn clear_output(context: &WorkerContext) {
    let output = context.mailbox().output.samples.as_mut_ptr().cast::<f32>();
    unsafe { output.write_bytes(0, MAX_CHANNELS as usize * MAX_FRAMES as usize) };
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
    if context.format == FORMAT_NONE || context.mailbox.is_null() {
        return context.format == FORMAT_NONE;
    }
    let block = context.mailbox();
    let input = block.input.samples.as_mut_ptr().cast::<f32>();
    let output = block.output.samples.as_mut_ptr().cast::<f32>();
    let samples = (channels as usize).saturating_mul(frames as usize);
    unsafe { core::ptr::copy_nonoverlapping(input, output, samples) };
    let midi_count = block.midi_events.load(Ordering::Acquire);
    let parameter_count = block.parameter_changes.load(Ordering::Acquire);
    match context.format {
        FORMAT_CLAP => unsafe {
            hirari_clap_process_block(
                context.clap_plugin,
                context.clap_input,
                input,
                output,
                channels,
                frames,
                core::ptr::from_mut(&mut block.midi).cast::<c_void>(),
                midi_count,
                core::ptr::from_mut(&mut block.extended_midi).cast::<c_void>(),
                core::ptr::from_mut(&mut block.parameter_change).cast::<c_void>(),
                parameter_count,
            )
        },
        #[cfg(target_os = "macos")]
        FORMAT_AUDIO_UNIT => {
            let processed = unsafe {
                hirari_au_process_block(
                    context.au_instance,
                    input,
                    output,
                    channels,
                    frames,
                    core::ptr::from_mut(&mut block.midi).cast::<c_void>(),
                    midi_count,
                    core::ptr::from_mut(&mut block.parameter_change).cast::<c_void>(),
                    parameter_count,
                )
            };
            if !processed {
                unsafe { clear_output(context) };
            }
            processed
        }
        #[cfg(vst3_sdk)]
        FORMAT_VST3 => unsafe {
            hirari_worker_vst3_process(
                context.vst3_instance,
                context.mailbox as *mut c_void,
                channels,
                frames,
            )
        },
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
pub unsafe fn worker_entry(argc: i32, argv: *const *const c_char) -> i32 {
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
    let shared_bytes = core::mem::size_of::<SharedAudioBlock>();
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
    let mailbox = shared.cast::<SharedAudioBlock>();
    if unsafe { (*mailbox).protocol_version.load(Ordering::Acquire) }
        != mailbox::AUDIO_BLOCK_PROTOCOL_VERSION
    {
        unsafe {
            send_status(status_fd, ERROR_ABI);
            hirari_plugin_worker_unmap_shared(shared, shared_bytes);
        }
        return 12;
    }
    unsafe {
        (*mailbox)
            .active_sample_rate
            .store(sample_rate as u32, Ordering::Release);
        (*mailbox)
            .active_channels
            .store(channels, Ordering::Release);
    }

    let mut context = WorkerContext {
        format: FORMAT_NONE,
        mailbox,
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

    // VST3 hosting needs the SDK-built adapter; without it a VST3 request
    // fails closed with the same status byte the unsupported-SDK path
    // reported.
    #[cfg(not(vst3_sdk))]
    if formats & 2 != 0 {
        debug_log("VST3 hosting is not available in this worker");
        unsafe { send_status(status_fd, ERROR_UNSUPPORTED) };
        unsafe { hirari_plugin_worker_unmap_shared(shared, shared_bytes) };
        return 12;
    }
    #[cfg(vst3_sdk)]
    if formats & 2 != 0 {
        context.vst3_instance = unsafe {
            hirari_worker_vst3_load(plugin_path, sample_rate, max_frames, &mut midi_instrument)
        };
        if context.vst3_instance.is_null() {
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
    if !(builtin_passthrough || is_audio_unit) {
        if let Some(resolved) = unsafe { resolve_plugin_image(plugin_path) } {
            image = resolved;
        }
    }
    debug_log(if image.as_bytes().is_empty() {
        "plugin image empty"
    } else {
        "plugin image resolved"
    });
    if !(builtin_passthrough || is_audio_unit || is_clap || is_vst3) {
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
    if !(builtin_passthrough || is_audio_unit || is_clap || is_vst3) {
        // No Rust-owned adapter claims this binary outside the SDK-built
        // VST3 path, so an unknown image is always an ABI failure.
        unsafe { send_status(status_fd, ERROR_ABI) };
        unsafe {
            dlclose(plugin_handle);
            hirari_plugin_worker_unmap_shared(shared, shared_bytes);
        }
        return 7;
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
        let mailbox = context.mailbox;
        context.clap_input = unsafe {
            hirari_clap_input_context_create(
                core::ptr::from_mut(&mut (*mailbox).output_midi_events).cast::<c_void>(),
                core::ptr::from_mut(&mut (*mailbox).output_midi_dropped).cast::<c_void>(),
                core::ptr::from_mut(&mut (*mailbox).output_midi).cast::<c_void>(),
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
        (*context.mailbox)
            .midi_instrument
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
        #[cfg(target_os = "macos")]
        if !context.au_instance.is_null() {
            hirari_au_instance_destroy(context.au_instance);
            context.au_instance = core::ptr::null_mut();
        }
        #[cfg(vst3_sdk)]
        if !context.vst3_instance.is_null() {
            hirari_worker_vst3_destroy(context.vst3_instance);
            context.vst3_instance = core::ptr::null_mut();
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
        #[cfg(vst3_sdk)]
        assert!(json.contains("\"vst3\":true"));
        #[cfg(not(vst3_sdk))]
        assert!(json.contains("\"vst3\":false"));
    }

    fn unloading_context(format: u8) -> WorkerContext {
        WorkerContext {
            format,
            mailbox: core::ptr::null_mut(),
            clap_instance: core::ptr::null_mut(),
            clap_plugin: core::ptr::null(),
            clap_state: core::ptr::null(),
            clap_input: core::ptr::null_mut(),
            au_instance: core::ptr::null_mut(),
            vst3_instance: core::ptr::null_mut(),
            sample_rate: 48_000.0,
            min_frames: 1,
            max_frames: MAX_FRAMES,
        }
    }

    #[test]
    fn an_unmapped_mailbox_is_a_process_failure_not_a_crash() {
        let mut context = unloading_context(FORMAT_CLAP);
        let processed = backend_process(
            (&mut context as *mut WorkerContext).cast::<c_void>(),
            core::ptr::null_mut(),
            1,
            1,
        );
        assert!(!processed);
    }

    #[test]
    fn no_adapter_reports_a_successful_empty_block() {
        let mut context = unloading_context(FORMAT_NONE);
        assert!(backend_state((&mut context as *mut WorkerContext).cast::<c_void>(), 1) != 0);
    }
}
