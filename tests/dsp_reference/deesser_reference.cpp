#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>

namespace {
struct DeEsserReference {
    double sampleRate = 44'100.0;
    float threshold = 0.5f;
    float intensity = 0.8f;
    float envelope = 0.0f;
    float currentGain = 1.0f;
    float z1 = 0.0f;
    float z2 = 0.0f;
    float b0 = 1.0f;
    float b1 = 0.0f;
    float b2 = 0.0f;
    float a1 = 0.0f;
    float a2 = 0.0f;

    void prepare(double sr) {
        if (!std::isfinite(sr) || sr < 100.0 || sr > 384'000.0) return;
        sampleRate = sr;
        const double w0 = 2.0 * 3.14159265358979323846 * 6000.0 / sampleRate;
        const double alpha = std::sin(w0) / 2.0;
        const double a0 = 1.0 + alpha;
        b0 = static_cast<float>(alpha / a0);
        b1 = 0.0f;
        b2 = static_cast<float>(-alpha / a0);
        a1 = static_cast<float>(-2.0 * std::cos(w0) / a0);
        a2 = static_cast<float>((1.0 - alpha) / a0);
    }

    void reset() {
        envelope = 0.0f;
        currentGain = 1.0f;
        z1 = z2 = 0.0f;
    }

    float filter(float input) {
        const float output = b0 * input + b1 * z1 + b2 * z2 - a1 * z1 - a2 * z2;
        z2 = z1;
        z1 = output;
        return output;
    }
};
} // namespace

extern "C" void* hirari_deesser_reference_create() {
    return new DeEsserReference;
}

extern "C" void hirari_deesser_reference_destroy(void* state) {
    delete static_cast<DeEsserReference*>(state);
}

extern "C" void hirari_deesser_reference_prepare(void* state, double sampleRate) {
    if (state) static_cast<DeEsserReference*>(state)->prepare(sampleRate);
}

extern "C" void hirari_deesser_reference_reset(void* state) {
    if (state) static_cast<DeEsserReference*>(state)->reset();
}

extern "C" void hirari_deesser_reference_set_parameters(
    void* state, float threshold, float intensity) {
    if (!state) return;
    auto& reference = *static_cast<DeEsserReference*>(state);
    reference.threshold = threshold;
    reference.intensity = intensity;
}

extern "C" uint32_t hirari_deesser_reference_tail_samples(const void* state) {
    if (!state) return 0;
    const auto& reference = *static_cast<const DeEsserReference*>(state);
    const double sr = std::clamp(reference.sampleRate, 100.0, 384'000.0);
    return static_cast<uint32_t>(std::min(30.0 * sr, 0.32 * sr));
}

extern "C" void hirari_deesser_reference_process(
    void* state, float* left, float* right, uint32_t frames) {
    if (!state || !left || frames == 0) return;
    auto& reference = *static_cast<DeEsserReference*>(state);
    const float threshold = std::clamp(reference.threshold, 0.001f, 1.0f);
    const float intensity = std::clamp(reference.intensity, 0.0f, 2.0f);
    for (uint32_t frame = 0; frame < frames; ++frame) {
        const float inLeft = std::isfinite(left[frame]) ? left[frame] : 0.0f;
        const float inRight = right && std::isfinite(right[frame]) ? right[frame] : inLeft;
        const float signal = reference.filter(0.5f * (inLeft + inRight));
        const float level = std::abs(signal);
        if (level > reference.envelope) {
            reference.envelope = 0.9f * reference.envelope + 0.1f * level;
        } else {
            reference.envelope = 0.999f * reference.envelope + 0.001f * level;
        }
        if (std::abs(reference.envelope) < 1.0e-24f) reference.envelope = 0.0f;
        const float excess = std::max(0.0f, reference.envelope - threshold);
        const float targetGain = 1.0f / (1.0f + intensity * excess * 12.0f);
        reference.currentGain += (targetGain - reference.currentGain) * 0.08f;
        left[frame] = inLeft * reference.currentGain;
        if (right) right[frame] = inRight * reference.currentGain;
    }
}
