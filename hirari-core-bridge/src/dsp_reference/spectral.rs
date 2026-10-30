//! Frozen Hann-window STFT/OLA spectral gain reference.

use crate::fft_utils::hirari_fft_utils_process;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

unsafe fn fft_process(values: &mut [Complex], inverse: bool) {
    hirari_fft_utils_process(
        values.as_mut_ptr() as *mut f32,
        values.len(),
        inverse,
    );
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[no_mangle]
pub unsafe extern "C" fn spectral_reference_apply_gain(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    mut t0: f32,
    mut f0: f32,
    mut t1: f32,
    mut f1: f32,
    mut gain: f32,
) -> bool {
    if channels.is_null()
        || channel_count == 0
        || frames == 0
        || !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
        || !gain.is_finite()
        || !t0.is_finite()
        || !f0.is_finite()
        || !t1.is_finite()
        || !f1.is_finite()
    {
        return false;
    }
    if t0 > t1 {
        std::mem::swap(&mut t0, &mut t1);
    }
    if f0 > f1 {
        std::mem::swap(&mut f0, &mut f1);
    }
    t0 = t0.max(0.0);
    t1 = t0.max(t1);
    f0 = f0.max(0.0);
    f1 = f0.max(f1);
    gain = gain.clamp(0.0, 8.0);

    const FFT_SIZE: usize = 2048;
    const HOP: usize = FFT_SIZE / 2;
    let frames = frames as usize;

    let mut window = vec![0.0f32; FFT_SIZE];
    let mut original = vec![0.0f32; FFT_SIZE];
    let mut spectrum = vec![Complex::default(); FFT_SIZE];
    let mut delta = vec![0.0f32; frames];
    let mut norm = vec![0.0f32; frames];

    for (index, slot) in window.iter_mut().enumerate() {
        let phase = index as f32 / (FFT_SIZE - 1) as f32;
        *slot = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * phase).cos();
    }
    let frame_seconds = FFT_SIZE as f32 / sample_rate as f32;
    let time_feather = (frame_seconds * 0.5).max(1.0e-4);
    let bin_width = sample_rate as f32 / FFT_SIZE as f32;
    let freq_feather = (bin_width * 2.0).max(1.0e-3);

    for channel in 0..channel_count as usize {
        let data = *channels.add(channel);
        if data.is_null() {
            continue;
        }
        let data = std::slice::from_raw_parts_mut(data, frames);
        delta.fill(0.0);
        norm.fill(0.0);
        let mut offset = 0usize;
        while offset < frames {
            let time =
                (offset + FFT_SIZE / 2) as f32 / sample_rate as f32;
            if time < t0 - time_feather || time > t1 + time_feather {
                offset += HOP;
                continue;
            }
            let time_weight = smoothstep((time - (t0 - time_feather)) / time_feather)
                * smoothstep(((t1 + time_feather) - time) / time_feather);
            if time_weight <= 0.0 {
                offset += HOP;
                continue;
            }
            for index in 0..FFT_SIZE {
                let position = offset + index;
                let sample = if position < frames && data[position].is_finite() {
                    data[position]
                } else {
                    0.0
                };
                original[index] = sample * window[index];
                spectrum[index] = Complex {
                    re: original[index],
                    im: 0.0,
                };
            }
            fft_process(&mut spectrum, false);
            for k in 0..FFT_SIZE {
                let raw_frequency = k as f32 * sample_rate as f32 / FFT_SIZE as f32;
                let frequency = raw_frequency.min(sample_rate as f32 - raw_frequency);
                if frequency < f0 - freq_feather || frequency > f1 + freq_feather {
                    continue;
                }
                let low = smoothstep((frequency - (f0 - freq_feather)) / freq_feather);
                let high = smoothstep(((f1 + freq_feather) - frequency) / freq_feather);
                let scale = 1.0 + (gain - 1.0) * time_weight * low * high;
                spectrum[k].re *= scale;
                spectrum[k].im *= scale;
            }
            fft_process(&mut spectrum, true);
            for index in 0..FFT_SIZE {
                let position = offset + index;
                if position >= frames {
                    break;
                }
                let difference = (spectrum[index].re - original[index]) * window[index];
                if difference.is_finite() {
                    delta[position] += difference;
                }
                norm[position] += window[index] * window[index];
            }
            offset += HOP;
        }
        for index in 0..frames {
            let difference = if norm[index] > 1.0e-6 {
                delta[index] / norm[index]
            } else {
                0.0
            };
            if difference.is_finite() {
                data[index] += difference;
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn spectral_reference_fft(
    real: *mut f32,
    imag: *mut f32,
    size: usize,
    inverse: bool,
) -> bool {
    if real.is_null() || imag.is_null() || size < 2 || !size.is_power_of_two() {
        return false;
    }
    let real_slice = std::slice::from_raw_parts_mut(real, size);
    let imag_slice = std::slice::from_raw_parts_mut(imag, size);
    let mut spectrum: Vec<Complex> = real_slice
        .iter()
        .zip(imag_slice.iter())
        .map(|(&re, &im)| Complex { re, im })
        .collect();
    fft_process(&mut spectrum, inverse);
    for index in 0..size {
        real_slice[index] = spectrum[index].re;
        imag_slice[index] = spectrum[index].im;
    }
    true
}
