use crate::export::{
    hirari_wave64_decode, hirari_wave64_decoded_free, Wave64DecodedOwned,
};
use crate::preview_audio_runtime::hirari_free_decoded_audio;
use crate::wav_decoder_compat::decode_wav_file;

pub struct WavDiagnosticJson {
    bytes: Vec<u8>,
}

fn escape_json_string(value: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(value.len() + 8);
    for &byte in value {
        match byte {
            b'\\' => escaped.extend_from_slice(b"\\\\"),
            b'"' => escaped.extend_from_slice(b"\\\""),
            b'\n' => escaped.extend_from_slice(b"\\n"),
            b'\r' => escaped.extend_from_slice(b"\\r"),
            b'\t' => escaped.extend_from_slice(b"\\t"),
            byte => escaped.push(byte),
        }
    }
    escaped
}

fn build_json(
    ok: bool,
    wave64: bool,
    error: &[u8],
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
    frames: u64,
    samples: &[f32],
) -> Vec<u8> {
    let format = if wave64 { "WAVE64" } else { "WAVE" };
    let mut json = format!("{{\"ok\":{},\"format\":\"{}\"", ok, format).into_bytes();
    if ok {
        json.extend_from_slice(
            format!(
                ",\"sample_rate\":{},\"channels\":{},\"bit_depth\":{},\"samples\":{}",
                sample_rate, channels, bit_depth, frames
            )
            .as_bytes(),
        );
        let mut peak = 0.0_f64;
        let mut sum_squares = 0.0_f64;
        let mut finite_samples = 0_u64;
        let mut non_finite_samples = 0_u64;
        for &sample in samples {
            if !sample.is_finite() {
                non_finite_samples += 1;
                continue;
            }
            peak = peak.max(f64::from(sample).abs());
            sum_squares += f64::from(sample) * f64::from(sample);
            finite_samples += 1;
        }
        let mean_square = if finite_samples == 0 {
            0.0
        } else {
            sum_squares / finite_samples as f64
        };
        let rms_lufs_estimate = if mean_square > 0.0 {
            (10.0 * mean_square.log10() - 0.691).max(-120.0)
        } else {
            -120.0
        };
        json.extend_from_slice(format!(
            ",\"peak_linear\":{peak:.6},\"rms_lufs_estimate\":{rms_lufs_estimate:.6},\"finite_samples\":{finite_samples},\"non_finite_samples\":{non_finite_samples}"
        ).as_bytes());
    } else {
        json.extend_from_slice(b",\"error\":\"");
        json.extend_from_slice(&escape_json_string(error));
        json.push(b'"');
    }
    json.push(b'}');
    json
}

fn into_handle(json: Vec<u8>) -> *mut WavDiagnosticJson {
    Box::into_raw(Box::new(WavDiagnosticJson { bytes: json }))
}

/// Decodes the file and formats its diagnostic in one call. The decode,
/// failure-reason wording and statistics all live here; C++ only forwards the
/// path and wraps the returned bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_wav_diagnostic_json_for_path(
    path: *const u8,
    path_size: usize,
    wave64: bool,
) -> *mut WavDiagnosticJson {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    if wave64 {
        let mut decoded = Wave64DecodedOwned {
            sample_rate: 0,
            channels: 0,
            bit_depth: 0,
            frames: 0,
            samples: std::ptr::null_mut(),
            sample_count: 0,
            error: std::ptr::null_mut(),
            error_size: 0,
        };
        let decoded_ok = unsafe { hirari_wave64_decode(path.cast(), path_size, &mut decoded) } != 0;
        if decoded_ok {
            let samples =
                unsafe { std::slice::from_raw_parts(decoded.samples, decoded.sample_count) };
            let json = build_json(
                true,
                true,
                &[],
                decoded.sample_rate,
                decoded.channels,
                decoded.bit_depth,
                decoded.frames,
                samples,
            );
            unsafe { hirari_wave64_decoded_free(&mut decoded) };
            return into_handle(json);
        }
        let detail = if decoded.error.is_null() {
            b"unsupported or invalid file".to_vec()
        } else {
            unsafe { std::slice::from_raw_parts(decoded.error, decoded.error_size) }.to_vec()
        };
        unsafe { hirari_wave64_decoded_free(&mut decoded) };
        let mut message = b"WAVE64 decode failed: ".to_vec();
        message.extend_from_slice(&detail);
        message.extend_from_slice(b": ");
        message.extend_from_slice(path_bytes);
        return into_handle(build_json(false, true, &message, 0, 0, 0, 0, &[]));
    }
    match decode_wav_file(path_bytes) {
        Some(decoded) => {
            let samples =
                unsafe { std::slice::from_raw_parts(decoded.samples, decoded.sample_count) };
            let json = build_json(
                true,
                false,
                &[],
                decoded.sample_rate as u32,
                decoded.channels as u16,
                decoded.bits_per_sample as u16,
                (decoded.sample_count / decoded.channels as usize) as u64,
                samples,
            );
            unsafe { hirari_free_decoded_audio(decoded.samples, decoded.sample_count) };
            into_handle(json)
        }
        None => {
            let mut message = b"Invalid or unsupported WAVE file: ".to_vec();
            message.extend_from_slice(path_bytes);
            into_handle(build_json(false, false, &message, 0, 0, 0, 0, &[]))
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_diagnostic_json_data(
    value: *const WavDiagnosticJson,
) -> *const u8 {
    unsafe { value.as_ref() }.map_or(std::ptr::null(), |json| json.bytes.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_diagnostic_json_size(
    value: *const WavDiagnosticJson,
) -> usize {
    unsafe { value.as_ref() }.map_or(0, |json| json.bytes.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_diagnostic_json_free(value: *mut WavDiagnosticJson) {
    if !value.is_null() {
        drop(unsafe { Box::from_raw(value) });
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    fn diagnostic(path: &std::path::Path, wave64: bool) -> String {
        let bytes = path.to_string_lossy().into_owned().into_bytes();
        let handle = unsafe {
            hirari_wav_diagnostic_json_for_path(bytes.as_ptr(), bytes.len(), wave64)
        };
        assert!(!handle.is_null());
        let data = unsafe { hirari_wav_diagnostic_json_data(handle) };
        let size = unsafe { hirari_wav_diagnostic_json_size(handle) };
        let text = String::from_utf8(unsafe { std::slice::from_raw_parts(data, size) }.to_vec())
            .expect("utf8 json");
        unsafe { hirari_wav_diagnostic_json_free(handle) };
        text
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hirari-wav-diagnostic-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reports_ok_error_and_missing_files() {
        let path = temp_path("probe.w64");
        crate::export::write_wave64_float32(&path, &[0.0_f32, 0.25, -0.25, 0.5], 48_000, 2)
            .expect("write");
        let ok = diagnostic(&path, true);
        assert!(ok.contains("\"ok\":true"), "{ok}");
        assert!(ok.contains("\"format\":\"WAVE64\""), "{ok}");
        assert!(ok.contains("\"channels\":2"), "{ok}");

        let mut bytes = std::fs::read(&path).expect("read");
        for byte in &mut bytes[56..64] {
            *byte = 0;
        }
        std::fs::write(&path, &bytes).expect("rewrite");
        let broken = diagnostic(&path, true);
        assert!(broken.contains("\"ok\":false"), "{broken}");
        assert!(broken.contains("\"error\":\""), "{broken}");

        let missing = diagnostic(&path.with_file_name("hirari-wav-does-not-exist.w64"), true);
        assert!(missing.contains("\"ok\":false"), "{missing}");
        assert!(missing.contains("No such file or directory"), "{missing}");
        assert!(missing.contains("hirari-wav-does-not-exist.w64"), "{missing}");
        let _ = std::fs::remove_file(path);
    }
}
