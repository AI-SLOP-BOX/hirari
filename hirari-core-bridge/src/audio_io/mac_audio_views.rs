//! Windows/status accessors, split out of mac_audio_driver.rs.
use super::*;

pub fn view_status(handle: usize) -> &'static str {
    host_ref(handle as *mut c_void).map_or(status_name(STATE_UNAVAILABLE), |host| {
        status_name(host.state.load(Ordering::Acquire))
    })
}

pub fn view_is_running(handle: usize) -> bool {
    host_ref(handle as *mut c_void).is_some_and(MacAudioHost::is_running)
}

pub fn view_error_code(handle: usize) -> i32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::last_error_code)
}

pub fn view_output_peak(handle: usize) -> f32 {
    host_ref(handle as *mut c_void).map_or(0.0, MacAudioHost::output_peak)
}

pub fn view_callback_count(handle: usize) -> u64 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::callback_count)
}

pub fn view_io_latency_secs(handle: usize) -> f64 {
    host_ref(handle as *mut c_void).map_or(0.0, MacAudioHost::io_latency_secs)
}

pub fn view_input_channel_count(handle: usize) -> u32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::input_channel_count)
}

pub fn view_output_channel_count(handle: usize) -> u32 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::output_channel_count)
}

pub fn view_dropped_input_blocks(handle: usize) -> u64 {
    host_ref(handle as *mut c_void).map_or(0, MacAudioHost::dropped_input_blocks)
}

/// An absent host answers with the empty catalog the deleted `!m_driver` branch
/// returned, which is not what a live host with no selected device reports.
pub fn view_list_devices_json(handle: usize) -> String {
    host_ref(handle as *mut c_void)
        .map_or_else(|| "[]".to_owned(), MacAudioHost::list_devices_json)
}

pub fn view_selected_device_uid(handle: usize) -> String {
    host_ref(handle as *mut c_void)
        .map_or_else(String::new, MacAudioHost::selected_device_uid)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_start(host: *mut c_void) -> bool {
    host_ref(host).is_some_and(|host| host.start(None))
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_stop(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.stop();
    }
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_is_running(host: *mut c_void) -> bool {
    host_ref(host).is_some_and(MacAudioHost::is_running)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_sample_rate(host: *mut c_void) -> f64 {
    host_ref(host).map_or(FALLBACK_SAMPLE_RATE, MacAudioHost::sample_rate)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_buffer_size(host: *mut c_void) -> u32 {
    host_ref(host).map_or(FALLBACK_BUFFER_SIZE, MacAudioHost::buffer_size)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_input_channels(host: *mut c_void) -> u32 {
    host_ref(host).map_or(0, MacAudioHost::input_channel_count)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_status(host: *mut c_void) -> *const c_char {
    host_ref(host).map_or_else(
        || status_c_string(STATE_UNAVAILABLE),
        |host| status_c_string(host.state.load(Ordering::Acquire)),
    )
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_last_error_code(host: *mut c_void) -> i32 {
    host_ref(host).map_or(0, MacAudioHost::last_error_code)
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_try_reconnect(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.try_reconnect();
    }
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_reconfigure(
    host: *mut c_void,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.reconfigure(FormatRequest {
            sample_rate,
            buffer_size,
        })
    })
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_select_device(
    host: *mut c_void,
    device: u32,
    sample_rate: f64,
    buffer_size: u32,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.select_device(
            device,
            FormatRequest {
                sample_rate,
                buffer_size,
            },
        )
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mac_audio_host_poll_input_block(
    host: *mut c_void,
    destination: *const *mut f32,
    destination_channels: u32,
    destination_frames: u32,
    info: *mut BlockInfo,
    dropped: *mut u64,
) -> bool {
    host_ref(host).is_some_and(|host| {
        host.poll_input_block(
            destination,
            destination_channels,
            destination_frames,
            info,
            dropped,
        )
    })
}

#[no_mangle]
pub extern "C" fn hirari_mac_audio_host_discard_pending_input(host: *mut c_void) {
    if let Some(host) = host_ref(host) {
        host.discard_pending_input_blocks();
    }
}

#[cfg(test)]
#[path = "mac_audio_driver_tests.rs"]
mod mac_audio_driver_tests;
