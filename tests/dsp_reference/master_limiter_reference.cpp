#include <algorithm>
#include <cmath>
#include <cstdint>
#include <new>

namespace {
constexpr uint32_t kMaxLookahead = 2048;
constexpr uint32_t kMask = kMaxLookahead - 1;

struct MasterLimiterReference {
    explicit MasterLimiterReference(double rate)
        : sampleRate(std::isfinite(rate) && rate >= 8'000.0 && rate <= 384'000.0
              ? rate : 44'100.0) {}

    double sampleRate;
    float delayLeft[kMaxLookahead]{};
    float delayRight[kMaxLookahead]{};
    uint32_t writeIndex = 0;
    float currentGain = 1.0f;
};
}

extern "C" void* hirari_master_limiter_reference_create(double sample_rate) {
    return new (std::nothrow) MasterLimiterReference(sample_rate);
}

extern "C" void hirari_master_limiter_reference_destroy(void* state) {
    delete static_cast<MasterLimiterReference*>(state);
}

extern "C" void hirari_master_limiter_reference_process(
    void* opaque,
    float* left,
    float* right,
    size_t frames,
    bool has_right,
    float threshold_gain,
    float ceiling,
    float release_ms,
    float lookahead_ms) {
    auto* state = static_cast<MasterLimiterReference*>(opaque);
    if (!state || !left || (has_right && !right)) return;

    const double sampleRate = state->sampleRate > 0.0 ? state->sampleRate : 44'100.0;
    release_ms = std::isfinite(release_ms) ? std::clamp(release_ms, 1.0f, 1000.0f) : 50.0f;
    float releaseCoefficient = std::exp(-1.0f / (release_ms * 0.001f * static_cast<float>(sampleRate)));
    if (!std::isfinite(releaseCoefficient)) releaseCoefficient = 0.99f;
    releaseCoefficient = std::clamp(releaseCoefficient, 0.0f, 0.9999f);
    threshold_gain = std::isfinite(threshold_gain)
        ? std::clamp(threshold_gain, 0.001f, 15.8489f) : 1.0f;
    ceiling = std::isfinite(ceiling) ? std::clamp(ceiling, 0.001f, 1.0f) : 0.99f;
    lookahead_ms = std::isfinite(lookahead_ms)
        ? std::clamp(lookahead_ms, 0.0f, 20.0f) : 2.0f;
    uint32_t lookaheadSamples = static_cast<uint32_t>(std::round(
        lookahead_ms * 0.001f * static_cast<float>(sampleRate)));
    lookaheadSamples = std::clamp<uint32_t>(lookaheadSamples, 0, kMaxLookahead - 1);

    for (size_t frame = 0; frame < frames; ++frame) {
        const float inputLeft = std::isfinite(left[frame]) ? left[frame] * threshold_gain : 0.0f;
        const float inputRight = has_right && std::isfinite(right[frame])
            ? right[frame] * threshold_gain : inputLeft;
        state->delayLeft[state->writeIndex] = inputLeft;
        state->delayRight[state->writeIndex] = inputRight;
        const uint32_t readIndex = (state->writeIndex + kMaxLookahead - lookaheadSamples) & kMask;
        const float outputLeft = state->delayLeft[readIndex];
        const float outputRight = state->delayRight[readIndex];

        float peak = 0.0f;
        const uint32_t window = lookaheadSamples + 1;
        for (uint32_t ahead = 0; ahead < window; ++ahead) {
            const uint32_t index = (readIndex + ahead) & kMask;
            const uint32_t previous = (index + kMaxLookahead - 1) & kMask;
            peak = std::max(peak, std::max(std::abs(state->delayLeft[index]),
                                           std::abs(state->delayRight[index])));
            peak = std::max(peak, std::max(
                std::abs(0.5f * (state->delayLeft[previous] + state->delayLeft[index])),
                std::abs(0.5f * (state->delayRight[previous] + state->delayRight[index]))));
        }
        state->writeIndex = (state->writeIndex + 1) & kMask;
        float targetAttenuation = peak > ceiling ? ceiling / (peak + 1.0e-6f) : 1.0f;
        if (!std::isfinite(targetAttenuation)) targetAttenuation = 1.0f;
        if (targetAttenuation < state->currentGain) {
            state->currentGain = targetAttenuation;
        } else {
            state->currentGain = state->currentGain * releaseCoefficient
                + targetAttenuation * (1.0f - releaseCoefficient);
        }
        if (!std::isfinite(state->currentGain)) state->currentGain = 1.0f;

        const float limitedLeft = outputLeft * state->currentGain;
        left[frame] = std::isfinite(limitedLeft)
            ? std::clamp(limitedLeft, -ceiling, ceiling) : 0.0f;
        if (has_right) {
            const float limitedRight = outputRight * state->currentGain;
            right[frame] = std::isfinite(limitedRight)
                ? std::clamp(limitedRight, -ceiling, ceiling) : 0.0f;
        }
    }
}
