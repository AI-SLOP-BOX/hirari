//! Tests split out of worker_main.rs.

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
