use std::ffi::{c_char, CStr};

/// Status text for a plug-in sandbox failure code. These strings are part of
/// the native status surface: diagnostics, logs and project-hydration checks
/// match them verbatim, so the table lives beside the protocol constants.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_failure_name(code: u8) -> *const c_char {
    let name: &'static CStr = match code {
        0 => c"none",
        1 => c"missing-helper",
        2 => c"invalid-plugin-path",
        3 => c"spawn-failed",
        4 => c"handshake-timeout",
        5 => c"plugin-load-failed",
        6 => c"plugin-abi-invalid",
        7 => c"plugin-instance-failed",
        8 => c"security-setup-failed",
        9 => c"plugin-format-unsupported",
        10 => c"plugin-quarantined",
        11 => c"process-exited",
        12 => c"process-hung",
        13 => c"process-failed",
        14 => c"unsupported-platform",
        _ => c"unknown-failure",
    };
    name.as_ptr()
}

/// Status text for the state-mailbox error code.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_state_error_name(code: u8) -> *const c_char {
    let name: &'static CStr = match code {
        0 => c"none",
        1 => c"state-oversize",
        2 => c"state-version-unsupported",
        3 => c"state-checksum-mismatch",
        4 => c"state-plugin-rejected",
        5 => c"state-unavailable",
        6 => c"state-busy",
        7 => c"state-timeout",
        _ => c"state-unknown-error",
    };
    name.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_code_has_a_distinct_name() {
        let expected = [
            "none",
            "missing-helper",
            "invalid-plugin-path",
            "spawn-failed",
            "handshake-timeout",
            "plugin-load-failed",
            "plugin-abi-invalid",
            "plugin-instance-failed",
            "security-setup-failed",
            "plugin-format-unsupported",
            "plugin-quarantined",
            "process-exited",
            "process-hung",
            "process-failed",
            "unsupported-platform",
        ];
        for (code, name) in expected.iter().enumerate() {
            let actual = unsafe { CStr::from_ptr(hirari_plugin_sandbox_failure_name(code as u8)) };
            assert_eq!(actual.to_str().unwrap(), *name);
        }
        let unknown = unsafe { CStr::from_ptr(hirari_plugin_sandbox_failure_name(200)) };
        assert_eq!(unknown.to_str().unwrap(), "unknown-failure");
    }

    #[test]
    fn every_state_error_code_has_a_name() {
        let expected = [
            "none",
            "state-oversize",
            "state-version-unsupported",
            "state-checksum-mismatch",
            "state-plugin-rejected",
            "state-unavailable",
            "state-busy",
            "state-timeout",
        ];
        for (code, name) in expected.iter().enumerate() {
            let actual =
                unsafe { CStr::from_ptr(hirari_plugin_sandbox_state_error_name(code as u8)) };
            assert_eq!(actual.to_str().unwrap(), *name);
        }
        let unknown = unsafe { CStr::from_ptr(hirari_plugin_sandbox_state_error_name(200)) };
        assert_eq!(unknown.to_str().unwrap(), "state-unknown-error");
    }
}
