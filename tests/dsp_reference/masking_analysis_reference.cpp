#include <algorithm>
#include <cmath>
#include <complex>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace {
void legacy_forward(std::vector<std::complex<float>>& values) {
    const size_t size = values.size();
    for (size_t i = 1, j = 0; i < size; ++i) {
        size_t bit = size >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap(values[i], values[j]);
    }
    for (size_t length = 2; length <= size; length <<= 1) {
        const size_t step = size / length;
        for (size_t start = 0; start < size; start += length) {
            for (size_t offset = 0; offset < length / 2; ++offset) {
                const size_t lut_index = (512 - step * offset) % 512;
                const double angle = 2.0 * 3.14159265358979323846 * lut_index / 512.0;
                const std::complex<float> twiddle(
                    static_cast<float>(std::cos(angle)),
                    -static_cast<float>(std::sin(angle)));
                const auto even = values[start + offset];
                const auto odd = values[start + offset + length / 2] * twiddle;
                values[start + offset] = even + odd;
                values[start + offset + length / 2] = even - odd;
            }
        }
    }
}
}

extern "C" size_t hirari_masking_analysis_reference(
    const float* target_mono,
    const float* other_mono,
    const uint32_t* other_track_ids,
    size_t other_count,
    size_t frame_count,
    uint32_t* output_bins,
    float* output_intensities,
    uint32_t* output_track_ids,
    size_t output_capacity) {
    if (!target_mono || !output_bins || !output_intensities || !output_track_ids ||
        frame_count != 512 || output_capacity == 0 ||
        (other_count && (!other_mono || !other_track_ids))) return 0;

    constexpr size_t bins = 256;
    std::vector<std::complex<float>> target(512);
    for (size_t i = 0; i < 512; ++i) target[i] = {target_mono[i], 0.0f};
    legacy_forward(target);
    size_t written = 0;
    std::vector<std::complex<float>> other(512);
    for (size_t track = 0; track < other_count; ++track) {
        for (size_t i = 0; i < 512; ++i) other[i] = {other_mono[track * 512 + i], 0.0f};
        legacy_forward(other);
        for (size_t bin = 0; bin < bins; ++bin) {
            const float intensity = std::abs(target[bin]) * std::abs(other[bin]);
            if (intensity > 0.02f) {
                output_bins[written] = static_cast<uint32_t>(bin);
                output_intensities[written] = intensity;
                output_track_ids[written] = other_track_ids[track];
                if (++written == output_capacity) return written;
            }
        }
    }
    return written;
}
