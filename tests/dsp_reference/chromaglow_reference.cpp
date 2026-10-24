#include <algorithm>
#include <cmath>
#include <cstdint>

namespace {
struct ChromaGlowReference {
    double sampleRate = 44'100.0;
    float gain = 1.0f;
    float mix = 0.5f;
    float lastL = 0.0f;
    float lastR = 0.0f;
    uint32_t mode = 1;

    void prepare(double sr) {
        sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        reset();
    }

    void reset() { lastL = lastR = 0.0f; }

    void setParams(float driveDb, float character, uint32_t selectedMode) {
        const float db = std::clamp(std::isfinite(driveDb) ? driveDb : 0.0f, -24.0f, 36.0f);
        gain = std::pow(10.0f, db / 20.0f);
        mix = std::clamp(std::isfinite(character) ? character : 0.0f, 0.0f, 1.0f);
        mode = selectedMode;
    }

    void setParameter(uint32_t id, float value) {
        if (!std::isfinite(value)) return;
        if (id == 0) {
            gain = std::pow(10.0f, (-24.0f + std::clamp(value, 0.0f, 1.0f) * 60.0f) / 20.0f);
        } else if (id == 1) {
            mix = std::clamp(value, 0.0f, 1.0f);
        } else if (id == 2) {
            mode = static_cast<uint32_t>(std::clamp(static_cast<int>(std::lround(value * 2.0f)), 0, 2));
        }
    }

    float getParameter(uint32_t id) const {
        if (id == 0) return std::clamp((20.0f * std::log10(std::max(gain, 1.0e-6f)) + 24.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 1) return mix;
        return id == 2 ? static_cast<float>(mode) / 2.0f : 0.0f;
    }

    static float fastTanh(float x) {
        if (x > 3.0f) return 1.0f;
        if (x < -3.0f) return -1.0f;
        const float x2 = x * x;
        return x * (27.0f + x2) / (27.0f + 9.0f * x2);
    }

    float saturate(float x) const {
        switch (mode) {
        case 0: return x > 0.0f ? fastTanh(x) : x / (1.0f + std::abs(x));
        case 1: return fastTanh(x);
        case 2: return std::clamp((1.5f * x) * (1.0f - (x * x) / 3.0f), -1.0f, 1.0f);
        default: return x;
        }
    }

    void process(float* left, float* right, uint32_t frames) const {
        if (!left || !right || frames == 0) return;
        const float localGain = std::isfinite(gain) ? gain : 1.0f;
        const float localMix = std::clamp(std::isfinite(mix) ? mix : 0.0f, 0.0f, 1.0f);
        for (uint32_t i = 0; i < frames; ++i) {
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float dryR = std::isfinite(right[i]) ? right[i] : 0.0f;
            const float wetL = saturate(dryL * localGain);
            const float wetR = saturate(dryR * localGain);
            const float outL = dryL + localMix * (wetL - dryL);
            const float outR = dryR + localMix * (wetR - dryR);
            if (right == left) {
                const float mono = 0.5f * (outL + outR);
                left[i] = std::isfinite(mono) ? std::clamp(mono, -16.0f, 16.0f) : 0.0f;
            } else {
                left[i] = std::isfinite(outL) ? std::clamp(outL, -16.0f, 16.0f) : 0.0f;
                right[i] = std::isfinite(outR) ? std::clamp(outR, -16.0f, 16.0f) : 0.0f;
            }
        }
    }
};
} // namespace

extern "C" void* hirari_chromaglow_reference_create(double sampleRate) {
    auto* state = new ChromaGlowReference;
    if (!std::isfinite(sampleRate) || sampleRate < 8'000.0 || sampleRate > 384'000.0) {
        state->sampleRate = 44'100.0;
    } else {
        state->sampleRate = sampleRate;
    }
    return state;
}

extern "C" void hirari_chromaglow_reference_destroy(void* state) {
    delete static_cast<ChromaGlowReference*>(state);
}

extern "C" void hirari_chromaglow_reference_prepare(void* state, double sampleRate) {
    if (state) static_cast<ChromaGlowReference*>(state)->prepare(sampleRate);
}

extern "C" void hirari_chromaglow_reference_reset(void* state) {
    if (state) static_cast<ChromaGlowReference*>(state)->reset();
}

extern "C" void hirari_chromaglow_reference_set_params(
    void* state, float driveDb, float mix, uint32_t mode) {
    if (state) static_cast<ChromaGlowReference*>(state)->setParams(driveDb, mix, mode);
}

extern "C" void hirari_chromaglow_reference_set_parameter(
    void* state, uint32_t id, float value) {
    if (state) static_cast<ChromaGlowReference*>(state)->setParameter(id, value);
}

extern "C" float hirari_chromaglow_reference_get_parameter(const void* state, uint32_t id) {
    return state ? static_cast<const ChromaGlowReference*>(state)->getParameter(id) : 0.0f;
}

extern "C" void hirari_chromaglow_reference_process(
    void* state, float* left, float* right, uint32_t frames) {
    if (state) static_cast<const ChromaGlowReference*>(state)->process(left, right, frames);
}
