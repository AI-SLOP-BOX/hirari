//! Tests split out of worker_runtime.rs.

use super::*;

#[test]
fn auto_reports_only_the_matching_adapters() {
    assert_eq!(resolve_formats(0, false, false, false, false), 0);
    assert_eq!(
        resolve_formats(0, true, false, true, false),
        WORKER_FORMAT_CLAP | WORKER_FORMAT_AUDIO_UNIT
    );
}

#[test]
fn explicit_requests_never_fall_back_to_another_format() {
    assert_eq!(
        resolve_formats(2, true, true, true, true),
        WORKER_FORMAT_CLAP
    );
    assert_eq!(
        resolve_formats(3, true, false, true, true),
        WORKER_FORMAT_UNSUPPORTED
    );
    assert_eq!(
        resolve_formats(4, false, false, true, false),
        WORKER_FORMAT_AUDIO_UNIT
    );
}

#[test]
fn builtin_requests_require_the_builtin_scheme() {
    assert_eq!(resolve_formats(1, false, false, false, true), 0);
    assert_eq!(
        resolve_formats(1, true, false, false, false),
        WORKER_FORMAT_UNSUPPORTED
    );
}

#[test]
fn load_failures_carry_error_and_exit_status() {
    assert_eq!(hirari_plugin_worker_load_failure(1), (0xB0 << 8) | 6);
    assert_eq!(hirari_plugin_worker_load_failure(2), (0xB1 << 8) | 8);
    assert_eq!(hirari_plugin_worker_load_failure(-1), (0xB2 << 8) | 10);
}

#[test]
fn parse_args_publishes_the_resolved_formats() {
    let owned: std::vec::Vec<std::ffi::CString> = [
        "worker",
        "--control-fd",
        "3",
        "--status-fd",
        "4",
        "--shared-name",
        "block",
        "--plugin",
        "/plugins/demo.component",
        "--format",
        "auto",
    ]
    .iter()
    .map(|value| std::ffi::CString::new(*value).unwrap())
    .collect();
    let argv: std::vec::Vec<*const c_char> = owned.iter().map(|value| value.as_ptr()).collect();
    let mut options: WorkerLaunchOptions = unsafe { std::mem::zeroed() };
    let mut error_kind = 99;
    let parsed = unsafe {
        hirari_plugin_worker_parse_args(
            argv.len() as i32,
            argv.as_ptr(),
            &mut options,
            &mut error_kind,
        )
    };
    assert!(parsed);
    assert_eq!(error_kind, 0);
    assert_eq!(options.resolved_formats, WORKER_FORMAT_AUDIO_UNIT);
}
