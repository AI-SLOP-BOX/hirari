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

/// Formats the native WAV decoder's diagnostic after C++ has supplied its
/// decoded metadata and channel planes.
#[no_mangle]
pub unsafe extern "C" fn hirari_wav_diagnostic_json(
    ok: bool,
    wave64: bool,
    error: *const u8,
    error_length: usize,
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
    frames: u64,
    planes: *const *const f32,
    samples_per_channel: usize,
) -> *mut WavDiagnosticJson {
    if error_length != 0 && error.is_null() {
        return std::ptr::null_mut();
    }
    if ok && (channels == 0 || planes.is_null()) {
        return std::ptr::null_mut();
    }
    let error = if error_length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(error, error_length) }
    };
    let format = if wave64 { "WAVE64" } else { "WAVE" };
    let mut json = format!("{{\"ok\":{},\"format\":\"{}\"", ok, format).into_bytes();
    if ok {
        json.extend_from_slice(format!(
            ",\"sample_rate\":{},\"channels\":{},\"bit_depth\":{},\"samples\":{}",
            sample_rate, channels, bit_depth, frames
        ).as_bytes());
        let channel_planes = unsafe { std::slice::from_raw_parts(planes, channels as usize) };
        let mut peak = 0.0_f64;
        let mut sum_squares = 0.0_f64;
        let mut finite_samples = 0_u64;
        let mut non_finite_samples = 0_u64;
        for &plane in channel_planes {
            if plane.is_null() && samples_per_channel != 0 {
                return std::ptr::null_mut();
            }
            let values = if samples_per_channel == 0 {
                &[][..]
            } else {
                unsafe { std::slice::from_raw_parts(plane, samples_per_channel) }
            };
            for &sample in values {
                if !sample.is_finite() {
                    non_finite_samples += 1;
                    continue;
                }
                peak = peak.max(f64::from(sample).abs());
                sum_squares += f64::from(sample) * f64::from(sample);
                finite_samples += 1;
            }
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
    Box::into_raw(Box::new(WavDiagnosticJson { bytes: json }))
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
