/// Mixes the mono source into both channels, advancing `cursor`. Returns false
/// once the sample is exhausted (the cursor is then rewound for a replay).
pub(crate) unsafe fn render_into(
    source: &[f32],
    source_rate: f64,
    target_rate: f64,
    left: &mut [f32],
    right: &mut [f32],
    cursor: &mut f64,
) -> bool {
    if !cursor.is_finite() || *cursor < 0.0 {
        *cursor = 0.0;
    }
    let rate = if source_rate.is_finite() {
        source_rate.max(1.0)
    } else {
        target_rate
    };
    let step = rate / target_rate;
    let sample_count = source.len();
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
