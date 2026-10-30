//! Frozen pre-Rust masking analyser with its legacy radix-2 transform.

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

fn legacy_forward(values: &mut [Complex]) {
    let size = values.len();
    let mut j = 0usize;
    for i in 1..size {
        let mut bit = size >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut length = 2usize;
    while length <= size {
        let step = size / length;
        for start in (0..size).step_by(length) {
            for offset in 0..length / 2 {
                let lut_index = (512 - step * offset) % 512;
                let angle = 2.0 * std::f64::consts::PI * lut_index as f64 / 512.0;
                let twiddle_re = angle.cos() as f32;
                let twiddle_im = -(angle.sin()) as f32;
                let even = values[start + offset];
                let odd = values[start + offset + length / 2];
                let odd_re = odd.re * twiddle_re - odd.im * twiddle_im;
                let odd_im = odd.re * twiddle_im + odd.im * twiddle_re;
                values[start + offset] = Complex {
                    re: even.re + odd_re,
                    im: even.im + odd_im,
                };
                values[start + offset + length / 2] = Complex {
                    re: even.re - odd_re,
                    im: even.im - odd_im,
                };
            }
        }
        length <<= 1;
    }
}

fn magnitude(value: Complex) -> f32 {
    (value.re * value.re + value.im * value.im).sqrt()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_masking_analysis_reference(
    target_mono: *const f32,
    other_mono: *const f32,
    other_track_ids: *const u32,
    other_count: usize,
    frame_count: usize,
    output_bins: *mut u32,
    output_intensities: *mut f32,
    output_track_ids: *mut u32,
    output_capacity: usize,
) -> usize {
    if target_mono.is_null()
        || output_bins.is_null()
        || output_intensities.is_null()
        || output_track_ids.is_null()
        || frame_count != 512
        || output_capacity == 0
        || (other_count != 0 && (other_mono.is_null() || other_track_ids.is_null()))
    {
        return 0;
    }

    const BINS: usize = 256;
    const FRAME: usize = 512;

    let target_input = std::slice::from_raw_parts(target_mono, FRAME);
    let mut target = vec![Complex::default(); FRAME];
    for (slot, &sample) in target.iter_mut().zip(target_input) {
        *slot = Complex { re: sample, im: 0.0 };
    }
    legacy_forward(&mut target);
    let target_magnitude: Vec<f32> = target.iter().copied().map(magnitude).collect();

    let other_input = std::slice::from_raw_parts(other_mono, other_count * FRAME);
    let track_ids = std::slice::from_raw_parts(other_track_ids, other_count);
    let bins = std::slice::from_raw_parts_mut(output_bins, output_capacity);
    let intensities = std::slice::from_raw_parts_mut(output_intensities, output_capacity);
    let ids = std::slice::from_raw_parts_mut(output_track_ids, output_capacity);

    let mut written = 0usize;
    let mut other = vec![Complex::default(); FRAME];
    for track in 0..other_count {
        for (index, slot) in other.iter_mut().enumerate() {
            *slot = Complex {
                re: other_input[track * FRAME + index],
                im: 0.0,
            };
        }
        legacy_forward(&mut other);
        for bin in 0..BINS {
            let intensity = target_magnitude[bin] * magnitude(other[bin]);
            if intensity > 0.02 {
                bins[written] = bin as u32;
                intensities[written] = intensity;
                ids[written] = track_ids[track];
                written += 1;
                if written == output_capacity {
                    return written;
                }
            }
        }
    }
    written
}
