const MAX_OUTPUT_SAMPLES: usize = 100_000_000;

fn output_len(source_len: usize, source_rate: f64, target_rate: f64) -> usize {
    if source_len == 0
        || !source_rate.is_finite()
        || !target_rate.is_finite()
        || source_rate <= 0.0
        || target_rate <= 0.0
    {
        return 0;
    }
    if (source_rate - target_rate).abs() < 0.1 {
        return source_len;
    }
    let ratio = source_rate / target_rate;
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0;
    }
    let length = source_len as f64 / ratio;
    if !length.is_finite() || length <= 0.0 || length > MAX_OUTPUT_SAMPLES as f64 {
        return 0;
    }
    (length as usize).min(MAX_OUTPUT_SAMPLES)
}

fn interpolate_cubic(source: &[f32], mut position: f64) -> f32 {
    if source.is_empty() || !position.is_finite() {
        return 0.0;
    }
    position = position.clamp(0.0, (source.len() - 1) as f64);
    let i1 = position as usize;
    let i0 = i1.saturating_sub(1);
    let i2 = (i1 + 1).min(source.len() - 1);
    let i3 = (i2 + 1).min(source.len() - 1);
    let f = (position - i1 as f64) as f32;
    let y0 = source[i0];
    let y1 = source[i1];
    let y2 = source[i2];
    let y3 = source[i3];
    let a = -0.5_f32 * y0 + 1.5_f32 * y1 - 1.5_f32 * y2 + 0.5_f32 * y3;
    let b = y0 - 2.5_f32 * y1 + 2.0_f32 * y2 - 0.5_f32 * y3;
    let c = -0.5_f32 * y0 + 0.5_f32 * y2;
    let value = a * f * f * f + b * f * f + c * f + y1;
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_resampler_output_len(
    source_len: usize,
    source_rate: f64,
    target_rate: f64,
) -> usize {
    output_len(source_len, source_rate, target_rate)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_resampler_process(
    source: *const f32,
    source_len: usize,
    source_rate: f64,
    target_rate: f64,
    output: *mut f32,
    output_capacity: usize,
) -> bool {
    let length = output_len(source_len, source_rate, target_rate);
    if source.is_null() || output.is_null() || length == 0 || output_capacity < length {
        return false;
    }
    let source = std::slice::from_raw_parts(source, source_len);
    let output = std::slice::from_raw_parts_mut(output, length);
    if (source_rate - target_rate).abs() < 0.1 {
        output.copy_from_slice(&source[..length]);
        return true;
    }
    let ratio = source_rate / target_rate;
    for (index, sample) in output.iter_mut().enumerate() {
        let source_position = (index as f64 * ratio).min((source_len - 1) as f64);
        *sample = interpolate_cubic(source, source_position);
    }
    true
}
