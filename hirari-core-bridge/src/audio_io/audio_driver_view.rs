//! Platform-split read view over the Rust-owned audio driver host.
//!
//! The engine publishes the host it constructed (`AudioEngine::audio_driver_host`)
//! and every driver value the UI polls is answered here. On macOS that means
//! reading `mac_audio_driver` through [`crate::mac_audio_driver::view_status`]
//! and its siblings. Off macOS the host is the silent construction in
//! [`crate::audio_driver_host`], so the answers are exactly what the deleted
//! `UnavailableAudioDriver` reported: a permanently silent fallback with no
//! channels, callbacks, peaks, error code or catalog.

/// The lifecycle state string the diagnostics envelope and health snapshot
/// publish. A zero handle is the absent host the C++ null guard named
/// "unavailable"; off macOS the silent fallback is the only state there is.
pub(crate) fn status(handle: usize) -> String {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_status(handle).to_owned();
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        "silent-fallback".to_owned()
    }
}

/// Silent fallback means "nothing audible will ever come out of this device".
/// The unreadable host and a `start-failed` device both qualify, as they did in
/// the C++ forwarder this replaces.
pub(crate) fn is_silent_fallback(handle: usize) -> bool {
    #[cfg(target_os = "macos")]
    return matches!(
        crate::mac_audio_driver::view_status(handle),
        "unavailable" | "start-failed"
    );
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        true
    }
}

pub(crate) fn error_code(handle: usize) -> i32 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_error_code(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0
    }
}

pub(crate) fn output_peak(handle: usize) -> f32 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_output_peak(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0.0
    }
}

pub(crate) fn callback_count(handle: usize) -> u64 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_callback_count(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0
    }
}

pub(crate) fn io_latency_seconds(handle: usize) -> f64 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_io_latency_secs(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0.0
    }
}

pub(crate) fn input_channel_count(handle: usize) -> u32 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_input_channel_count(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0
    }
}

pub(crate) fn output_channel_count(handle: usize) -> u32 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_output_channel_count(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0
    }
}

pub(crate) fn dropped_input_blocks(handle: usize) -> u64 {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_dropped_input_blocks(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        0
    }
}

/// The physical device catalog. Absent hosts report the empty catalog, matching
/// the `!m_driver` branch the deleted forwarder carried.
pub(crate) fn list_devices_json(handle: usize) -> String {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_list_devices_json(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        "[]".to_owned()
    }
}

/// Stable endpoint UID for the device feeding the input callback; empty means no
/// physical endpoint, off macOS always.
pub(crate) fn selected_device_uid(handle: usize) -> String {
    #[cfg(target_os = "macos")]
    return crate::mac_audio_driver::view_selected_device_uid(handle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handle;
        String::new()
    }
}
