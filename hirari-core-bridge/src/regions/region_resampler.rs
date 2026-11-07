use std::{slice, sync::OnceLock};

const KERNEL_VALUES: usize = 17 * 64 * 8;
static RESAMPLE_KERNEL: OnceLock<Box<[f32; KERNEL_VALUES]>> = OnceLock::new();

fn build_resample_kernel() -> Box<[f32; KERNEL_VALUES]> {
    let mut kernel = Box::new([0.0; KERNEL_VALUES]);
    let pi = std::f64::consts::PI;
    for cutoff_index in 0..17 {
        let cutoff = (-(cutoff_index as f64) / 4.0).exp2();
        for phase in 0..64 {
            let fraction = phase as f64 / 64.0;
            let mut coefficients = [0.0f64; 8];
            let mut sum = 0.0;
            for (tap, coefficient) in coefficients.iter_mut().enumerate() {
                let x = (tap as i32 - 3) as f64 - fraction;
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
                *coefficient = sinc * window;
                sum += sinc * window;
            }
            let normalizer = if sum.abs() > 1.0e-12 { 1.0 / sum } else { 1.0 };
            let base = (cutoff_index * 64 + phase) * 8;
            for (tap, coefficient) in coefficients.into_iter().enumerate() {
                kernel[base + tap] = (coefficient * normalizer) as f32;
            }
        }
    }
    kernel
}

fn resample_kernel() -> &'static [f32; KERNEL_VALUES] {
    RESAMPLE_KERNEL.get_or_init(build_resample_kernel)
}

/// Builds and retains the resampling coefficients on the Track construction
/// thread, before any audio callback can request interpolation.
#[no_mangle]
pub extern "C" fn hirari_region_resampler_prepare() -> *const f32 {
    resample_kernel().as_ptr()
}

/// Reads one resampled region sample and its local source slope. The Track
/// renderer retains ownership of immutable audio and the prebuilt kernel;
/// this allocation-free kernel owns the interpolation and boundary rules.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_read_warped(
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
    let output = slice::from_raw_parts_mut(output, 2);
    output.fill(0.0);
    if source.is_null()
        || kernel.is_null()
        || !position.is_finite()
        || source_samples == 0
        || source_span == 0
        || source_samples > i64::MAX as u64
        || source_offset > i64::MAX as u64
        || source_offset.saturating_add(source_span) > source_samples
    {
        return;
    }
    let floor_position = position.floor();
    if floor_position < i64::MIN as f64 + 8192.0 || floor_position > i64::MAX as f64 - 8192.0 {
        return;
    }

    let relative = floor_position as i64;
    let source_offset = source_offset as i64;
    let source_span = source_span as i64;
    let source_index = if reverse != 0 {
        source_span - 1 - relative
    } else {
        relative
    };
    let Some(absolute_index) = source_offset.checked_add(source_index) else {
        return;
    };
    let region_first = source_offset;
    let Some(region_last) = source_offset.checked_add(source_span - 1) else {
        return;
    };
    let fraction = (position - floor_position) as f32;
    let lower_bound = if allow_source_preroll != 0 {
        0
    } else {
        region_first
    };
    let upper_bound = if allow_source_preroll != 0 {
        source_samples as i64 - 1
    } else {
        region_last
    };
    let direction = if reverse != 0 { -1_i64 } else { 1_i64 };

    let tap = |offset: i64| {
        let index = absolute_index
            .saturating_add(direction.saturating_mul(offset))
            .clamp(lower_bound, upper_bound) as usize;
        let value = *source.add(index);
        if value.is_finite() {
            value
        } else {
            0.0
        }
    };

    let y0 = tap(0);
    let y1 = tap(1);
    if (resample_step - 1.0).abs() < 1.0e-6 {
        let ym1 = tap(-1);
        let y2 = tap(2);
        output[0] = interpolate_hermite(ym1, y0, y1, y2, fraction);
    } else {
        let safe_step = if resample_step.is_nan() {
            1.0
        } else {
            resample_step.abs().clamp(1.0 / 16.0, 16.0)
        };
        let cutoff_index = ((safe_step.max(1.0).log2() * 4.0).ceil() as usize).min(16);
        let phase_index = ((fraction * 64.0) as usize).min(63);
        let kernel = slice::from_raw_parts(kernel, KERNEL_VALUES);
        let kernel_base = (cutoff_index * 64 + phase_index) * 8;
        let mut value = 0.0f32;
        for tap_index in 0..8 {
            value += tap(tap_index as i64 - 3) * kernel[kernel_base + tap_index];
        }
        output[0] = value;
    }
    output[1] = y1 - y0;
}

#[inline]
fn interpolate_hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    let d = y1;
    a * t * t * t + b * t * t + c * t + d
}

#[cfg(test)]
mod tests {
    use super::hirari_region_read_warped;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_region_resampler_reference_kernel(output: *mut f32, capacity: usize) -> bool;
        fn hirari_region_read_warped_reference(
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
        );
    }

    #[test]
    #[cfg(feature = "dsp-differential-reference")]
    fn region_resampler_matches_frozen_cpp_kernel() {
        const KERNEL_SIZE: usize = 17 * 64 * 8;
        let mut reference_kernel = [0.0f32; KERNEL_SIZE];
        let rust_kernel = super::hirari_region_resampler_prepare();
        unsafe {
            assert!(hirari_region_resampler_reference_kernel(
                reference_kernel.as_mut_ptr(),
                reference_kernel.len()
            ));
        }

        let source = (0..128)
            .map(|index| ((index as f32 * 0.173).sin() * 0.75) + (index as f32 * 0.001))
            .collect::<Vec<_>>();
        let mut source = source;
        source[41] = f32::NAN;
        source[88] = f32::INFINITY;

        for reverse in [0, 1] {
            for allow_preroll in [0, 1] {
                for step in [1.0, 0.999_999_5, 0.5, 1.5, 2.25, 8.0, 16.0] {
                    for position in [0.0, 0.25, 0.999, 1.5, 15.75, 31.125, 62.875, 63.5, 64.0] {
                        let mut rust = [0.0f32; 2];
                        let mut cpp = [0.0f32; 2];
                        unsafe {
                            hirari_region_read_warped(
                                source.as_ptr(),
                                source.len() as u64,
                                24,
                                64,
                                position,
                                reverse,
                                allow_preroll,
                                step,
                                rust_kernel,
                                rust.as_mut_ptr(),
                            );
                            hirari_region_read_warped_reference(
                                source.as_ptr(),
                                source.len() as u64,
                                24,
                                64,
                                position,
                                reverse,
                                allow_preroll,
                                step,
                                reference_kernel.as_ptr(),
                                cpp.as_mut_ptr(),
                            );
                        }
                        for index in 0..2 {
                            assert!(
                                (rust[index] - cpp[index]).abs() <= 2.0e-6,
                                "mismatch reverse={reverse}, preroll={allow_preroll}, step={step}, position={position}, output={index}: Rust={}, C++={}",
                                rust[index],
                                cpp[index]
                            );
                        }
                    }
                }
            }
        }
    }
}
