//! Filesystem admission checks shared by plugin discovery and launch paths.

use std::ffi::{c_char, CStr};
use std::path::Path;

pub(crate) fn classify_format(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("vst3") => Some("VST3"),
        Some(extension) if extension.eq_ignore_ascii_case("component") => Some("AU"),
        Some(extension) if extension.eq_ignore_ascii_case("clap") => Some("CLAP"),
        _ => None,
    }
}

pub(crate) fn safe_candidate(path: &Path, expected_format: &str) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let file_type = metadata.file_type();
    if !file_type.is_file() && !file_type.is_dir() {
        return false;
    }
    expected_format.is_empty()
        || classify_format(path).is_some_and(|actual| actual.eq_ignore_ascii_case(expected_format))
}

/// Mirrors the user-facing admission result used by Track before it creates a
/// sandbox process: 0=accepted, 1=empty path, 2=missing path, 3=unsupported
/// extension. The host performs its stricter symlink and file-type check when
/// the worker is launched.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_admission_error(path: *const c_char) -> u8 {
    let Some(path) = (unsafe { c_string(path) }) else {
        return 1;
    };
    if path.is_empty() {
        return 1;
    }
    if path.starts_with("builtin://") {
        return 0;
    }
    if std::fs::metadata(&path).is_err() {
        return 2;
    }
    let extension = Path::new(&path)
        .extension()
        .and_then(|value| value.to_str());
    let supported = match extension {
        Some("clap" | "vst3") => true,
        #[cfg(target_os = "macos")]
        Some("component") => true,
        _ => false,
    };
    if supported {
        0
    } else {
        3
    }
}

/// Admission for `PluginScanner::scanInSandbox`. Returns null when the path may
/// be scanned, otherwise the rejection text the scanner publishes verbatim.
/// The checks mirror the filesystem contract the native scanner used: empty
/// path, missing path, inspect failure, directory, then regular file.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_scan_rejection(path: *const c_char) -> *const c_char {
    let Some(path) = (unsafe { c_string(path) }) else {
        return c"plugin scan rejected: path is empty".as_ptr();
    };
    if path.is_empty() {
        return c"plugin scan rejected: path is empty".as_ptr();
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return c"plugin scan rejected: path does not exist".as_ptr();
        }
        Err(_) => return c"plugin scan rejected: unable to inspect path".as_ptr(),
    };
    if metadata.is_dir() {
        return c"plugin scan rejected: path is a directory".as_ptr();
    }
    if !metadata.is_file() {
        return c"plugin scan rejected: path is not a regular file".as_ptr();
    }
    std::ptr::null()
}

unsafe fn c_string(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn rejection(path: &CStr) -> Option<String> {
        let text = unsafe { hirari_plugin_scan_rejection(path.as_ptr()) };
        (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
    }

    #[test]
    fn scan_rejection_reports_the_first_failed_check() {
        assert_eq!(
            rejection(c"").as_deref(),
            Some("plugin scan rejected: path is empty")
        );
        assert_eq!(
            rejection(c"/definitely/not/here/hirari-plugin").as_deref(),
            Some("plugin scan rejected: path does not exist")
        );
        let directory = std::env::temp_dir();
        let directory = CString::new(directory.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(
            rejection(&directory).as_deref(),
            Some("plugin scan rejected: path is a directory")
        );
        assert_eq!(
            rejection(c"/dev/null").as_deref(),
            Some("plugin scan rejected: path is not a regular file")
        );
        let executable = std::env::current_exe().unwrap();
        let executable = CString::new(executable.to_string_lossy().as_bytes()).unwrap();
        assert!(rejection(&executable).is_none());
    }
}
