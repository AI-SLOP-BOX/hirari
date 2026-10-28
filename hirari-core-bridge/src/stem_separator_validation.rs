use std::ffi::{c_char, CStr};
use std::path::Path;

/// Validate the model-file constraints shared by the optional stem runtimes.
/// Return values are stable bridge error codes consumed by the C++ adapter.
#[no_mangle]
pub unsafe extern "C" fn hirari_stem_separator_validate_model(
    backend: u8,
    model_path: *const u8,
    model_path_len: usize,
    model_sample_rate: u32,
    max_channels: u32,
) -> u8 {
    if backend == 0 { return 0; }
    if model_path_len == 0 { return 1; }
    if model_sample_rate < 8_000 || model_sample_rate > 384_000 { return 2; }
    if max_channels == 0 || max_channels > 32 { return 3; }
    if model_path.is_null() { return 1; }
    let bytes = std::slice::from_raw_parts(model_path, model_path_len);
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(bytes))
    };
    #[cfg(not(unix))]
    let path = match std::str::from_utf8(bytes) { Ok(p) => Path::new(p), Err(_) => return 4 };
    let metadata = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => return 4,
    };
    if metadata.len() == 0 { return 5; }
    if metadata.len() > 4 * 1024 * 1024 * 1024u64 { return 6; }
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if backend == 1 && extension != "onnx" { return 7; }
    if backend == 2 && extension != "pt" && extension != "pth" { return 9; }
    0
}

/// Human-readable text for a validation code. The C++ adapter used to hold
/// this table; keeping it beside the checks means a new code cannot be
/// introduced without its message.
#[no_mangle]
pub extern "C" fn hirari_stem_separator_validation_message(code: u8) -> *const c_char {
    let message: &'static CStr = match code {
        1 => c"model path is empty",
        2 => c"model sample rate is outside the supported range",
        3 => c"model channel count is outside the supported range",
        4 => c"model path is not a regular file",
        5 => c"model file is empty or unreadable",
        6 => c"model file exceeds the 4 GiB safety limit",
        7 => c"ONNX Runtime backend requires an .onnx model",
        9 => c"LibTorch backend requires a .pt or .pth model",
        _ => c"model validation failed",
    };
    message.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_validation_code_has_a_message() {
        for code in 1..=9u8 {
            let message = unsafe { CStr::from_ptr(hirari_stem_separator_validation_message(code)) };
            assert!(!message.to_bytes().is_empty(), "code {code}");
        }
        let unknown =
            unsafe { CStr::from_ptr(hirari_stem_separator_validation_message(200)) };
        assert_eq!(unknown.to_str().unwrap(), "model validation failed");
    }
}
