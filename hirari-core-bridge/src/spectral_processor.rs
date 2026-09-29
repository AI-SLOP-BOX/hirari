use crate::fft::FftPlan;

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "spectral_differential_tests.rs"]
mod differential_tests;

const FFT_SIZE: usize = 2048;
const HOP: usize = FFT_SIZE / 2;

#[inline]
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn make_window() -> Vec<f32> {
    (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        .collect()
}

unsafe fn channel_mut<'a>(
    channels: *const *mut f32,
    index: usize,
    len: usize,
) -> Option<&'a mut [f32]> {
    let pointer = *channels.add(index);
    if pointer.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts_mut(pointer, len))
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_apply_gain(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    t0: f32,
    f0: f32,
    t1: f32,
    f1: f32,
    gain: f32,
) -> bool {
    let len = frames as usize;
    if channels.is_null()
        || channel_count == 0
        || len == 0
        || !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
        || ![t0, t1, f0, f1, gain].iter().all(|v| v.is_finite())
    {
        return false;
    }
    let t0 = t0.min(t1).max(0.0);
    let t1 = t1.max(t0);
    let f0 = f0.min(f1).max(0.0);
    let f1 = f1.max(f0);
    let gain = gain.clamp(0.0, 8.0);
    let window = make_window();
    let plan = FftPlan::new(FFT_SIZE).unwrap();
    let frame_seconds = FFT_SIZE as f32 / sample_rate as f32;
    let time_feather = (frame_seconds * 0.5).max(1.0e-4);
    let bin_width = sample_rate as f32 / FFT_SIZE as f32;
    let freq_feather = (bin_width * 2.0).max(1.0e-3);
    for channel in 0..channel_count as usize {
        let Some(output) = channel_mut(channels, channel, len) else {
            continue;
        };
        let input = output.to_vec();
        let mut delta = vec![0.0f32; len];
        let mut norm = vec![0.0f32; len];
        let mut real = vec![0.0f32; FFT_SIZE];
        let mut imag = vec![0.0f32; FFT_SIZE];
        for offset in (0..len).step_by(HOP) {
            let time = (offset + FFT_SIZE / 2) as f32 / sample_rate as f32;
            if time < t0 - time_feather || time > t1 + time_feather {
                continue;
            }
            let time_weight = smoothstep((time - (t0 - time_feather)) / time_feather)
                * smoothstep(((t1 + time_feather) - time) / time_feather);
            if time_weight <= 0.0 {
                continue;
            }
            for i in 0..FFT_SIZE {
                let sample = input
                    .get(offset + i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0);
                real[i] = sample * window[i];
                imag[i] = 0.0;
            }
            plan.forward(&mut real, &mut imag);
            for k in 0..FFT_SIZE {
                let raw_frequency = k as f32 * sample_rate as f32 / FFT_SIZE as f32;
                let frequency = raw_frequency.min(sample_rate as f32 - raw_frequency);
                if frequency < f0 - freq_feather || frequency > f1 + freq_feather {
                    continue;
                }
                let low = smoothstep((frequency - (f0 - freq_feather)) / freq_feather);
                let high = smoothstep(((f1 + freq_feather) - frequency) / freq_feather);
                let scale = 1.0 + (gain - 1.0) * time_weight * low * high;
                real[k] *= scale;
                imag[k] *= scale;
            }
            plan.inverse(&mut real, &mut imag);
            for i in 0..FFT_SIZE {
                let Some(position) = offset.checked_add(i).filter(|p| *p < len) else {
                    break;
                };
                let difference = (real[i]
                    - input
                        .get(offset + i)
                        .copied()
                        .filter(|v| v.is_finite())
                        .unwrap_or(0.0)
                        * window[i])
                    * window[i];
                if difference.is_finite() {
                    delta[position] += difference;
                }
                norm[position] += window[i] * window[i];
            }
        }
        for i in 0..len {
            let difference = if norm[i] > 1.0e-6 {
                delta[i] / norm[i]
            } else {
                0.0
            };
            if difference.is_finite() {
                output[i] += difference;
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_apply_gain_regions(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    regions: *const f32,
    region_count: u32,
    gain: f32,
) -> bool {
    if regions.is_null() || region_count == 0 {
        return false;
    }
    let regions = std::slice::from_raw_parts(regions, region_count as usize * 4);
    let mut applied = false;
    for region in regions.chunks_exact(4) {
        applied |= hirari_spectral_processor_apply_gain(
            channels,
            channel_count,
            frames,
            sample_rate,
            region[0],
            region[1],
            region[2],
            region[3],
            gain,
        );
    }
    applied
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_apply_mask(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    regions: *const f32,
    gains: *const f32,
    region_count: u32,
) -> bool {
    if regions.is_null() || gains.is_null() || region_count == 0 {
        return false;
    }
    let regions = std::slice::from_raw_parts(regions, region_count as usize * 4);
    let gains = std::slice::from_raw_parts(gains, region_count as usize);
    let mut applied = false;
    for (region, gain) in regions.chunks_exact(4).zip(gains) {
        if ![region[0], region[1], region[2], region[3], *gain]
            .iter()
            .all(|v| v.is_finite())
        {
            continue;
        }
        applied |= hirari_spectral_processor_apply_gain(
            channels,
            channel_count,
            frames,
            sample_rate,
            region[0],
            region[1],
            region[2],
            region[3],
            *gain,
        );
    }
    applied
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_remove_hum(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    fundamental: f32,
    harmonics: u32,
    bandwidth: f32,
    t0: f32,
    f0: f32,
    t1: f32,
    f1: f32,
) -> bool {
    if !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
        || !fundamental.is_finite()
        || fundamental <= 0.0
        || !bandwidth.is_finite()
        || bandwidth <= 0.0
        || harmonics == 0
        || ![t0, f0, t1, f1].iter().all(|v| v.is_finite())
        || channel_count == 0
        || frames == 0
    {
        return false;
    }
    let t0 = t0.min(t1).max(0.0);
    let t1 = t1.max(t0);
    let f0 = f0.min(f1).max(0.0);
    let f1 = f1.max(f0);
    let nyquist = sample_rate as f32 * 0.5;
    let mut applied = false;
    for harmonic in 1..=harmonics.min(32) {
        let frequency = fundamental * harmonic as f32;
        if frequency >= nyquist {
            break;
        }
        let low = f0.max(frequency - bandwidth);
        let high = f1.min(frequency + bandwidth);
        if high > low {
            applied |= hirari_spectral_processor_apply_gain(
                channels,
                channel_count,
                frames,
                sample_rate,
                t0,
                low,
                t1,
                high,
                0.0,
            );
        }
    }
    applied
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_interpolate(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    t0: f32,
    f0: f32,
    t1: f32,
    f1: f32,
    blend: f32,
) -> bool {
    let len = frames as usize;
    if channels.is_null()
        || channel_count == 0
        || len < 64
        || !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
        || ![t0, t1, f0, f1, blend].iter().all(|v| v.is_finite())
    {
        return false;
    }
    let t0 = t0.min(t1).max(0.0);
    let t1 = t1.max(t0);
    let f0 = f0.min(f1).max(0.0);
    let f1 = f1.max(f0);
    let blend = blend.clamp(0.0, 1.0);
    let window = make_window();
    let plan = FftPlan::new(FFT_SIZE).unwrap();
    let time_feather = ((FFT_SIZE as f32 / sample_rate as f32) * 0.5).max(1.0e-4);
    let bin_width = sample_rate as f32 / FFT_SIZE as f32;
    let freq_feather = (bin_width * 2.0).max(1.0e-3);
    for channel in 0..channel_count as usize {
        let Some(output) = channel_mut(channels, channel, len) else {
            continue;
        };
        let input = output.to_vec();
        let mut delta = vec![0.0f32; len];
        let mut norm = vec![0.0f32; len];
        let mut real = vec![0.0f32; FFT_SIZE];
        let mut imag = vec![0.0f32; FFT_SIZE];
        let mut previous_real = vec![0.0f32; FFT_SIZE];
        let mut previous_imag = vec![0.0f32; FFT_SIZE];
        let mut have_previous = false;
        for offset in (0..len).step_by(HOP) {
            let time = (offset + FFT_SIZE / 2) as f32 / sample_rate as f32;
            for i in 0..FFT_SIZE {
                let sample = input
                    .get(offset + i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0);
                real[i] = sample * window[i];
                imag[i] = 0.0;
            }
            plan.forward(&mut real, &mut imag);
            previous_real.copy_from_slice(&real);
            previous_imag.copy_from_slice(&imag);
            if time >= t0 - time_feather && time <= t1 + time_feather {
                let time_weight = smoothstep((time - (t0 - time_feather)) / time_feather)
                    * smoothstep(((t1 + time_feather) - time) / time_feather);
                let first = ((f0 / bin_width).floor() as i32).max(0);
                let last = ((f1 / bin_width).ceil() as i32).min((FFT_SIZE / 2) as i32);
                let left = if first > 0 {
                    first - 1
                } else {
                    (last + 1).min((FFT_SIZE / 2) as i32)
                } as usize;
                let right = if last < (FFT_SIZE / 2) as i32 {
                    last + 1
                } else {
                    (first - 1).max(0)
                } as usize;
                let full_band = first == 0 && last == (FFT_SIZE / 2) as i32;
                for k in first..=last {
                    let k = k as usize;
                    let mirror = FFT_SIZE as i32 - k as i32;
                    let alpha = if last > first {
                        (k as i32 - first) as f32 / (last - first) as f32
                    } else {
                        0.5
                    };
                    let (estimate_real, estimate_imag) = if full_band && have_previous {
                        (previous_real[k], previous_imag[k])
                    } else {
                        (
                            real[left] * (1.0 - alpha) + real[right] * alpha,
                            imag[left] * (1.0 - alpha) + imag[right] * alpha,
                        )
                    };
                    let frequency = k as f32 * bin_width;
                    let low = smoothstep((frequency - (f0 - freq_feather)) / freq_feather);
                    let high = smoothstep(((f1 + freq_feather) - frequency) / freq_feather);
                    let amount = blend * time_weight * low * high;
                    real[k] = real[k] * (1.0 - amount) + estimate_real * amount;
                    imag[k] = imag[k] * (1.0 - amount) + estimate_imag * amount;
                    if mirror > 0 && mirror < FFT_SIZE as i32 {
                        real[mirror as usize] = real[k];
                        imag[mirror as usize] = -imag[k];
                    }
                }
            }
            have_previous = true;
            plan.inverse(&mut real, &mut imag);
            for i in 0..FFT_SIZE {
                let Some(position) = offset.checked_add(i).filter(|p| *p < len) else {
                    break;
                };
                let original = input
                    .get(offset + i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0)
                    * window[i];
                delta[position] += (real[i] - original) * window[i];
                norm[position] += window[i] * window[i];
            }
        }
        for i in 0..len {
            let difference = if norm[i] > 1.0e-6 {
                delta[i] / norm[i]
            } else {
                0.0
            };
            if difference.is_finite() {
                output[i] = if output[i].is_finite() {
                    output[i] + difference
                } else {
                    difference
                };
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_reduce_noise(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    t0: f32,
    f0: f32,
    t1: f32,
    f1: f32,
    amount: f32,
    profile_seconds: f32,
) -> bool {
    let len = frames as usize;
    if channels.is_null()
        || channel_count == 0
        || len < 64
        || !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
        || ![t0, t1, f0, f1, amount, profile_seconds]
            .iter()
            .all(|v| v.is_finite())
    {
        return false;
    }
    let t0 = t0.min(t1).max(0.0);
    let t1 = t1.max(t0);
    let f0 = f0.min(f1).max(0.0);
    let f1 = f1.max(f0);
    let amount = amount.clamp(0.0, 1.0);
    let profile_seconds = profile_seconds.clamp(0.02, 10.0);
    let window = make_window();
    let plan = FftPlan::new(FFT_SIZE).unwrap();
    let time_feather = ((FFT_SIZE as f32 / sample_rate as f32) * 0.5).max(1.0e-4);
    let bin_width = sample_rate as f32 / FFT_SIZE as f32;
    let freq_feather = (bin_width * 2.0).max(1.0e-3);
    let restrict_frequency = f1 > f0;
    let profile_frames = ((profile_seconds * sample_rate as f32 / HOP as f32) as usize).max(1);
    let mut originals = Vec::with_capacity(channel_count as usize);
    for channel in 0..channel_count as usize {
        let pointer = *channels.add(channel);
        if pointer.is_null() {
            originals.push(None);
        } else {
            originals.push(Some(std::slice::from_raw_parts(pointer, len).to_vec()));
        }
    }
    let mut profile = vec![0.0f32; FFT_SIZE];
    let mut learned = 0usize;
    let mut real = vec![0.0f32; FFT_SIZE];
    let mut imag = vec![0.0f32; FFT_SIZE];
    for data in originals.iter().flatten() {
        let mut channel_frames = 0;
        for offset in (0..len).step_by(HOP).take(profile_frames) {
            if channel_frames >= profile_frames {
                break;
            }
            for i in 0..FFT_SIZE {
                real[i] = data
                    .get(offset + i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0)
                    * window[i];
                imag[i] = 0.0;
            }
            plan.forward(&mut real, &mut imag);
            for k in 0..FFT_SIZE {
                profile[k] += (real[k] * real[k] + imag[k] * imag[k]).sqrt();
            }
            channel_frames += 1;
        }
        learned += channel_frames;
    }
    if learned == 0 {
        return false;
    }
    for value in &mut profile {
        *value /= learned as f32;
    }
    for (channel, original) in originals.iter().enumerate() {
        let Some(input) = original else {
            continue;
        };
        let Some(output) = channel_mut(channels, channel, len) else {
            continue;
        };
        let mut delta = vec![0.0f32; len];
        let mut norm = vec![0.0f32; len];
        for offset in (0..len).step_by(HOP) {
            let time = (offset + FFT_SIZE / 2) as f32 / sample_rate as f32;
            if time < t0 - time_feather || time > t1 + time_feather {
                continue;
            }
            let time_weight = smoothstep((time - (t0 - time_feather)) / time_feather)
                * smoothstep(((t1 + time_feather) - time) / time_feather);
            if time_weight <= 0.0 {
                continue;
            }
            for i in 0..FFT_SIZE {
                real[i] = input
                    .get(offset + i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0)
                    * window[i];
                imag[i] = 0.0;
            }
            plan.forward(&mut real, &mut imag);
            for k in 0..FFT_SIZE {
                let magnitude = (real[k] * real[k] + imag[k] * imag[k]).sqrt();
                if magnitude <= 1.0e-9 {
                    continue;
                }
                let mut frequency_weight = 1.0;
                if restrict_frequency {
                    let raw = k as f32 * bin_width;
                    let frequency = raw.min(sample_rate as f32 - raw);
                    if frequency < f0 - freq_feather || frequency > f1 + freq_feather {
                        continue;
                    }
                    frequency_weight = smoothstep((frequency - (f0 - freq_feather)) / freq_feather)
                        * smoothstep(((f1 + freq_feather) - frequency) / freq_feather);
                }
                let subtraction =
                    (magnitude * 0.98).min(profile[k] * amount * time_weight * frequency_weight);
                let scale = (magnitude - subtraction) / magnitude;
                real[k] *= scale;
                imag[k] *= scale;
            }
            plan.inverse(&mut real, &mut imag);
            for i in 0..FFT_SIZE {
                let Some(position) = offset.checked_add(i).filter(|p| *p < len) else {
                    break;
                };
                delta[position] += real[i] * window[i];
                norm[position] += window[i] * window[i];
            }
        }
        for i in 0..len {
            let value = if norm[i] > 1.0e-6 {
                delta[i] / norm[i]
            } else {
                output[i]
            };
            if value.is_finite() {
                output[i] = value;
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_remove_clicks(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    t0: f32,
    t1: f32,
    threshold: f32,
    radius: u32,
    selected: bool,
) -> u32 {
    let len = frames as usize;
    if channels.is_null() || channel_count == 0 {
        return 0;
    }
    let mut threshold = if threshold.is_finite() {
        threshold
    } else {
        0.65
    };
    threshold = threshold.clamp(0.01, 1.0);
    let mut radius = radius.clamp(1, 64) as usize;
    if !selected {
        if len < radius * 2 + 1 {
            return 0;
        }
    } else {
        if len < 3
            || !sample_rate.is_finite()
            || !(8000.0..=384000.0).contains(&sample_rate)
            || !t0.is_finite()
            || !t1.is_finite()
        {
            return 0;
        }
        radius = radius.min((len - 1) / 2);
        if radius == 0 {
            return 0;
        }
    }
    let (first, last) = if selected {
        let duration = len as f64 / sample_rate;
        let start = t0.min(t1).max(0.0).min(duration as f32);
        let end = t1.max(t0).max(start).min(duration as f32);
        let first = radius.max((start as f64 * sample_rate).ceil() as usize);
        let last = (len - radius - 1).min((end as f64 * sample_rate).floor() as usize);
        if first > last {
            return 0;
        }
        (first, last)
    } else {
        (radius, len - radius - 1)
    };
    let mut repaired = 0u32;
    for channel in 0..channel_count as usize {
        let Some(data) = channel_mut(channels, channel, len) else {
            continue;
        };
        let mut i = first;
        while i <= last {
            let value = data[i];
            if !value.is_finite() {
                data[i] = 0.0;
                repaired += 1;
                i += 1;
                continue;
            }
            let left = data[i - radius];
            let right = data[i + radius];
            let baseline = 0.5 * (left + right);
            if (value - baseline).abs() <= threshold
                || (value - left).abs() <= threshold
                || (value - right).abs() <= threshold
            {
                i += 1;
                continue;
            }
            let begin = if selected {
                (i - radius).max(first)
            } else {
                i - radius
            };
            let end = if selected {
                (i + radius).min(last)
            } else {
                i + radius
            };
            if end <= begin {
                i += 1;
                continue;
            }
            for j in begin..=end {
                let t = (j - begin) as f32 / (end - begin) as f32;
                let s = t * t * (3.0 - 2.0 * t);
                data[j] = left + (right - left) * s;
            }
            repaired += 1;
            i = if selected {
                last.min(i + radius) + 1
            } else {
                i + radius + 1
            };
        }
    }
    repaired
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_processor_repair_clipped(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    sample_rate: f64,
    t0: f32,
    t1: f32,
    ceiling: f32,
    selected: bool,
) -> u32 {
    let len = frames as usize;
    if channels.is_null() || channel_count == 0 {
        return 0;
    }
    let ceiling = (if ceiling.is_finite() {
        ceiling.abs()
    } else {
        0.999
    })
    .clamp(0.5, 1.0);
    let (first, last) = if selected {
        if len < 3
            || !sample_rate.is_finite()
            || !(8000.0..=384000.0).contains(&sample_rate)
            || !t0.is_finite()
            || !t1.is_finite()
        {
            return 0;
        }
        let duration = len as f64 / sample_rate;
        let start = t0.min(t1).max(0.0).min(duration as f32);
        let end = t1.max(t0).max(start).min(duration as f32);
        let first = 1usize.max((start as f64 * sample_rate).ceil() as usize);
        let last = (len - 2).min((end as f64 * sample_rate).floor() as usize);
        if first > last {
            return 0;
        }
        (first, last)
    } else {
        if len < 3 {
            return 0;
        }
        (1, len - 2)
    };
    let mut repaired = 0u32;
    for channel in 0..channel_count as usize {
        let Some(data) = channel_mut(channels, channel, len) else {
            continue;
        };
        let mut i = first;
        while i <= last {
            if !data[i].is_finite() || data[i].abs() < ceiling {
                i += 1;
                continue;
            }
            let start = i;
            while i <= last && data[i].is_finite() && data[i].abs() >= ceiling {
                i += 1;
            }
            let end = i;
            if (selected && (start == first || end > last)) || end <= start {
                continue;
            }
            let a = data[start - 1];
            let b = data[end];
            let span = end - start;
            for j in 0..span {
                let t = (j + 1) as f32 / (span + 1) as f32;
                let s = t * t * (3.0 - 2.0 * t);
                data[start + j] = a + (b - a) * s;
            }
            repaired += span as u32;
        }
    }
    repaired
}
