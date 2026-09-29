#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>

// Frozen equations from the pre-migration StemSplitter::split implementation.
extern "C" bool hirari_stem_split_reference(
    const float* const* inputs,
    float* const* drums,
    float* const* bass,
    float* const* vocals,
    float* const* other,
    uint32_t channels,
    uint32_t samples,
    double sample_rate) {
    constexpr uint32_t max_channels = 16;
    if (!inputs || !drums || !bass || !vocals || !other || channels == 0 ||
        channels > max_channels || samples == 0 || !std::isfinite(sample_rate) ||
        sample_rate <= 0.0) return false;

    const float bass_coefficient = static_cast<float>(
        std::clamp(80.0 / sample_rate, 0.001, 0.25));
    std::array<float, max_channels> bass_state{};
    std::array<float, max_channels> previous{};
    for (uint32_t channel = 0; channel < channels; ++channel) {
        if (!inputs[channel] || !drums[channel] || !bass[channel] ||
            !vocals[channel] || !other[channel]) return false;
        for (uint32_t index = 0; index < samples; ++index) {
            const float raw = inputs[channel][index];
            const float sample = std::isfinite(raw) ? raw : 0.0f;
            const float delta = std::abs(sample - previous[channel]);
            const float drum_mask = std::clamp(delta * 5.0f, 0.0f, 1.0f);
            bass_state[channel] += bass_coefficient * (sample - bass_state[channel]);
            const float bass_sample = bass_state[channel] * (1.0f - drum_mask);
            const float vocal_mask = std::clamp(
                1.0f - std::abs(sample - bass_state[channel]) /
                    (std::abs(sample) + 1.0e-6f), 0.0f, 1.0f);
            const float vocal_sample = (sample - bass_state[channel]) *
                (1.0f - drum_mask) * vocal_mask * vocal_mask;
            const float drum_sample = sample * drum_mask;
            drums[channel][index] = drum_sample;
            bass[channel][index] = bass_sample;
            vocals[channel][index] = vocal_sample;
            other[channel][index] = sample - drum_sample - bass_sample - vocal_sample;
            previous[channel] = sample;
        }
    }
    return true;
}
