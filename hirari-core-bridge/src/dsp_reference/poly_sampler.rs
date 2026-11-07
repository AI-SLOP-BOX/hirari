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

