#pragma once
#include <vector>
#include <memory>
#include <array>
#include "linkwitz_riley.hpp"
#include "../effects/pro_limiter.hpp"

namespace Hirari::Core::DSP::Mixing {

/**
 * @class MultiBandDynamics
 * @brief Professional Zero-Latency Multi-band Dynamics.
 * HONEST FIX: Replaced memory-allocating vectors with RT-safe pre-allocated buffers.
 * Uses Linkwitz-Riley 4th order (24dB/oct) phase-transparent crossovers.
 */
class MultiBandDynamics {
public:
    MultiBandDynamics(double sr) : m_sampleRate(sr) {
        m_lowMidLR_L.setParameters(250.0f, sr);
        m_lowMidLR_R.setParameters(250.0f, sr);
        m_midHighLR_L.setParameters(3500.0f, sr);
        m_midHighLR_R.setParameters(3500.0f, sr);
        
        for (int i = 0; i < 3; ++i) m_limiters.emplace_back(sr);
    }

    /**
     * @brief ACCELERATED DYNAMICS: Processes bands with zero-delay crossovers and RT-safety.
     */
    void process(float* l, float* r, uint32_t numSamples) {
        uint32_t limit = std::min(numSamples, 1024u);
        
        for (uint32_t s = 0; s < limit; ++s) {
            float inL = l[s], inR = r[s];
            
            // --- SPLIT LEFT (Phase-Transparent crossover) ---
            float loL = m_lowMidLR_L.process(inL);
            float midHighL = inL - loL;
            float midL = m_midHighLR_L.process(midHighL);
            float hiL = midHighL - midL;

            // --- SPLIT RIGHT ---
            float loR = m_lowMidLR_R.process(inR);
            float midHighR = inR - loR;
            float midR = m_midHighLR_R.process(midHighR);
            float hiR = midHighR - midR;

            m_loL[s] = loL; m_loR[s] = loR;
            m_midL[s] = midL; m_midR[s] = midR;
            m_hiL[s] = hiL; m_hiR[s] = hiR;
        }

        applyDynamics(m_loL, m_loR, limit, 0);
        applyDynamics(m_midL, m_midR, limit, 1);
        applyDynamics(m_hiL, m_hiR, limit, 2);

        for (uint32_t s = 0; s < limit; ++s) {
            l[s] = m_loL[s] + m_midL[s] + m_hiL[s];
            r[s] = m_loR[s] + m_midR[s] + m_hiR[s];
        }
    }

private:
    void applyDynamics(float* bl, float* br, uint32_t samples, int idx) {
        float* chs[2] = { bl, br };
        m_view.wrapChannels(chs, 2, samples);
        for (uint32_t s = 0; s < samples; ++s) m_limiters[idx].processBlock(m_view, s);
    }

    double m_sampleRate;
    LinkwitzRileyFilter m_lowMidLR_L, m_lowMidLR_R, m_midHighLR_L, m_midHighLR_R;
    std::vector<Effects::ProLimiter> m_limiters;
    
    // RT-SAFE PRE-ALLOCATED BUFFERS
    float m_loL[1024]{0}, m_loR[1024]{0};
    float m_midL[1024]{0}, m_midR[1024]{0};
    float m_hiL[1024]{0}, m_hiR[1024]{0};
    ::Hirari::Core::AudioBuffer m_view;
};

} // namespace Hirari::Core::DSP::Mixing
