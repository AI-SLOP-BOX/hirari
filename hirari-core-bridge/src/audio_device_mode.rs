//! Environment-driven audio device admission.
//!
//! Control-plane clients (CLI renders, offline jobs and isolated tests) must be
//! able to build an engine without claiming the user's hardware device. The
//! variables are read on every call, so a host can switch modes between
//! sessions exactly as the previous per-call `getenv` comparisons did.

use std::ffi::OsStr;

const OFFLINE_MODE_VARIABLE: &str = "HIRARI_AUDIO_MODE";
const ISOLATION_VARIABLE: &str = "HIRARI_NATIVE_TEST_ISOLATION";

fn is_offline_mode(requested: Option<&OsStr>) -> bool {
    requested.is_some_and(|value| value == "offline" || value == "none")
}

fn is_isolated(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

/// True when the host asked for a device-less engine via `HIRARI_AUDIO_MODE`.
#[no_mangle]
pub extern "C" fn hirari_audio_device_mode_offline() -> bool {
    is_offline_mode(std::env::var_os(OFFLINE_MODE_VARIABLE).as_deref())
}

/// True when native integration tests asked the engine to stay off hardware.
#[no_mangle]
pub extern "C" fn hirari_native_test_isolation() -> bool {
    is_isolated(std::env::var_os(ISOLATION_VARIABLE).as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_mode_accepts_only_the_two_documented_values() {
        assert!(is_offline_mode(Some(OsStr::new("offline"))));
        assert!(is_offline_mode(Some(OsStr::new("none"))));
        assert!(!is_offline_mode(None));
        assert!(!is_offline_mode(Some(OsStr::new(""))));
        assert!(!is_offline_mode(Some(OsStr::new("Offline"))));
        assert!(!is_offline_mode(Some(OsStr::new("offline "))));
        assert!(!is_offline_mode(Some(OsStr::new("coreaudio"))));
    }

    #[test]
    fn isolation_accepts_only_the_documented_flag() {
        assert!(is_isolated(Some(OsStr::new("1"))));
        assert!(!is_isolated(None));
        assert!(!is_isolated(Some(OsStr::new(""))));
        assert!(!is_isolated(Some(OsStr::new("0"))));
        assert!(!is_isolated(Some(OsStr::new("true"))));
        assert!(!is_isolated(Some(OsStr::new(" 1"))));
    }

    #[test]
    fn exported_accessors_follow_the_process_environment() {
        // The accessors read the live environment; this test only asserts that
        // an unset variable never turns the gate on, which is the property the
        // control-plane constructors rely on.
        if std::env::var_os(OFFLINE_MODE_VARIABLE).is_none() {
            assert!(!hirari_audio_device_mode_offline());
        }
        if std::env::var_os(ISOLATION_VARIABLE).is_none() {
            assert!(!hirari_native_test_isolation());
        }
    }
}
