#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>
#include <cstdint>

extern "C" bool hirari_region_resampler_reference_kernel(float* output, size_t capacity) {
    constexpr size_t kValues = 17 * 64 * 8;
    if (output == nullptr || capacity < kValues) return false;
    constexpr double pi = 3.14159265358979323846;
    for (int cutoffIndex = 0; cutoffIndex < 17; ++cutoffIndex) {
        const double cutoff = std::exp2(-static_cast<double>(cutoffIndex) / 4.0);
        for (int phase = 0; phase < 64; ++phase) {
            const double fraction = static_cast<double>(phase) / 64.0;
            double sum = 0.0;
            std::array<double, 8> coefficients{};
            for (int tap = 0; tap < 8; ++tap) {
                const double x = static_cast<double>(tap - 3) - fraction;
                const double sinc = std::abs(x) < 1.0e-12
                    ? cutoff : std::sin(pi * cutoff * x) / (pi * x);
                const double window = std::abs(x) >= 4.0
                    ? 0.0
                    : (std::abs(x) < 1.0e-12
                        ? 1.0
                        : (std::sin(pi * x / 4.0) / (pi * x / 4.0)));
                coefficients[static_cast<size_t>(tap)] = sinc * window;
                sum += sinc * window;
            }
            const double normalizer = std::abs(sum) > 1.0e-12 ? 1.0 / sum : 1.0;
            const size_t base = static_cast<size_t>((cutoffIndex * 64 + phase) * 8);
            for (int tap = 0; tap < 8; ++tap) {
                output[base + static_cast<size_t>(tap)] = static_cast<float>(
                    coefficients[static_cast<size_t>(tap)] * normalizer);
            }
        }
    }
    return true;
}

extern "C" void hirari_region_read_warped_reference(
    const float* source, uint64_t sourceSamples, uint64_t sourceOffset,
    uint64_t sourceSpan, double position, uint8_t reverse,
    uint8_t allowSourcePreroll, double resampleStep, const float* kernel,
    float* output) {
    if (output == nullptr) return;
    output[0] = 0.0f;
    output[1] = 0.0f;
    if (source == nullptr || kernel == nullptr || !std::isfinite(position) ||
        sourceSamples == 0 || sourceSpan == 0 ||
        sourceSamples > static_cast<uint64_t>(INT64_MAX) ||
        sourceOffset > static_cast<uint64_t>(INT64_MAX)) return;
    const double floorPosition = std::floor(position);
    if (floorPosition < static_cast<double>(INT64_MIN) + 8192.0 ||
        floorPosition > static_cast<double>(INT64_MAX) - 8192.0) return;
    const int64_t relative = static_cast<int64_t>(floorPosition);
    const int64_t offset = static_cast<int64_t>(sourceOffset);
    const int64_t span = static_cast<int64_t>(sourceSpan);
    const int64_t direction = reverse != 0 ? -1 : 1;
    const int64_t sourceIndex = reverse != 0 ? span - 1 - relative : relative;
    const int64_t absoluteIndex = offset + sourceIndex;
    const int64_t regionLast = offset + span - 1;
    const float fraction = static_cast<float>(position - floorPosition);
    const auto tap = [&](int tapOffset) {
        const int64_t lower = allowSourcePreroll != 0 ? 0 : offset;
        const int64_t upper = allowSourcePreroll != 0
            ? static_cast<int64_t>(sourceSamples - 1) : regionLast;
        const int64_t index = std::clamp<int64_t>(
            absoluteIndex + direction * tapOffset, lower, upper);
        const float sample = source[index];
        return std::isfinite(sample) ? sample : 0.0f;
    };
    const float y0 = tap(0);
    const float y1 = tap(1);
    if (std::abs(resampleStep - 1.0) < 1.0e-6) {
        const float ym1 = tap(-1);
        const float y2 = tap(2);
        const float a = (-0.5f * ym1 + 1.5f * y0 - 1.5f * y1 + 0.5f * y2);
        const float b = (ym1 - 2.5f * y0 + 2.0f * y1 - 0.5f * y2);
        const float c = (-0.5f * ym1 + 0.5f * y1);
        output[0] = a * fraction * fraction * fraction + b * fraction * fraction +
                    c * fraction + y0;
    } else {
        const double safeStep = std::clamp(std::abs(resampleStep), 1.0 / 16.0, 16.0);
        const int cutoffIndex = std::clamp(
            static_cast<int>(std::ceil(std::log2(std::max(1.0, safeStep)) * 4.0)), 0, 16);
        const int phaseIndex = std::clamp(static_cast<int>(fraction * 64.0f), 0, 63);
        const size_t kernelBase = static_cast<size_t>((cutoffIndex * 64 + phaseIndex) * 8);
        for (int tapIndex = 0; tapIndex < 8; ++tapIndex) {
            output[0] += tap(tapIndex - 3) * kernel[kernelBase + static_cast<size_t>(tapIndex)];
        }
    }
    output[1] = y1 - y0;
}
