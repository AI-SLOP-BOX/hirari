//! Platform-split audio driver host behind one flat C++-facing surface.
//!
//! The C++ engine used to keep two adapter classes for this boundary:
//! `MacAudioDriverHost`, a null-checking forwarder to the Rust CoreAudio host,
//! and `UnavailableAudioDriver`, the off-macOS silent fallback with its own
//! callback dispatcher. Both are gone. The lifecycle defaults, the bounded
//! input-queue capacity and the driver's diagnostic sink live here, so C++
//! calls one set of functions and the platform split sits beside
//! [`crate::audio_driver_view`] instead of inside the engine class.

use std::ffi::{c_char, c_void};

/// The engine's realtime block callback. The macOS host takes this shape from
/// `mac_audio_driver`; off macOS that module is not compiled, so the boundary is
/// named here and both sides use the same function pointer type.
pub type ProcessCallback = unsafe extern "C" fn(
    inputs: *const *const f32,
    input_channel_count: u32,
    outputs: *const *mut f32,
    output_channel_count: u32,
    frames: u32,
    context: *mut c_void,
);

/// One consumer's worth of interleaved input, plus what the device queue had to
/// throw away getting here.
#[derive(Default)]
pub struct InputBlock {
    pub interleaved: Vec<f32>,
    pub channel_count: u32,
    pub dropped_blocks: u64,
}

#[cfg(target_os = "macos")]
mod device {
    use std::ffi::{CStr, c_char, c_void};
    use std::ptr;

    use super::InputBlock;
    use crate::mac_audio_driver as mac;
    use crate::mac_audio_driver::{BlockInfo, ProcessCallback};

    /// Component id the driver's device-side diagnostics are filed under, the
    /// one the deleted C++ log trampoline used.
    const DRIVER_LOG_COMPONENT: u32 = 0xA001;
    /// The channel ceiling of the Rust-owned SPSC input queue.
    const INPUT_QUEUE_CHANNELS: usize = 32;
    /// The frame ceiling of the same queue, and the stride of every plane.
    const INPUT_QUEUE_FRAMES: u32 = mac::MAX_FRAMES_PER_SLICE;

    /// Planar scratch reused between polls. Holding it behind a lock is what
    /// keeps the queue's single-consumer rule true even if two capture workers
    /// ask for a block at the same time.
    static POLL_SCRATCH: std::sync::Mutex<Vec<f32>> = std::sync::Mutex::new(Vec::new());

    /// The driver's device-side diagnostics land in the Rust log ring directly;
    /// the C++ adapter used to trampolines them across.
    unsafe extern "C" fn post_driver_log(
        _context: *mut c_void,
        level: u32,
        message: *const c_char,
    ) {
        if message.is_null() {
            return;
        }
        let bytes = unsafe { CStr::from_ptr(message) }.to_bytes();
        unsafe {
            crate::log_buffer::hirari_log_buffer_post(
                crate::log_buffer::hirari_log_buffer_default_state(),
                level,
                DRIVER_LOG_COMPONENT,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
    }

    pub fn create(process: Option<ProcessCallback>, context: *mut c_void) -> *mut c_void {
        mac::hirari_mac_audio_host_create(process, context, Some(post_driver_log), ptr::null_mut())
    }

    /// # Safety
    /// `host` is the handle `create` returned and is not used again afterwards.
    pub unsafe fn destroy(host: *mut c_void) {
        unsafe { mac::hirari_mac_audio_host_destroy(host) };
    }

    pub fn start(host: *mut c_void) -> bool {
        mac::hirari_mac_audio_host_start(host)
    }

    pub fn stop(host: *mut c_void) {
        mac::hirari_mac_audio_host_stop(host);
    }

    pub fn is_running(host: *mut c_void) -> bool {
        mac::hirari_mac_audio_host_is_running(host)
    }

    pub fn sample_rate(host: *mut c_void) -> f64 {
        mac::hirari_mac_audio_host_sample_rate(host)
    }

    pub fn buffer_size(host: *mut c_void) -> u32 {
        mac::hirari_mac_audio_host_buffer_size(host)
    }

    pub fn status(host: *mut c_void) -> *const c_char {
        mac::hirari_mac_audio_host_status(host)
    }

    pub fn last_error_code(host: *mut c_void) -> i32 {
        mac::hirari_mac_audio_host_last_error_code(host)
    }

    pub fn try_reconnect(host: *mut c_void) {
        mac::hirari_mac_audio_host_try_reconnect(host);
    }

    pub fn reconfigure(host: *mut c_void, sample_rate: f64, buffer_size: u32) -> bool {
        mac::hirari_mac_audio_host_reconfigure(host, sample_rate, buffer_size)
    }

    pub fn select_device(
        host: *mut c_void,
        device: u32,
        sample_rate: f64,
        buffer_size: u32,
    ) -> bool {
        mac::hirari_mac_audio_host_select_device(host, device, sample_rate, buffer_size)
    }

    /// A real device drives the process callback itself, so the engine's own
    /// block entry never dispatches through here.
    pub fn dispatch_process(
        _host: *mut c_void,
        _left: *mut f32,
        _right: *mut f32,
        _frames: u32,
    ) -> bool {
        false
    }

    pub fn poll_input(host: *mut c_void) -> InputBlock {
        let channels =
            (mac::hirari_mac_audio_host_input_channels(host) as usize).min(INPUT_QUEUE_CHANNELS);
        if channels == 0 {
            return InputBlock::default();
        }
        let mut scratch = POLL_SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
        let frame_stride = INPUT_QUEUE_FRAMES as usize;
        if scratch.len() < channels * frame_stride {
            scratch.resize(channels * frame_stride, 0.0);
        }
        let base = scratch.as_mut_ptr();
        let mut planes = [ptr::null_mut::<f32>(); INPUT_QUEUE_CHANNELS];
        for (channel, plane) in planes.iter_mut().take(channels).enumerate() {
            *plane = base.wrapping_add(channel * frame_stride);
        }
        let mut info = BlockInfo::default();
        let mut dropped = 0u64;
        let polled = unsafe {
            mac::hirari_mac_audio_host_poll_input_block(
                host,
                planes.as_ptr(),
                channels as u32,
                INPUT_QUEUE_FRAMES,
                &mut info,
                &mut dropped,
            )
        };
        if !polled {
            return InputBlock {
                dropped_blocks: dropped,
                ..InputBlock::default()
            };
        }
        InputBlock {
            interleaved: crate::runtime_views::interleave_planar_audio_input(
                &scratch,
                info.channel_count,
                info.frame_count,
                INPUT_QUEUE_FRAMES,
            ),
            channel_count: info.channel_count,
            dropped_blocks: dropped,
        }
    }

    pub fn discard_pending_input(host: *mut c_void) {
        mac::hirari_mac_audio_host_discard_pending_input(host);
    }
}

#[cfg(not(target_os = "macos"))]
mod device {
    use std::ffi::{c_char, c_void};
    use std::ptr;

    use super::{InputBlock, ProcessCallback};

    /// The format an engine with no device to negotiate with reports, the same
    /// pair the macOS host answers for an absent handle.
    const FALLBACK_SAMPLE_RATE: f64 = 44_100.0;
    const FALLBACK_BUFFER_SIZE: u32 = 512;

    /// The host this platform has: a callback it never gets to run, because no
    /// device exists to drive it. Kept so the engine's lifecycle calls have the
    /// same shape here as on macOS.
    struct SilentHost {
        process: Option<ProcessCallback>,
        context: *mut c_void,
    }

    pub fn create(process: Option<ProcessCallback>, context: *mut c_void) -> *mut c_void {
        Box::into_raw(Box::new(SilentHost { process, context })).cast()
    }

    /// # Safety
    /// `host` is the handle `create` returned and is not used again afterwards.
    pub unsafe fn destroy(host: *mut c_void) {
        if !host.is_null() {
            drop(unsafe { Box::from_raw(host.cast::<SilentHost>()) });
        }
    }

    /// Nothing can start here; the engine reports the failure and keeps working
    /// offline, exactly as the deleted C++ fallback did.
    pub fn start(_host: *mut c_void) -> bool {
        false
    }

    pub fn stop(_host: *mut c_void) {}

    pub fn is_running(_host: *mut c_void) -> bool {
        false
    }

    pub fn sample_rate(_host: *mut c_void) -> f64 {
        FALLBACK_SAMPLE_RATE
    }

    pub fn buffer_size(_host: *mut c_void) -> u32 {
        FALLBACK_BUFFER_SIZE
    }

    pub fn status(_host: *mut c_void) -> *const c_char {
        b"silent-fallback\0".as_ptr().cast()
    }

    pub fn last_error_code(_host: *mut c_void) -> i32 {
        0
    }

    pub fn try_reconnect(_host: *mut c_void) {}

    pub fn reconfigure(_host: *mut c_void, _sample_rate: f64, _buffer_size: u32) -> bool {
        false
    }

    pub fn select_device(
        _host: *mut c_void,
        _device: u32,
        _sample_rate: f64,
        _buffer_size: u32,
    ) -> bool {
        false
    }

    /// The offline path still owns one callback boundary: a caller that hands
    /// this host a bounded stereo block gets the engine's own process callback
    /// invoked for it, which is how the non-hardware build renders at all. It
    /// never claims a running device.
    pub fn dispatch_process(host: *mut c_void, left: *mut f32, right: *mut f32, frames: u32) -> bool {
        let Some(silent) = (unsafe { host.cast::<SilentHost>().as_ref() }) else {
            return false;
        };
        if left.is_null() || right.is_null() || frames == 0 {
            return false;
        }
        let Some(process) = silent.process else {
            return false;
        };
        let outputs = [left, right];
        unsafe {
            process(
                ptr::null(),
                0,
                outputs.as_ptr(),
                outputs.len() as u32,
                frames,
                silent.context,
            )
        };
        true
    }

    pub fn poll_input(_host: *mut c_void) -> InputBlock {
        InputBlock::default()
    }

    pub fn discard_pending_input(_host: *mut c_void) {}
}

/// The engine's whole driver-host surface. Each entry answers for a null host
/// exactly as the deleted C++ guards did.
#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_create(
    process: Option<ProcessCallback>,
    context: *mut c_void,
) -> *mut c_void {
    device::create(process, context)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_driver_host_destroy(host: *mut c_void) {
    // SAFETY: the engine destroys its own host once, after stopping it.
    unsafe { device::destroy(host) }
}

/// Starting the device is the engine's creation policy, so it lives here: the
/// request is declined for a control-plane client, an offline session or an
/// isolated native run, and a start that fails on real hardware is published as
/// a Warning notification instead of being left for the caller to report.
#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_start_at_creation(
    host: *mut c_void,
    requested: bool,
) -> bool {
    if !requested
        || crate::audio_device_mode::hirari_audio_device_mode_offline()
        || crate::audio_device_mode::hirari_native_test_isolation()
    {
        return false;
    }
    if device::start(host) {
        return true;
    }
    report_startup_failure(host);
    false
}

/// The driver's own status string and platform error code, as one notification.
fn report_startup_failure(host: *mut c_void) {
    let status = unsafe {
        std::ffi::CStr::from_ptr(device::status(host))
            .to_string_lossy()
            .into_owned()
    };
    let text = format!(
        "Audio device startup failed: status={} error={}",
        status,
        device::last_error_code(host)
    );
    // The notification is diagnostic: a full queue must not fail the engine
    // that is starting anyway.
    let _ = unsafe {
        crate::status_queue::hirari_status_queue_push_default(
            crate::status_queue::SEVERITY_WARNING,
            text.as_ptr(),
            text.len(),
        )
    };
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_stop(host: *mut c_void) {
    device::stop(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_is_running(host: *mut c_void) -> bool {
    device::is_running(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_sample_rate(host: *mut c_void) -> f64 {
    device::sample_rate(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_buffer_size(host: *mut c_void) -> u32 {
    device::buffer_size(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_status(host: *mut c_void) -> *const c_char {
    // Every status is a static string, so the pointer outlives the call.
    device::status(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_last_error_code(host: *mut c_void) -> i32 {
    device::last_error_code(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_try_reconnect(host: *mut c_void) {
    device::try_reconnect(host)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_reconfigure(
    host: *mut c_void,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    device::reconfigure(host, sample_rate, buffer_size)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_select_device(
    host: *mut c_void,
    device_id: u32,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    device::select_device(host, device_id, sample_rate, buffer_size)
}

#[no_mangle]
pub extern "C" fn hirari_audio_driver_host_dispatch_process(
    host: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) -> bool {
    device::dispatch_process(host, left, right, frames)
}

/// Polls one non-realtime input block for the capture worker. Rust owns the
/// planar scratch, the queue's capacity clamp and the interleaving, so nothing
/// between the device queue and the recording session is C++ state.
pub fn poll_input(host: usize) -> InputBlock {
    device::poll_input(host as *mut c_void)
}

pub fn discard_pending_input(host: usize) {
    device::discard_pending_input(host as *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DISPATCHED_BLOCKS: AtomicUsize = AtomicUsize::new(0);
    static DISPATCHED_FRAMES: AtomicUsize = AtomicUsize::new(0);

    /// The engine's own block callback, as the C++ side registers it: it writes
    /// a sentinel into every frame it is handed.
    unsafe extern "C" fn count_block(
        _inputs: *const *const f32,
        _input_channels: u32,
        outputs: *const *mut f32,
        output_channels: u32,
        frames: u32,
        _context: *mut c_void,
    ) {
        DISPATCHED_BLOCKS.fetch_add(1, Ordering::Release);
        DISPATCHED_FRAMES.fetch_add(frames as usize, Ordering::Release);
        if outputs.is_null() {
            return;
        }
        for channel in 0..output_channels as usize {
            let plane = unsafe { *outputs.add(channel) };
            if plane.is_null() {
                continue;
            }
            for frame in 0..frames as usize {
                unsafe { *plane.add(frame) = 0.5 };
            }
        }
    }

    fn status_str(host: *mut c_void) -> String {
        let pointer = hirari_audio_driver_host_status(host);
        assert!(!pointer.is_null());
        unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn an_absent_host_answers_the_fallback_format_and_never_runs() {
        let null = ptr::null_mut();
        assert!(!device::start(null));
        assert!(!hirari_audio_driver_host_is_running(null));
        assert_eq!(hirari_audio_driver_host_sample_rate(null), 44_100.0);
        assert_eq!(hirari_audio_driver_host_buffer_size(null), 512);
        assert_eq!(hirari_audio_driver_host_last_error_code(null), 0);
        assert_ne!(status_str(null), "running");
        assert!(!hirari_audio_driver_host_reconfigure(null, 48_000.0, 256));
        assert!(!hirari_audio_driver_host_select_device(null, 3, 48_000.0, 256));
        hirari_audio_driver_host_stop(null);
        hirari_audio_driver_host_try_reconnect(null);
        let mut left = [0.0_f32; 4];
        let mut right = [0.0_f32; 4];
        DISPATCHED_BLOCKS.store(0, Ordering::Release);
        assert!(!hirari_audio_driver_host_dispatch_process(
            null,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ));
        assert_eq!(DISPATCHED_BLOCKS.load(Ordering::Acquire), 0);
        let block = poll_input(0);
        assert!(block.interleaved.is_empty());
        assert_eq!((block.channel_count, block.dropped_blocks), (0, 0));
        discard_pending_input(0);
    }

    #[test]
    fn a_constructed_host_is_stopped_and_holds_no_input_queue() {
        DISPATCHED_BLOCKS.store(0, Ordering::Release);
        let host = hirari_audio_driver_host_create(Some(count_block), ptr::null_mut());
        assert!(!host.is_null());
        // Construction never attaches a device, so nothing has rendered and the
        // capture worker has no queued block to read.
        assert!(!hirari_audio_driver_host_is_running(host));
        assert_eq!(DISPATCHED_BLOCKS.load(Ordering::Acquire), 0);
        let block = poll_input(host as usize);
        assert!(block.interleaved.is_empty());
        assert_eq!((block.channel_count, block.dropped_blocks), (0, 0));
        discard_pending_input(host as usize);
        // SAFETY: this is the host created above and it is not used again.
        unsafe { hirari_audio_driver_host_destroy(host) };
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_coreaudio_host_never_dispatches_the_block_the_render_thread_owns() {
        DISPATCHED_BLOCKS.store(0, Ordering::Release);
        let host = hirari_audio_driver_host_create(Some(count_block), ptr::null_mut());
        let mut left = [0.0_f32; 8];
        let mut right = [0.0_f32; 8];
        // A real device drives the callback itself; dispatching here would
        // render the graph a second time for one block.
        assert!(!hirari_audio_driver_host_dispatch_process(
            host,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            8
        ));
        assert_eq!(DISPATCHED_BLOCKS.load(Ordering::Acquire), 0);
        assert_eq!(left, [0.0_f32; 8]);
        // SAFETY: the host is this test's own and is not used after this.
        unsafe { hirari_audio_driver_host_destroy(host) };
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_silent_host_reports_no_device_and_dispatches_only_a_supplied_block() {
        DISPATCHED_BLOCKS.store(0, Ordering::Release);
        DISPATCHED_FRAMES.store(0, Ordering::Release);
        let host = hirari_audio_driver_host_create(Some(count_block), ptr::null_mut());
        assert!(!device::start(host));
        assert!(!hirari_audio_driver_host_is_running(host));
        assert_eq!(hirari_audio_driver_host_sample_rate(host), 44_100.0);
        assert_eq!(hirari_audio_driver_host_buffer_size(host), 512);
        assert_eq!(status_str(host), "silent-fallback");
        assert!(!hirari_audio_driver_host_reconfigure(host, 48_000.0, 256));
        assert!(!hirari_audio_driver_host_select_device(host, 1, 48_000.0, 256));
        assert!(poll_input(host as usize).interleaved.is_empty());
        hirari_audio_driver_host_stop(host);
        hirari_audio_driver_host_try_reconnect(host);

        let mut left = [0.0_f32; 6];
        let mut right = [0.0_f32; 6];
        // The offline build keeps one callback boundary: a caller that supplies
        // a bounded stereo block gets the engine's render invoked for it.
        assert!(hirari_audio_driver_host_dispatch_process(
            host,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            6
        ));
        assert_eq!(DISPATCHED_BLOCKS.load(Ordering::Acquire), 1);
        assert_eq!(DISPATCHED_FRAMES.load(Ordering::Acquire), 6);
        assert_eq!(left, [0.5_f32; 6]);
        assert_eq!(right, [0.5_f32; 6]);

        // A block the caller cannot describe is refused without rendering.
        assert!(!hirari_audio_driver_host_dispatch_process(
            host,
            ptr::null_mut(),
            right.as_mut_ptr(),
            6
        ));
        assert!(!hirari_audio_driver_host_dispatch_process(
            host,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            0
        ));
        assert_eq!(DISPATCHED_BLOCKS.load(Ordering::Acquire), 1);

        // SAFETY: the host is this test's own and is not used after this.
        unsafe { hirari_audio_driver_host_destroy(host) };
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_silent_host_without_a_callback_has_nothing_to_dispatch() {
        let host = hirari_audio_driver_host_create(None, ptr::null_mut());
        let mut left = [0.0_f32; 4];
        let mut right = [0.0_f32; 4];
        assert!(!hirari_audio_driver_host_dispatch_process(
            host,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ));
        assert_eq!(left, [0.0_f32; 4]);
        // SAFETY: the host is this test's own and is not used after this.
        unsafe { hirari_audio_driver_host_destroy(host) };
    }

    #[test]
    fn an_unrequested_creation_start_claims_nothing_and_reports_nothing() {
        let _serial = crate::status_queue::default_queue_test_lock();
        drain_status();
        assert!(!hirari_audio_driver_host_start_at_creation(ptr::null_mut(), false));
        assert!(take_status().is_none());
    }

    #[test]
    fn a_failed_device_start_is_published_as_a_warning_with_the_driver_diagnosis() {
        let _serial = crate::status_queue::default_queue_test_lock();
        drain_status();
        report_startup_failure(ptr::null_mut());
        let message = take_status().expect("the failure is published");
        assert_eq!(message.severity, crate::status_queue::SEVERITY_WARNING);
        let end = message
            .text
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(message.text.len());
        let text = String::from_utf8_lossy(&message.text[..end]).into_owned();
        assert!(text.starts_with("Audio device startup failed: status="), "{text}");
        assert!(text.contains(" error="), "{text}");
    }

    fn take_status() -> Option<crate::status_queue::StatusMessage> {
        let mut message = crate::status_queue::StatusMessage::default();
        // SAFETY: the message is this test's own storage.
        unsafe { crate::status_queue::hirari_status_queue_pop_default(&mut message) }
            .then_some(message)
    }

    fn drain_status() {
        while take_status().is_some() {}
    }
}
