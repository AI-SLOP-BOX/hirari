const MAX_CHANNELS: usize = 16;

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_stem_split_reference(
    inputs: *const *const f32,
    drums: *mut *mut f32,
    bass: *mut *mut f32,
    vocals: *mut *mut f32,
    other: *mut *mut f32,
    channels: u32,
    samples: u32,
    sample_rate: f64,
) -> bool {
    if inputs.is_null()
        || drums.is_null()
        || bass.is_null()
        || vocals.is_null()
        || other.is_null()
        || channels == 0
        || channels as usize > MAX_CHANNELS
        || samples == 0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
    {
        return false;
    }
    let bass_coefficient = (80.0 / sample_rate).clamp(0.001, 0.25) as f32;
    let mut bass_state = [0.0f32; MAX_CHANNELS];
    let mut previous = [0.0f32; MAX_CHANNELS];
    for channel in 0..channels as usize {
        let input = *inputs.add(channel);
        let drum_out = *drums.add(channel);
        let bass_out = *bass.add(channel);
        let vocal_out = *vocals.add(channel);
        let other_out = *other.add(channel);
        if input.is_null()
            || drum_out.is_null()
            || bass_out.is_null()
            || vocal_out.is_null()
            || other_out.is_null()
        {
            return false;
        }
        for index in 0..samples as usize {
            let raw = *input.add(index);
            let sample = if raw.is_finite() { raw } else { 0.0 };
            let delta = (sample - previous[channel]).abs();
            let drum_mask = (delta * 5.0).clamp(0.0, 1.0);
            bass_state[channel] += bass_coefficient * (sample - bass_state[channel]);
            let bass_sample = bass_state[channel] * (1.0 - drum_mask);
            let vocal_mask = (1.0 - (sample - bass_state[channel]).abs() / (sample.abs() + 1.0e-6))
                .clamp(0.0, 1.0);
            let vocal_sample =
                (sample - bass_state[channel]) * (1.0 - drum_mask) * vocal_mask * vocal_mask;
            let drum_sample = sample * drum_mask;
            *drum_out.add(index) = drum_sample;
            *bass_out.add(index) = bass_sample;
            *vocal_out.add(index) = vocal_sample;
            *other_out.add(index) = sample - drum_sample - bass_sample - vocal_sample;
            previous[channel] = sample;
        }
    }
    true
}
