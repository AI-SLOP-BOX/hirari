use std::ffi::{c_char, CStr};
use std::path::Path;

/// Stable display name for a backend code.
pub fn backend_name(backend: u8) -> &'static str {
    match backend {
        0 => "Heuristic",
        1 => "ONNX Runtime",
        2 => "LibTorch",
        3 => "External Process",
        _ => "Unknown",
    }
}

/// Human-readable text for a validation code. Keeping the table beside the
/// checks means a new code cannot be introduced without its message.
pub fn validation_message(code: u8) -> &'static CStr {
    match code {
        1 => c"model path is empty",
        2 => c"model sample rate is outside the supported range",
        3 => c"model channel count is outside the supported range",
        4 => c"model path is not a regular file",
        5 => c"model file is empty or unreadable",
        6 => c"model file exceeds the 4 GiB safety limit",
        7 => c"ONNX Runtime backend requires an .onnx model",
        9 => c"LibTorch backend requires a .pt or .pth model",
        _ => c"model validation failed",
    }
}

fn validate_model_file(backend: u8, path: &Path) -> u8 {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return 4,
    };
    if metadata.len() == 0 {
        return 5;
    }
    if metadata.len() > 4 * 1024 * 1024 * 1024u64 {
        return 6;
    }
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if backend == 1 && extension != "onnx" {
        return 7;
    }
    if backend == 2 && extension != "pt" && extension != "pth" {
        return 9;
    }
    0
}

/// Validate the model-file constraints shared by the optional stem runtimes.
/// Return values are stable bridge error codes consumed by the registry.
pub fn validate_model(
    backend: u8,
    model_path: &str,
    model_sample_rate: u32,
    max_channels: u32,
) -> u8 {
    if backend == 0 {
        return 0;
    }
    if model_path.is_empty() {
        return 1;
    }
    if !(8_000..=384_000).contains(&model_sample_rate) {
        return 2;
    }
    if max_channels == 0 || max_channels > 32 {
        return 3;
    }
    validate_model_file(backend, Path::new(model_path))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stem_separator_validate_model(
    backend: u8,
    model_path: *const u8,
    model_path_len: usize,
    model_sample_rate: u32,
    max_channels: u32,
) -> u8 {
    if backend == 0 {
        return 0;
    }
    if model_path_len == 0 {
        return 1;
    }
    if model_sample_rate < 8_000 || model_sample_rate > 384_000 {
        return 2;
    }
    if max_channels == 0 || max_channels > 32 {
        return 3;
    }
    if model_path.is_null() {
        return 1;
    }
    // SAFETY: The caller promises `model_path_len` readable bytes at `model_path`.
    let bytes = unsafe { std::slice::from_raw_parts(model_path, model_path_len) };
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(bytes))
    };
    #[cfg(not(unix))]
    let path = match std::str::from_utf8(bytes) {
        Ok(path) => Path::new(path),
        Err(_) => return 4,
    };
    validate_model_file(backend, path)
}

#[no_mangle]
pub extern "C" fn hirari_stem_separator_backend_name(backend: u8) -> *const c_char {
    // Every arm is a NUL-terminated C string literal.
    let name: &'static CStr = match backend {
        0 => c"Heuristic",
        1 => c"ONNX Runtime",
        2 => c"LibTorch",
        3 => c"External Process",
        _ => c"Unknown",
    };
    name.as_ptr()
}

#[no_mangle]
pub extern "C" fn hirari_stem_separator_validation_message(code: u8) -> *const c_char {
    validation_message(code).as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_codes_map_to_their_display_names() {
        for (code, name) in ["Heuristic", "ONNX Runtime", "LibTorch", "External Process"]
            .iter()
            .enumerate()
        {
            let actual = unsafe { CStr::from_ptr(hirari_stem_separator_backend_name(code as u8)) };
            assert_eq!(actual.to_str().unwrap(), *name);
        }
        let unknown = unsafe { CStr::from_ptr(hirari_stem_separator_backend_name(200)) };
        assert_eq!(unknown.to_str().unwrap(), "Unknown");
    }

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
