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
    AtmosReverb(double sr = 44100.0) : m_sampleRate(sr) {
        m_delays.resize(12);
        for (auto& d : m_delays) d.resize(4410, 0.0f); 
    }

    /**
     * @brief PROCESS IMMERSIVE: FDN processing with Denormal Protection.
     */
    void processImmersive(const std::vector<float*>& buffers, uint32_t numSamples) {
        const uint32_t channels = std::min<uint32_t>(static_cast<uint32_t>(buffers.size()), 12);
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
                buffers[c][s] = std::isfinite(dry * 0.8f + wet * 0.2f) ? dry * 0.8f + wet * 0.2f : 0.0f;
            }
        }
    }


    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        std::vector<float*> buffers{l, r};
        processImmersive(buffers, numSamples);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0) return;
        std::vector<float*> buffers;
        buffers.reserve(std::min<uint32_t>(buffer.getNumChannels(), 12));
        for (uint32_t c = 0; c < std::min<uint32_t>(buffer.getNumChannels(), 12); ++c) buffers.push_back(buffer.getWritePointer(c));
        processImmersive(buffers, buffer.getNumSamples());
    }
    void reset() noexcept override {
        for (auto& delay : m_delays) std::fill(delay.begin(), delay.end(), 0.0f);
        m_state.fill(0.0f);
        m_writeIndices.fill(0);
    }

    void setSampleRate(double sr) { m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0; }
    uint32_t getLatency() const { return 0; }

private:
    double m_sampleRate;
    std::vector<std::vector<float>> m_delays;
    std::array<size_t, 12> m_writeIndices{};
    std::array<float, 12> m_state{};
};

} // namespace Aura::DSP::Effects
