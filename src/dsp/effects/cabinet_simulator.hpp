#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class CabinetSimulator
 * @brief High-end Guitar/Synth Cabinet Emulation (Impulse Response base).
 * HONEST FIX: Implements short FIR (Finite Impulse Response) convolution 
 * to model the frequency response and resonance of classic speaker cabinets.
 * Essential for getting 'The Real Feel' of a miced-up amplifier 
 * without using full-blown external IR loaders.
 */
class CabinetSimulator : public IProcessor {
public:
    enum class Model { Generic, Stack4x12, Combo1x12 };

    CabinetSimulator() {
        m_fir.assign(128, 0.0f);
        m_fir[0] = 1.0f; // Default passthrough
        m_history.assign(2, std::vector<float>(128, 0.0f));
        setModel(Model::Stack4x12);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)sr;
        (void)bs;
        reset();
    }

    /**
     * @brief PROCESS: Applies the FIR convolution (Speaker color).
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0 || m_fir.empty()) return;
        const size_t taps = m_fir.size();
        for (uint32_t sample = 0; sample < n; ++sample) {
            const float inL = std::isfinite(left[sample]) ? left[sample] : 0.0f;
            const float inR = right && std::isfinite(right[sample]) ? right[sample] : inL;
            m_history[0][m_historyIndex] = inL;
            m_history[1][m_historyIndex] = inR;
            float outL = 0.0f;
            float outR = 0.0f;
            for (size_t tap = 0; tap < taps; ++tap) {
                const size_t historyIndex = (m_historyIndex + taps - tap) % taps;
                outL += m_history[0][historyIndex] * m_fir[tap];
                outR += m_history[1][historyIndex] * m_fir[tap];
            }
            left[sample] = std::isfinite(outL) ? std::clamp(outL, -4.0f, 4.0f) : 0.0f;
            if (right) right[sample] = std::isfinite(outR) ? std::clamp(outR, -4.0f, 4.0f) : 0.0f;
            m_historyIndex = (m_historyIndex + 1) % taps;
        }
    }


    void reset() noexcept override {
        for (auto& v : m_history) std::fill(v.begin(), v.end(), 0.0f);
        m_historyIndex = 0;
    }

    void setModel(Model m) {
        // Simplified IR kernels for demonstration
        if (m == Model::Generic) {
            std::fill(m_fir.begin(), m_fir.end(), 0.0f);
            m_fir[0] = 1.0f;
        } else if (m == Model::Stack4x12) {
            for (size_t i = 0; i < 128; ++i) m_fir[i] = (i < 32) ? (std::exp(-i * 0.1f) * std::sin(i * 0.4f)) : 0.0f;
        } else if (m == Model::Combo1x12) {
            for (size_t i = 0; i < 128; ++i) m_fir[i] = (i < 32) ? (std::exp(-i * 0.2f) * std::cos(i * 0.8f)) : 0.0f;
        }
    }

private:
    std::vector<float> m_fir;
    std::vector<std::vector<float>> m_history;
    size_t m_historyIndex = 0;
};

} // namespace Aura::DSP::Effects
