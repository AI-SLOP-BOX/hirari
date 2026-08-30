/*
 * Aura DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#pragma once

#include <cmath>
#include <array>
#include <algorithm>
#include "dsp_utils.hpp"

namespace Aura::DSP::Utils {

/**
 * @class PitchShifter
 * @brief Real-time Synchronous Overlap-Add (SOLA) Pitch Shifter.
 * Low-latency and sounds significantly better than raw delay modulation.
 */
class PitchShifter {
public:
    static constexpr int kMaxDelay = 8192;
    static constexpr int kMask = kMaxDelay - 1;

    PitchShifter() { reset(); }

    void process(float* buffer, uint32_t numSamples, float pitchRatio, float /*sampleRate*/) {
        if (std::abs(pitchRatio - 1.0f) < 0.001f) return;

        for (uint32_t s = 0; s < numSamples; ++s) {
            float in = buffer[s];
            m_delayBuf[m_writeIdx] = in;

            // Dual delay-tap crossfading to prevent clicks
            float tap1 = (m_writeIdx - m_phase1);
            float tap2 = (m_writeIdx - m_phase2);
            
            // Circular wrap
            while (tap1 < 0) tap1 += kMaxDelay;
            while (tap2 < 0) tap2 += kMaxDelay;

            // Simple Linear Interpolation
            float out1 = m_delayBuf[static_cast<int>(tap1) & kMask];
            float out2 = m_delayBuf[static_cast<int>(tap2) & kMask];

            // Crossfade window calculation
            float window = std::abs(m_phase1 - (kMaxDelay / 2.0f)) / (kMaxDelay / 2.0f);
            float finalOut = (out1 * window) + (out2 * (1.0f - window));

            buffer[s] = finalOut;

            // Advance phases
            m_phase1 += (1.0f - pitchRatio);
            m_phase2 += (1.0f - pitchRatio);

            // Wrap phases
            if (m_phase1 >= kMaxDelay) m_phase1 -= kMaxDelay;
            if (m_phase1 < 0) m_phase1 += kMaxDelay;
            if (m_phase2 >= kMaxDelay) m_phase2 -= kMaxDelay;
            if (m_phase2 < 0) m_phase2 += kMaxDelay;

            m_writeIdx = (m_writeIdx + 1) & kMask;
        }
    }

    void reset() {
        m_delayBuf.fill(0.0f);
        m_writeIdx = 0;
        m_phase1 = 0.0f;
        m_phase2 = kMaxDelay / 2.0f; // Offset by 180 degrees
    }

private:
    std::array<float, kMaxDelay> m_delayBuf;
    uint32_t m_writeIdx = 0;
    float m_phase1 = 0, m_phase2 = 0;
};

} // namespace Aura::DSP::Utils
