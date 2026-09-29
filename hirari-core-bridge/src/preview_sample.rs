/// Renders a mono preview sample into a stereo audio block with linear
/// interpolation. The cursor remains owned by the caller's audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sample_render(
    samples: *const f32,
    sample_count: usize,
    source_rate: f64,
    target_rate: f64,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    position: *mut f64,
) -> bool {
    if samples.is_null()
        || left.is_null()
        || right.is_null()
        || position.is_null()
        || sample_count == 0
        || frames == 0
        || !target_rate.is_finite()
        || target_rate <= 0.0
    {
        return false;
    }
    let source = unsafe { std::slice::from_raw_parts(samples, sample_count) };
    let (left, right) = unsafe {
        (
            std::slice::from_raw_parts_mut(left, frames as usize),
            std::slice::from_raw_parts_mut(right, frames as usize),
        )
    };
    let cursor = unsafe { &mut *position };
    if !cursor.is_finite() || *cursor < 0.0 {
        *cursor = 0.0;
    }
    let rate = if source_rate.is_finite() {
        source_rate.max(1.0)
    } else {
        target_rate
    };
    let step = rate / target_rate;
    for (left, right) in left.iter_mut().zip(right) {
        if *cursor >= sample_count as f64 {
            break;
        }
        let index = *cursor as usize;
        let a = source[index];
        let b = source.get(index + 1).copied().unwrap_or(0.0);
        let fraction = (*cursor - index as f64) as f32;
        let sample = (a + (b - a) * fraction) * 0.8;
        *left += sample;
        *right += sample;
        *cursor += step;
    }
    if *cursor >= sample_count as f64 {
        *cursor = 0.0;
        false
    } else {
        true
    }
}
