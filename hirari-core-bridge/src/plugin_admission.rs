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

/// Stable plugin format IDs used by the C++ compatibility layer.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_format_for_path(path: *const c_char) -> u8 {
    let Some(path) = (unsafe { c_string(path) }) else {
        return 0;
    };
    match classify_format(Path::new(&path)) {
        Some("VST3") => 1,
        Some("AU") => 2,
        Some("CLAP") => 3,
        _ => 0,
    }
}

/// Rejects symlinks and special files before plugin metadata is inspected or
/// a binary is loaded. An empty expected format only asks for a safe file or
/// directory; a non-empty format must match the path extension.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_is_safe_candidate(
    path: *const c_char,
    expected_format: *const c_char,
) -> bool {
    let (Some(path), Some(expected)) = (unsafe { c_string(path) }, unsafe {
        c_string(expected_format)
    }) else {
        return false;
    };
    safe_candidate(Path::new(&path), &expected)
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
