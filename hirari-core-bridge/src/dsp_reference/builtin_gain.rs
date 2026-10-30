#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_process_reference(
    channels: *mut *mut f32,
    channel_count: u32,
    sample_count: u32,
    gain: f32,
) -> u32 {
    if channels.is_null() || !gain.is_finite() {
        return 0;
    }
    let mut sanitized = 0u32;
    for channel in 0..channel_count as usize {
        let samples = *channels.add(channel);
        if samples.is_null() {
            continue;
        }
        for index in 0..sample_count as usize {
            let value = *samples.add(index) * gain;
            if value.is_finite() {
                *samples.add(index) = value;
            } else {
                *samples.add(index) = 0.0;
                sanitized += 1;
            }
        }
    }
    sanitized
}
