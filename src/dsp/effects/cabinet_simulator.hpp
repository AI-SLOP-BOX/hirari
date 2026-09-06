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

    std::string getName() const override { return "Cabinet Simulator"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)sr;
        (void)bs;
        reset();
    }

    uint32_t getTailSamples() const noexcept override {
        return m_fir.empty() ? 0u : static_cast<uint32_t>(m_fir.size() - 1u);
    }

    /**
     * @brief PROCESS: Applies the FIR convolution (Speaker color).
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
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
        std::fill(m_fir.begin(), m_fir.end(), 0.0f);
        if (m == Model::Generic) {
            m_fir[0] = 1.0f;
            return;
        }
        const bool stack = m == Model::Stack4x12;
        const float decay = stack ? 0.060f : 0.095f;
        const float resonance = stack ? 0.31f : 0.52f;
        const float secondary = stack ? 0.16f : 0.10f;
        for (size_t i = 0; i < m_fir.size(); ++i) {
            const float t = static_cast<float>(i);
            const float envelope = std::exp(-decay * t);
            const float body = std::sin(resonance * t + (stack ? 0.15f : 0.42f));
            const float reflection = (i >= (stack ? 17u : 11u))
                ? secondary * std::exp(-0.11f * static_cast<float>(i - (stack ? 17u : 11u)))
                    * std::cos(0.19f * t) : 0.0f;
            m_fir[i] = envelope * (0.82f * body + reflection);
        }
        // Give the IR a defined transient and normalize energy so model
        // changes do not unexpectedly change channel loudness.
        m_fir[0] += stack ? 0.42f : 0.58f;
        float energy = 0.0f;
        for (const float tap : m_fir) energy += tap * tap;
        if (energy > 1.0e-9f && std::isfinite(energy)) {
            const float scale = 0.92f / std::sqrt(energy);
            for (float& tap : m_fir) tap *= scale;
        }
    }

private:
    std::vector<float> m_fir;
    std::vector<std::vector<float>> m_history;
    size_t m_historyIndex = 0;
};

} // namespace Aura::DSP::Effects
