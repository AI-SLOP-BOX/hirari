#pragma once
#include <vector>
#include <cmath>
#include <random>
#include <algorithm>
#include <array>

namespace Aura::Core::DSP::Effects {

/**
 * @class ProceduralFoleyKernel
 * @brief Physical-modeling based Foley generator (Footsteps & Friction).
 * HONEST FIX: Modal resonance filters, transient noise bursts, and friction envelope trackers.
 */
class ProceduralFoleyKernel {
public:
    struct BiquadState {
        float x1 = 0.0f, x2 = 0.0f;
        float y1 = 0.0f, y2 = 0.0f;
        
        inline float process(float x, float b0, float b1, float b2, float a1, float a2) {
            float y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            if (std::abs(y) < 1.0e-15f) y = 0.0f;
            x2 = x1; x1 = x;
            y2 = y1; y1 = y;
            return y;
        }
        void reset() { x1 = x2 = y1 = y2 = 0.0f; }
    };

    // A fixed default seed keeps offline renders reproducible. Callers that
    // explicitly want variation can provide their own seed per take.
    explicit ProceduralFoleyKernel(uint32_t seed = 0xA0F0E1u) : m_noiseGen(seed) {
        resetFilters();
    }

    /**
     * @brief Generates a single Foley event (e.g. footstep).
     */
    void triggerEvent(float hardness, float friction) {
        m_hardness = std::isfinite(hardness) ? std::clamp(hardness, 0.0f, 1.0f) : 0.5f;
        m_friction = std::isfinite(friction) ? std::clamp(friction, 0.0f, 1.0f) : 0.0f;

        // Strike impulse envelope setting
        m_strikeEnv = 1.0f;
        m_strikeDecay = 0.992f - (m_hardness * 0.005f); // Faster decay for hard strikes

        // Scale modal resonators based on hardness
        setupResonators();
    }

    void process(float* buffer, uint32_t numSamples) {
        if (!buffer || numSamples == 0) return;

        for (uint32_t s = 0; s < numSamples; ++s) {
            float noise = m_noiseDist(m_noiseGen);
            
            // 1. Strike Impulse Component
            float strikeSignal = 0.0f;
            if (m_strikeEnv > 0.0001f) {
                // Hard strike has sharper noise transient
                float noiseBurst = noise * m_strikeEnv * (0.3f + m_hardness * 0.7f);
                
                // Pass noise burst through the three modal resonators
                float r1 = m_modes[0].process(noiseBurst, m_c[0][0], m_c[0][1], m_c[0][2], m_c[0][3], m_c[0][4]);
                float r2 = m_modes[1].process(noiseBurst, m_c[1][0], m_c[1][1], m_c[1][2], m_c[1][3], m_c[1][4]);
                float r3 = m_modes[2].process(noiseBurst, m_c[2][0], m_c[2][1], m_c[2][2], m_c[2][3], m_c[2][4]);
                
                strikeSignal = r1 * 0.5f + r2 * 0.3f + r3 * 0.2f;
                m_strikeEnv *= m_strikeDecay;
            }

            // 2. Friction Component (Sustained clothing swish / scraping)
            float frictionSignal = 0.0f;
            if (m_friction > 0.01f) {
                // Low-pass filtered noise to simulate cloth rub
                m_frictionEnv += (m_friction - m_frictionEnv) * 0.001f; // Slow smoothing
                
                // Standard 1st-order LPF
                m_frictionLP += (noise - m_frictionLP) * 0.05f;
                frictionSignal = m_frictionLP * m_frictionEnv * 0.15f;
            }

            float finalOut = strikeSignal + frictionSignal;
            if (!std::isfinite(finalOut)) finalOut = 0.0f;

            // Add Foley sound to output buffer
            buffer[s] += finalOut;
        }
    }

private:
    void resetFilters() {
        for (auto& mode : m_modes) mode.reset();
        m_hardness = 0.5f;
        m_friction = 0.0f;
        m_strikeDecay = 0.99f;
        m_frictionLP = 0.0f;
        m_strikeEnv = 0.0f;
        m_frictionEnv = 0.0f;
    }

    void setupResonators() {
        // Wooden floor/concrete modal frequencies
        const float f1 = 80.0f + m_hardness * 40.0f;
        const float f2 = 220.0f + m_hardness * 80.0f;
        const float f3 = 800.0f + m_hardness * 400.0f;

        // Bandpass Q factor (higher Q means longer ringing resonance)
        const float q1 = 12.0f;
        const float q2 = 8.0f;
        const float q3 = 4.0f;

        m_c[0] = computeBPF(f1, q1);
        m_c[1] = computeBPF(f2, q2);
        m_c[2] = computeBPF(f3, q3);
    }

    std::array<float, 5> computeBPF(float freq, float q) {
        float w0 = 2.0f * static_cast<float>(M_PI) * freq / 44100.0f;
        float alpha = std::sin(w0) / (2.0f * q);
        float cosw0 = std::cos(w0);
        float a0 = 1.0f + alpha;

        float b0 = alpha / a0;
        float b1 = 0.0f;
        float b2 = -alpha / a0;
        float a1 = (-2.0f * cosw0) / a0;
        float a2 = (1.0f - alpha) / a0;
        return {b0, b1, b2, a1, a2};
    }

    float m_hardness = 0.5f;
    float m_friction = 0.0f;
    float m_strikeEnv = 0.0f;
    float m_strikeDecay = 0.99f;
    float m_frictionEnv = 0.0f;
    float m_frictionLP = 0.0f;

    std::array<BiquadState, 3> m_modes;
    std::array<std::array<float, 5>, 3> m_c; // Coefficients for resonators

    std::mt19937 m_noiseGen;
    std::uniform_real_distribution<float> m_noiseDist{-1.0f, 1.0f};
};

} // namespace Aura::Core::DSP::Effects
