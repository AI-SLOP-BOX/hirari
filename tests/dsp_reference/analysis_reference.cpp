#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

extern "C" size_t hirari_waveform_peak_envelope_reference(
    const float* left, const float* right, size_t sample_count, uint32_t peak_count,
    float* output, size_t capacity) {
    if (!left || !right || !output || peak_count == 0 || sample_count == 0 ||
        capacity < static_cast<size_t>(peak_count) * 2) return 0;
    for (uint32_t index = 0; index < peak_count; ++index) {
        const uint32_t begin = static_cast<uint32_t>(
            (static_cast<uint64_t>(index) * sample_count) / peak_count);
        const uint32_t end = std::max(
            begin + 1u,
            static_cast<uint32_t>((static_cast<uint64_t>(index + 1u) * sample_count) / peak_count));
        float maximum = 0.0f;
        float minimum = 0.0f;
        for (uint32_t sample = begin; sample < std::min<size_t>(end, sample_count); ++sample) {
            for (float value : {left[sample], right[sample]}) {
                if (!std::isfinite(value)) continue;
                maximum = std::max(maximum, value);
                minimum = std::min(minimum, value);
            }
        }
        output[index * 2] = maximum;
        output[index * 2 + 1] = minimum;
    }
    return static_cast<size_t>(peak_count) * 2;
}

extern "C" size_t hirari_waveform_peak_resample_reference(
    const float* peaks, size_t cached_count, uint32_t peak_count,
    float* output, size_t capacity) {
    if (!peaks || !output || cached_count == 0 || peak_count == 0 ||
        capacity < static_cast<size_t>(peak_count) * 2) return 0;
    for (uint32_t index = 0; index < peak_count; ++index) {
        const size_t source_index =
            (static_cast<size_t>(index) * cached_count / peak_count) * 2;
        output[index * 2] = peaks[source_index];
        output[index * 2 + 1] = peaks[source_index + 1];
    }
    return static_cast<size_t>(peak_count) * 2;
}
