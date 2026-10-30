//! Frozen windowed-sinc kernel and warped source reader.

const KERNEL_VALUES: usize = 17 * 64 * 8;

#[no_mangle]
pub unsafe extern "C" fn hirari_region_resampler_reference_kernel(
    output: *mut f32,
    capacity: usize,
) -> bool {
    if output.is_null() || capacity < KERNEL_VALUES {
        return false;
    }
    let values = std::slice::from_raw_parts_mut(output, KERNEL_VALUES);
    let pi = std::f64::consts::PI;
    for cutoff_index in 0..17usize {
        let cutoff = f64::exp2(-(cutoff_index as f64) / 4.0);
        for phase in 0..64usize {
            let fraction = phase as f64 / 64.0;
            let mut coefficients = [0.0f64; 8];
            let mut sum = 0.0f64;
            for tap in 0..8usize {
                let x = (tap as f64 - 3.0) - fraction;
                let sinc = if x.abs() < 1.0e-12 {
                    cutoff
                } else {
                    (pi * cutoff * x).sin() / (pi * x)
                };
                let window = if x.abs() >= 4.0 {
                    0.0
                } else if x.abs() < 1.0e-12 {
                    1.0
                } else {
                    (pi * x / 4.0).sin() / (pi * x / 4.0)
                };
                coefficients[tap] = sinc * window;
                sum += sinc * window;
            }
            let normalizer = if sum.abs() > 1.0e-12 { 1.0 / sum } else { 1.0 };
            let base = (cutoff_index * 64 + phase) * 8;
            for tap in 0..8usize {
                values[base + tap] = (coefficients[tap] * normalizer) as f32;
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_read_warped_reference(
    source: *const f32,
    source_samples: u64,
    source_offset: u64,
    source_span: u64,
    position: f64,
    reverse: u8,
    allow_source_preroll: u8,
    resample_step: f64,
    kernel: *const f32,
    output: *mut f32,
) {
    if output.is_null() {
        return;
    }
    *output = 0.0;
    *output.add(1) = 0.0;
    if source.is_null()
        || kernel.is_null()
        || !position.is_finite()
        || source_samples == 0
        || source_span == 0
        || source_samples > i64::MAX as u64
        || source_offset > i64::MAX as u64
    {
        return;
    }
    let floor_position = position.floor();
    if floor_position < i64::MIN as f64 + 8192.0 || floor_position > i64::MAX as f64 - 8192.0 {
        return;
    }
    let relative = floor_position as i64;
    let offset = source_offset as i64;
    let span = source_span as i64;
    let direction = if reverse != 0 { -1i64 } else { 1i64 };
    let source_index = if reverse != 0 {
        span.wrapping_sub(1).wrapping_sub(relative)
    } else {
        relative
    };
    let absolute_index = offset.wrapping_add(source_index);
    let region_last = offset.wrapping_add(span).wrapping_sub(1);
    let fraction = (position - floor_position) as f32;

    let tap = |tap_offset: i64| -> f32 {
        let lower = if allow_source_preroll != 0 { 0 } else { offset };
        let upper = if allow_source_preroll != 0 {
            source_samples as i64 - 1
        } else {
            region_last
        };
        let index = absolute_index
            .wrapping_add(direction.wrapping_mul(tap_offset))
            .clamp(lower, upper);
        let value = *source.add(index as usize);
        if value.is_finite() { value } else { 0.0 }
    };

    let y0 = tap(0);
    let y1 = tap(1);
    if (resample_step - 1.0).abs() < 1.0e-6 {
        let ym1 = tap(-1);
        let y2 = tap(2);
        let a = -0.5 * ym1 + 1.5 * y0 - 1.5 * y1 + 0.5 * y2;
        let b = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c = -0.5 * ym1 + 0.5 * y1;
        *output = a * fraction * fraction * fraction + b * fraction * fraction + c * fraction + y0;
    } else {
        let safe_step = resample_step.abs().clamp(1.0 / 16.0, 16.0);
        let cutoff_index = ((safe_step.max(1.0).log2() * 4.0).ceil() as i32).clamp(0, 16);
        let phase_index = ((fraction * 64.0) as i32).clamp(0, 63);
        let kernel_base = (cutoff_index as usize * 64 + phase_index as usize) * 8;
        for tap_index in 0..8i64 {
            *output += tap(tap_index - 3) * *kernel.add(kernel_base + tap_index as usize);
        }
    }
    *output.add(1) = y1 - y0;
}
