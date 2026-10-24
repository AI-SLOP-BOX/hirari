#include <cmath>
#include <cstdint>

extern "C" uint32_t hirari_builtin_gain_process_reference(
    float* const* channels,
    uint32_t channel_count,
    uint32_t sample_count,
    float gain) {
    if (!channels || !std::isfinite(gain)) return 0;
    uint32_t sanitized = 0;
    for (uint32_t channel = 0; channel < channel_count; ++channel) {
        float* samples = channels[channel];
        if (!samples) continue;
        for (uint32_t index = 0; index < sample_count; ++index) {
            const float value = samples[index] * gain;
            if (std::isfinite(value)) {
                samples[index] = value;
            } else {
                samples[index] = 0.0f;
                ++sanitized;
            }
        }
    }
    return sanitized;
}
