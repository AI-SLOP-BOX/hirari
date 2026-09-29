/// Fills a stereo block with the engine's 440 Hz diagnostic tone.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_test_tone_render(
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    playhead: u64,
    sample_rate: f64,
) -> bool {
    if left.is_null()
        || right.is_null()
        || frames == 0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || playhead.checked_add(u64::from(frames)).is_none()
    {
        return false;
    }
    let (left, right) = unsafe {
        (
            std::slice::from_raw_parts_mut(left, frames as usize),
            std::slice::from_raw_parts_mut(right, frames as usize),
        )
    };
    for (index, (left, right)) in left.iter_mut().zip(right).enumerate() {
        let position = playhead + index as u64;
        let phase = (position as f64 * 440.0 * 2.0 * std::f64::consts::PI) / sample_rate;
        let sample = (phase.sin() * 0.15) as f32;
        *left = sample;
        *right = sample;
    }
    true
}
