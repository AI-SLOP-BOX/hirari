#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_peak_envelope_reference(
    left: *const f32,
    right: *const f32,
    sample_count: usize,
    peak_count: u32,
    output: *mut f32,
    capacity: usize,
) -> usize {
    if left.is_null()
        || right.is_null()
        || output.is_null()
        || peak_count == 0
        || sample_count == 0
        || capacity < peak_count as usize * 2
    {
        return 0;
    }
    for index in 0..peak_count as usize {
        let begin = (index as u64 * sample_count as u64 / peak_count as u64) as usize;
        let end = std::cmp::max(
            begin + 1,
            ((index as u64 + 1) * sample_count as u64 / peak_count as u64) as usize,
        );
        let mut maximum = 0.0f32;
        let mut minimum = 0.0f32;
        let limit = std::cmp::min(end as u64, sample_count as u64) as usize;
        for sample in begin..limit {
            for value in [*left.add(sample), *right.add(sample)] {
                if !value.is_finite() {
                    continue;
                }
                maximum = maximum.max(value);
                minimum = minimum.min(value);
            }
        }
        *output.add(index * 2) = maximum;
        *output.add(index * 2 + 1) = minimum;
    }
    peak_count as usize * 2
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_peak_resample_reference(
    peaks: *const f32,
    cached_count: usize,
    peak_count: u32,
    output: *mut f32,
    capacity: usize,
) -> usize {
    if peaks.is_null()
        || output.is_null()
        || cached_count == 0
        || peak_count == 0
        || capacity < peak_count as usize * 2
    {
        return 0;
    }
    for index in 0..peak_count as usize {
        let source_index = (index * cached_count / peak_count as usize) * 2;
        *output.add(index * 2) = *peaks.add(source_index);
        *output.add(index * 2 + 1) = *peaks.add(source_index + 1);
    }
    peak_count as usize * 2
}
