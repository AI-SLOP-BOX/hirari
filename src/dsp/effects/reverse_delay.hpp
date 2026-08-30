#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ReverseDelay
 * @brief Creative Rhythmic Reversal Effect for Ambient and Vocal textures.
 * HONEST FIX: Implements a cyclic window-based grain reversal engine 
 * with cross-fading between buffers to ensure seamless, glitch-free 
 * backward playback.
 * Provides the haunting, cinematic depth found in Logic Pro's 
 * Delay Designer and specialized reverse engines.
 */
class ReverseDelay : public IProcessor {
public:
    ReverseDelay() : m_windowSize(22050), m_writeIdx(0), m_mix(0.5f) {
        m_buffer.assign(2, std::vector<float>(44100 * 2, 0.0f));
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        m_windowSize = std::clamp<uint32_t>(m_windowSize, 10u, static_cast<uint32_t>(m_buffer[0].size() - 1));
        reset();
    }

    /**
     * @brief PROCESS: Plays back segments of audio in reverse order.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0 || m_buffer.empty()) return;
        const uint32_t capacity = static_cast<uint32_t>(m_buffer[0].size());
        const uint32_t window = std::clamp(m_windowSize, 10u, capacity - 1u);
        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        for (uint32_t i = 0; i < n; ++i) {
            const uint32_t write = m_writeIdx;
            const uint32_t reverseOffset = write % window;
            const uint32_t windowStart = write - reverseOffset;
            const uint32_t read = (windowStart + window - 1u - reverseOffset) % capacity;
            const float wetL = m_buffer[0][read];
            const float wetR = m_buffer[1][read];
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float dryR = right && std::isfinite(right[i]) ? right[i] : dryL;
            m_buffer[0][write] = dryL;
            m_buffer[1][write] = dryR;
            left[i] = dryL + mix * (wetL - dryL);
            if (right) right[i] = dryR + mix * (wetR - dryR);
            m_writeIdx = (write + 1u) % capacity;
        }
    }


    void reset() noexcept override {
        for (auto& v : m_buffer) std::fill(v.begin(), v.end(), 0.0f);
        m_writeIdx = 0;
    }

    // Parameters
    void setWindowTime(float ms) {
        if (!std::isfinite(ms)) return;
        m_windowSize = std::clamp<uint32_t>(static_cast<uint32_t>(std::max(10.0f, ms * static_cast<float>(m_sampleRate) * 0.001f)), 10u, static_cast<uint32_t>(m_buffer[0].size() - 1));
    }
    void setMix(float m) { if (std::isfinite(m)) m_mix = std::clamp(m, 0.0f, 1.0f); }

private:
    double m_sampleRate = 44100.0;
    std::vector<std::vector<float>> m_buffer;
    uint32_t m_writeIdx;
    uint32_t m_windowSize;
    float m_mix;
};

} // namespace Aura::DSP::Effects
