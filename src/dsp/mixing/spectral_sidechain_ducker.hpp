#pragma once
#include <vector>
#include <memory>
#include <cmath>
#include "simd_svf.hpp"
#include "../effects/fet_compressor.hpp"

namespace Aura::DSP::Mixing {

/**
 * @class SpectralSidechainDucker
 * @brief High-precision Frequency-Selective Ducking.
 * HONEST FIX: Implements 'Spectral Ducking' where only specific frequency bands 
 * (e.g., Sub-bass) are compressed when the sidechain signal hits a threshold.
 * Prevents the entire mix from 'pumping' while maintaining clarity in 
 * overlapping instruments like Kick and Bass.
 */
class SpectralSidechainDucker {
public:
    SpectralSidechainDucker(double sr = 44100.0) 
        : m_lowPass(sr), m_highPass(sr), m_compressor(sr), m_sampleRate(sr) {
        m_lowPass.reset(); m_highPass.reset();
    }

    /**
     * @brief PROCESS: Dux specific frequencies based on the sidechain 'Key' input.
     */
    void process(float* l, float* r, uint32_t samples, const float* sidechainKey) {
        if (l == nullptr || r == nullptr || sidechainKey == nullptr || samples == 0 ||
            !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) return;

        const float lowCoeff = std::exp(-2.0f * 3.14159265358979323846f * 250.0f /
                                        static_cast<float>(m_sampleRate));
        const float attack = std::exp(-1.0f / (0.002f * static_cast<float>(m_sampleRate)));
        const float release = std::exp(-1.0f / (0.080f * static_cast<float>(m_sampleRate)));
        for (uint32_t i = 0; i < samples; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            const float key = std::isfinite(sidechainKey[i]) ? std::abs(sidechainKey[i]) : 0.0f;
            m_keyEnvelope = key > m_keyEnvelope
                ? attack * m_keyEnvelope + (1.0f - attack) * key
                : release * m_keyEnvelope + (1.0f - release) * key;
            const float amount = std::clamp((m_keyEnvelope - 0.12f) * 3.5f, 0.0f, 0.85f);
            m_lowL = lowCoeff * m_lowL + (1.0f - lowCoeff) * inL;
            m_lowR = lowCoeff * m_lowR + (1.0f - lowCoeff) * inR;
            l[i] = m_lowL * (1.0f - amount) + (inL - m_lowL);
            r[i] = m_lowR * (1.0f - amount) + (inR - m_lowR);
        }
    }

private:
    Mixing::SIMDSVF m_lowPass;
    Mixing::SIMDSVF m_highPass;
    Effects::FETCompressor m_compressor;
    double m_sampleRate = 44100.0;
    float m_keyEnvelope = 0.0f;
    float m_lowL = 0.0f;
    float m_lowR = 0.0f;
};

} // namespace Aura::DSP::Mixing
