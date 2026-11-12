//! Tests split out of worker_runtime.rs.

use super::*;

#[test]
fn parse_error_kinds_report_their_diagnostic() {
    let text = |kind| unsafe {
        core::ffi::CStr::from_ptr(hirari_plugin_worker_parse_error_text(kind))
            .to_str()
            .unwrap()
    };
    assert_eq!(text(1), "missing command-line option value");
    assert_eq!(text(3), "required worker argument is invalid");
    assert_eq!(text(4), "requested plugin format is invalid");
    assert_eq!(text(5), "requested audio configuration is invalid");
    assert_eq!(text(2), "unknown or invalid command-line option");
    assert_eq!(text(0), "unknown or invalid command-line option");
    assert_eq!(text(200), "unknown or invalid command-line option");
}
