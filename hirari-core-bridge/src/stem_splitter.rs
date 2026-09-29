const MAX_CHANNELS: usize = 16;

/// Rust implementation of the deterministic stem-mask kernel.
///
/// The caller owns input/output buffers. This realtime-independent kernel
/// performs no allocation and resets its one-pole analysis state per call,
/// matching the original splitter's block contract.
#[no_mangle]
pub unsafe extern "C" fn hirari_stem_split_process(
    inputs: *const *const f32,
    drums: *const *mut f32,
    bass: *const *mut f32,
    vocals: *const *mut f32,
    other: *const *mut f32,
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

    let coefficient = (80.0 / sample_rate).clamp(0.001, 0.25) as f32;
    let mut bass_state = [0.0f32; MAX_CHANNELS];
    let mut previous = [0.0f32; MAX_CHANNELS];
    for channel in 0..channels as usize {
        // SAFETY: The caller promises arrays contain one valid plane per channel.
        let (input, drum, bass_out, vocal, other_out) = unsafe {
            (
                *inputs.add(channel),
                *drums.add(channel),
                *bass.add(channel),
                *vocals.add(channel),
                *other.add(channel),
            )
        };
        if input.is_null()
            || drum.is_null()
            || bass_out.is_null()
            || vocal.is_null()
            || other_out.is_null()
        {
            return false;
        }
        for index in 0..samples as usize {
            // SAFETY: Each plane has at least `samples` readable/writable values.
            let raw = unsafe { *input.add(index) };
            let sample = if raw.is_finite() { raw } else { 0.0 };
            let delta = (sample - previous[channel]).abs();
            let drum_mask = (delta * 5.0).clamp(0.0, 1.0);
            bass_state[channel] += coefficient * (sample - bass_state[channel]);
            let bass_sample = bass_state[channel] * (1.0 - drum_mask);
            let vocal_mask = (1.0 - (sample - bass_state[channel]).abs() / (sample.abs() + 1.0e-6))
                .clamp(0.0, 1.0);
            let vocal_sample =
                (sample - bass_state[channel]) * (1.0 - drum_mask) * vocal_mask * vocal_mask;
            let drum_sample = sample * drum_mask;
            // SAFETY: Output planes have at least `samples` writable values.
            unsafe {
                *drum.add(index) = drum_sample;
                *bass_out.add(index) = bass_sample;
                *vocal.add(index) = vocal_sample;
                *other_out.add(index) = sample - drum_sample - bass_sample - vocal_sample;
            }
            previous[channel] = sample;
        }
    }
    true
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod tests {
    use super::hirari_stem_split_process;

    unsafe extern "C" {
        fn hirari_stem_split_reference(
            inputs: *const *const f32,
            drums: *const *mut f32,
            bass: *const *mut f32,
            vocals: *const *mut f32,
            other: *const *mut f32,
            channels: u32,
            samples: u32,
            sample_rate: f64,
        ) -> bool;
    }

    #[test]
    fn rust_stem_split_matches_frozen_cpp_for_varied_audio_and_layouts() {
        let fixtures: [&[f32]; 5] = [
            &[0.0; 129],
            &[0.8; 129],
            &[
                0.0,
                1.0,
                -1.0,
                0.5,
                -0.25,
                f32::NAN,
                0.3,
                f32::INFINITY,
                -f32::INFINITY,
                0.0,
                0.01,
                -0.02,
                0.4,
                0.8,
                -0.8,
                0.0,
            ],
            &[0.0, 0.1, 0.4, 0.9, 0.2, -0.6, -0.1, 0.7],
            &[0.25, -0.5, 0.75, -1.0, 0.5, -0.25],
        ];
        let rates = [8_000.0, 44_100.0, 48_000.0, 96_000.0, 384_000.0];

        for (fixture_index, fixture) in fixtures.iter().enumerate() {
            for channels in [1usize, 2, 3, 8, 16] {
                let sample_count = fixture.len();
                let source: Vec<Vec<f32>> = (0..channels)
                    .map(|channel| {
                        fixture
                            .iter()
                            .enumerate()
                            .map(|(index, sample)| {
                                if (index + channel) % 3 == 0 {
                                    *sample
                                } else {
                                    *sample * (channel as f32 + 1.0) * 0.125
                                }
                            })
                            .collect()
                    })
                    .collect();
                let input_ptrs: Vec<_> = source.iter().map(|plane| plane.as_ptr()).collect();
                let mut rust_outputs: [Vec<Vec<f32>>; 4] =
                    std::array::from_fn(|_| vec![vec![0.0; sample_count]; channels]);
                let mut cpp_outputs: [Vec<Vec<f32>>; 4] =
                    std::array::from_fn(|_| vec![vec![0.0; sample_count]; channels]);
                let mut rust_ptrs: [Vec<*mut f32>; 4] = std::array::from_fn(|_| Vec::new());
                let mut cpp_ptrs: [Vec<*mut f32>; 4] = std::array::from_fn(|_| Vec::new());
                for output in 0..4 {
                    rust_ptrs[output] = rust_outputs[output]
                        .iter_mut()
                        .map(Vec::as_mut_ptr)
                        .collect();
                    cpp_ptrs[output] = cpp_outputs[output]
                        .iter_mut()
                        .map(Vec::as_mut_ptr)
                        .collect();
                }
                let rate = rates[fixture_index];
                let rust_ok = unsafe {
                    hirari_stem_split_process(
                        input_ptrs.as_ptr(),
                        rust_ptrs[0].as_ptr(),
                        rust_ptrs[1].as_ptr(),
                        rust_ptrs[2].as_ptr(),
                        rust_ptrs[3].as_ptr(),
                        channels as u32,
                        sample_count as u32,
                        rate,
                    )
                };
                let cpp_ok = unsafe {
                    hirari_stem_split_reference(
                        input_ptrs.as_ptr(),
                        cpp_ptrs[0].as_ptr(),
                        cpp_ptrs[1].as_ptr(),
                        cpp_ptrs[2].as_ptr(),
                        cpp_ptrs[3].as_ptr(),
                        channels as u32,
                        sample_count as u32,
                        rate,
                    )
                };
                assert!(rust_ok && cpp_ok);
                for output in 0..4 {
                    for channel in 0..channels {
                        for index in 0..sample_count {
                            let actual = rust_outputs[output][channel][index];
                            let expected = cpp_outputs[output][channel][index];
                            if expected.is_finite() {
                                assert!((actual - expected).abs() <= 2.0e-6,
                                    "fixture={fixture_index} channel={channel} sample={index} output={output}: {actual} vs {expected}");
                            } else {
                                assert_eq!(actual.is_nan(), expected.is_nan());
                            }
                        }
                    }
                }
            }
        }
    }
}
