#include <algorithm>
#include <cmath>
#include <cstdint>

namespace {
struct TransientShaperReference {
    double sampleRate = 44'100.0;
    float attack = 0.0f;
    float sustain = 0.0f;
    float attackEnv = 0.0f;
    float sustainEnv = 0.0f;
    float attackAlpha = 0.9f;
    float sustainAlpha = 0.99f;
    float currentGain = 1.0f;

    void prepare(double sr) {
        sampleRate = sr;
        attackAlpha = std::exp(-1.0f / (sampleRate * 0.005f));
        sustainAlpha = std::exp(-1.0f / (sampleRate * 0.050f));
    }

    void reset() {
        attackEnv = 0.0f;
        sustainEnv = 0.0f;
        currentGain = 1.0f;
    }

    void process(float* left, float* right, uint32_t frames) {
        if (!left || frames == 0) return;
        const float attackGain = std::clamp(1.0f + attack, 0.0f, 2.0f);
        const float sustainGain = std::clamp(1.0f + sustain, 0.0f, 2.0f);
        for (uint32_t frame = 0; frame < frames; ++frame) {
            const float inL = left[frame];
            const float inR = right ? right[frame] : inL;
            const float level = std::max(std::abs(inL), std::abs(inR));
            attackEnv = attackAlpha * attackEnv + (1.0f - attackAlpha) * level;
            sustainEnv = sustainAlpha * sustainEnv + (1.0f - sustainAlpha) * level;
            const float transient = std::clamp(attackEnv - sustainEnv, -1.0f, 1.0f);
            const float body = std::clamp(sustainEnv, 0.0f, 1.0f);
            const float gain = std::clamp(
                1.0f + transient * (attackGain - 1.0f) + body * (sustainGain - 1.0f),
                0.0f, 3.0f);
            left[frame] = inL * gain;
            if (right) right[frame] = inR * gain;
        }
    }
};
} // namespace

extern "C" void* hirari_transient_shaper_reference_create(double sampleRate) {
    auto* state = new TransientShaperReference;
    (void)sampleRate; // The native default constructor starts at 44.1 kHz.
    return state;
}

extern "C" void hirari_transient_shaper_reference_destroy(void* state) {
    delete static_cast<TransientShaperReference*>(state);
}

extern "C" void hirari_transient_shaper_reference_prepare(void* state, double sampleRate) {
    if (state) static_cast<TransientShaperReference*>(state)->prepare(sampleRate);
}

extern "C" void hirari_transient_shaper_reference_reset(void* state) {
    if (state) static_cast<TransientShaperReference*>(state)->reset();
}

extern "C" void hirari_transient_shaper_reference_set_parameters(
    void* state, float attack, float sustain) {
    if (!state) return;
    auto& reference = *static_cast<TransientShaperReference*>(state);
    reference.attack = attack;
    reference.sustain = sustain;
}

extern "C" void hirari_transient_shaper_reference_process(
    void* state, float* left, float* right, uint32_t frames) {
    if (state) static_cast<TransientShaperReference*>(state)->process(left, right, frames);
}
