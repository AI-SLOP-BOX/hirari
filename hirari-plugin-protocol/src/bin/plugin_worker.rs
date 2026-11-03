//! Process entry point for the isolated plugin worker.
//!
//! The helper is deliberately a separate executable: plugin loading never
//! happens in the DAW process, so a crashing third-party binary cannot take the
//! sequencer down with it. All of the policy lives in `worker_main`.

use std::ffi::{CString, OsString};
use std::process::ExitCode;

/// Argument bytes are passed straight to the operating system, so a plugin path
/// is never required to be valid UTF-8.
#[cfg(unix)]
fn argument_bytes(argument: OsString) -> Vec<u8> {
    std::os::unix::ffi::OsStringExt::into_vec(argument)
}

#[cfg(not(unix))]
fn argument_bytes(argument: OsString) -> Vec<u8> {
    argument.to_string_lossy().into_owned().into_bytes()
}

fn main() -> ExitCode {
    let owned: Vec<CString> = std::env::args_os()
        .map(|argument| argument_bytes(argument))
        .map(|bytes| CString::new(bytes).unwrap_or_else(|_| CString::new("").unwrap()))
        .collect();
    let mut argv: Vec<*const std::ffi::c_char> =
        owned.iter().map(|argument| argument.as_ptr()).collect();
    argv.push(std::ptr::null());
    let argc = (argv.len() - 1) as i32;
    let status = unsafe { hirari_plugin_protocol::worker_main::worker_entry(argc, argv.as_ptr()) };
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}
