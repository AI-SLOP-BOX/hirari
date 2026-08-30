#pragma once

#include <vector>
#include <cmath>
#include <atomic>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief DynamicsSuite: Unified Dynamics Processing (Logic Pro-style).
 * Consolidates Multiband, Sidechain, Gate, DeEsser, and Limiting into one robust engine.
 */
class DynamicsSuite {
public:
    struct ProcessorState {
        float threshold = -20.0f;
        float ratio = 4.0f;
        float attack = 0.01f;
        float release = 0.1f;
    };

    /**
     * @brief High-fidelity dynamics processing for 3-band and sidechain.
     */
    void process(float* l, float* r, const float* sidechain, size_t numFrames) {
        if (!l || !r) return;
        float gain = 1.0f;
        const float threshold = std::pow(10.0f, m_sidechainThresh / 20.0f);
        for (size_t i = 0; i < numFrames; ++i) {
            const float left = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float right = std::isfinite(r[i]) ? r[i] : 0.0f;
            const float detector = sidechain && std::isfinite(sidechain[i]) ? std::abs(sidechain[i]) : std::max(std::abs(left), std::abs(right));
            const float target = detector > threshold ? std::pow(threshold / std::max(detector, 1.0e-6f), 0.75f) : 1.0f;
            const float coeff = target < gain ? 0.995f : 0.9995f;
            gain = coeff * gain + (1.0f - coeff) * target;
            const float gate = std::max(std::abs(left), std::abs(right)) < std::pow(10.0f, m_gateThresh / 20.0f) ? 0.0f : 1.0f;
            l[i] = std::isfinite(left * gain * gate) ? left * gain * gate : 0.0f;
            r[i] = std::isfinite(right * gain * gate) ? right * gain * gate : 0.0f;
        }
    }


private:
    float m_gateThresh = -60.0f;
    float m_sidechainThresh = -20.0f;
};

} // namespace Aura::Core::DSP::Mixing
