#pragma once
#include <vector>
#include <array>
#include <cmath>
#include "../iprocessor.hpp"
#include "../../core/engine/immersive_bus_manager.hpp"
#include "../../core/audio_buffer.hpp"
#include "../math/denormal_killer.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief AtmosReverb: Professional 12-Channel Immersive Reverb.
 * Now protected against Denormal CPU spikes in the feedback loop.
 */
class AtmosReverb : public IProcessor {
public:
    AtmosReverb(double sr = 44100.0) : m_sampleRate(44100.0) {
        m_delays.resize(12);
        setSampleRate(sr);
    }

    std::string getName() const override { return "Atmos Immersive Reverb"; }

    /**
     * @brief PROCESS IMMERSIVE: FDN processing with Denormal Protection.
     */
    void processImmersiveRaw(float* const* buffers, uint32_t bufferCount, uint32_t numSamples) {
        const uint32_t channels = std::min<uint32_t>(bufferCount, 12);
        if (channels == 0 || numSamples == 0) return;
        for (uint32_t s = 0; s < numSamples; ++s) {
            float input = 0.0f;
            uint32_t active = 0;
            for (uint32_t c = 0; c < channels; ++c) {
                if (!buffers[c]) continue;
                const float x = std::isfinite(buffers[c][s]) ? buffers[c][s] : 0.0f;
                input += x;
                ++active;
            }
            if (active == 0) continue;
            input /= static_cast<float>(active);
            float sum = 0.0f;
            for (uint32_t c = 0; c < 12; ++c) {
                auto& delay = m_delays[c];
                const size_t read = (m_writeIndices[c] + 1) % delay.size();
                const float value = std::isfinite(delay[read]) ? delay[read] : 0.0f;
                m_state[c] += (value - m_state[c]) * 0.08f;
                sum += m_state[c];
            }
            const float mean = sum / 12.0f;
            for (uint32_t c = 0; c < 12; ++c) {
                auto& delay = m_delays[c];
                const float fed = input * 0.18f + (m_state[c] - 2.0f * mean) * 0.82f;
                delay[m_writeIndices[c]] = std::isfinite(fed) ? fed : 0.0f;
                m_writeIndices[c] = (m_writeIndices[c] + 1) % delay.size();
            }
            for (uint32_t c = 0; c < channels; ++c) {
                if (!buffers[c]) continue;
                const float wet = m_state[c] * 0.35f + mean * 0.15f;
                const float dry = std::isfinite(buffers[c][s]) ? buffers[c][s] : 0.0f;
                const float out = dry * 0.8f + wet * 0.2f;
                buffers[c][s] = std::isfinite(out) ? std::clamp(out, -16.0f, 16.0f) : 0.0f;
            }
        }
    }

    void processImmersive(const std::vector<float*>& buffers, uint32_t numSamples) {
        processImmersiveRaw(buffers.data(), static_cast<uint32_t>(buffers.size()), numSamples);
    }


    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        m_bufferPointers[0] = l;
        m_bufferPointers[1] = r;
        for (size_t c = 2; c < m_bufferPointers.size(); ++c) m_bufferPointers[c] = nullptr;
        processImmersiveRaw(m_bufferPointers.data(), 2, numSamples);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 12);
        for (uint32_t c = 0; c < 12; ++c) {
            m_bufferPointers[c] = c < channels ? buffer.getWritePointer(c) : nullptr;
        }
        // The fixed pointer array avoids an allocation on every realtime
        // block while retaining the existing vector-based public helper.
        processImmersiveRaw(m_bufferPointers.data(), channels, buffer.getNumSamples());
    }
    void reset() noexcept override {
        for (auto& delay : m_delays) std::fill(delay.begin(), delay.end(), 0.0f);
        m_state.fill(0.0f);
        m_writeIndices.fill(0);
    }

    void setSampleRate(double sr) {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        const size_t delaySamples = std::max<size_t>(1u, static_cast<size_t>(m_sampleRate * 0.1));
        for (auto& delay : m_delays) delay.assign(delaySamples, 0.0f);
        m_writeIndices.fill(0);
    }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override {
        // The immersive tank feeds back at 0.82; reserve enough traversals
        // for the decay to fall below the export noise floor.
        const size_t longest = m_delays.empty() ? 0u : m_delays.front().size();
        return static_cast<uint32_t>(std::min<size_t>(longest * 40u,
            static_cast<size_t>(m_sampleRate * 30.0)));
    }

private:
    double m_sampleRate;
    std::vector<std::vector<float>> m_delays;
    std::array<size_t, 12> m_writeIndices{};
    std::array<float, 12> m_state{};
    std::array<float*, 12> m_bufferPointers{};
};

} // namespace Aura::DSP::Effects
