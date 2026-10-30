//! Frozen scalar polyphonic sampler voice renderer.

fn sample_at(
    data: &[f32],
    raw: i64,
    looping: bool,
    loop_start: u32,
    loop_end: u32,
) -> f32 {
    let length = data.len() as i64;
    let mut index = raw;
    if looping && loop_end as i64 > loop_start as i64 + 1 {
        let start = loop_start as i64;
        let end = loop_end as i64;
        let span = end - start;
        if index < start {
            index = end - ((start - index) % span);
            if index == end {
                index = start;
            }
        } else if index >= end {
            index = start + ((index - start) % span);
        }
    } else {
        index = index.clamp(0, length - 1);
    }
    let value = data[index as usize];
    if value.is_finite() { value } else { 0.0 }
}

fn interpolate(
    data: &[f32],
    position: f64,
    looping: bool,
    loop_start: u32,
    loop_end: u32,
) -> f32 {
    let i = position.floor() as i64;
    let t = (position - i as f64) as f32;
    let y0 = sample_at(data, i - 1, looping, loop_start, loop_end);
    let y1 = sample_at(data, i, looping, loop_start, loop_end);
    let y2 = sample_at(data, i + 1, looping, loop_start, loop_end);
    let y3 = sample_at(data, i + 2, looping, loop_start, loop_end);
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    let result = ((c3 * t + c2) * t + c1) * t + c0;
    if result.is_finite() { result } else { y1 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_reference_process(
    sample: *const f32,
    sample_size: u32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: u32,
    sample_rate: f64,
    source_rate: f64,
    loop_start: u32,
    loop_end: u32,
    release_at: u32,
    velocity: f32,
) {
    if output_left.is_null() || output_right.is_null() {
        return;
    }
    let frames = frames as usize;
    let left = std::slice::from_raw_parts_mut(output_left, frames);
    let right = std::slice::from_raw_parts_mut(output_right, frames);
    left.fill(0.0);
    right.fill(0.0);
    let sample = std::slice::from_raw_parts(sample, sample_size as usize);

    let mut level = 0.0f32;
    let mut filter_left = 0.0f32;
    let mut filter_right = 0.0f32;
    let attack_step = 1.0f32 / (0.005 * sample_rate as f32);
    let decay_step = 0.2f32 / (0.1 * sample_rate as f32);
    let release_coefficient = (-1.0 / (0.3 * sample_rate)).exp() as f32;
    let cutoff = 1000.0f32;
    let rc = 1.0f32 / (2.0 * std::f32::consts::PI * cutoff);
    let one_over_rate = 1.0f32 / sample_rate as f32;
    let alpha = (one_over_rate / (rc + one_over_rate)).clamp(0.001, 1.0);
    let rate_ratio = if source_rate >= 8000.0 && source_rate <= 384000.0 {
        source_rate / sample_rate
    } else {
        1.0
    };

    let mut position = 0.0f64;
    let mut stage = 1i32;
    for (frame, (left_slot, right_slot)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
        if frame as u32 == release_at {
            stage = 4;
        }
        if stage == 0 {
            break;
        }
        if position >= loop_end as f64 {
            let loop_length = (loop_end - loop_start) as f64;
            position = loop_start as f64 + (position - loop_start as f64).max(0.0) % loop_length;
        }
        let sample_value = interpolate(sample, position, true, loop_start, loop_end);
        if stage == 1 {
            level = (level + attack_step).min(1.0);
            if level >= 1.0 {
                stage = 2;
            }
        } else if stage == 2 {
            level = (level - decay_step).max(0.8);
            if level <= 0.8 {
                stage = 3;
            }
        } else if stage == 3 {
            level = 0.8;
        } else {
            level *= release_coefficient;
            if level <= 1.0e-5 {
                level = 0.0;
                stage = 0;
            }
        }
        let amplitude = level * velocity;
        let input = sample_value * amplitude;
        filter_left += alpha * (input - filter_left);
        filter_right += alpha * (input - filter_right);
        *left_slot = if filter_left.is_finite() { filter_left } else { input };
        *right_slot = if filter_right.is_finite() {
            filter_right
        } else {
            input
        };
        position += rate_ratio;
    }
}
