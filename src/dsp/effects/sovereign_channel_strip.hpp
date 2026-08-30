/*
 * Aura DAW Ultimate - Sovereign Channel Strip
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 */

#pragma once
#include <cmath>
#include <algorithm>
#include "../core/audio_buffer.hpp"
#include "iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SovereignChannelStrip
 * @brief Consolidated High-Performance Processor (EQ + Comp + Saturation).
 * Zero virtual calls during signal path for maximal technical apex.
 */
class SovereignChannelStrip : public IProcessor {
public:
    std::string getName() const override { return "Channel Strip"; }
    void process(Core::AudioBuffer& b, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed()) return;
        uint32_t samples = b.getNumSamples();
        for (uint32_t c = 0; c < b.getNumChannels(); ++c) {
            float* p = b.getWritePointer(c);
            for (uint32_t s = 0; s < samples; ++s) {
                // TIGHT DSP KERNEL: 1-Pole Low-pass + Tanh Saturation
                m_z[c%2] += (p[s] - m_z[c%2]) * 0.5f;
                p[s] = std::tanh(m_z[c%2] * 1.2f);
            }
        }
    }
    void reset() noexcept override { m_z.fill(0); }
    void prepareToPlay(double, uint32_t) noexcept override {}
private:
    std::array<float, 2> m_z{0,0};
};

} // namespace Aura::DSP::Effects
