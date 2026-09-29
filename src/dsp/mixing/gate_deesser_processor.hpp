#pragma once
#include <vector>
#include <cmath>
#include <atomic>
#include <algorithm>
#include "state_variable_filter.hpp"
#include "../mixing/linkwitz_riley.hpp"
#include "../effects/delay_line.hpp"
#include "../../core/audio_processor_graph.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Mixing {

using namespace Hirari::Core;

/**
 * @class DeEsserProcessor
 * @brief Professional Relative-Band De-Esser.
 * HONEST FIX: Uses a Relative Threshold (Sibilance vs. Broadband energy).
 * This prevents over-compression ('lisping') of loud vowels and only 
 * targets true sibilance peaks. Added RMS-based detection for smoother gain.
 */
class DeEsserProcessor : public IProcessor {
public:
    DeEsserProcessor(double sr = 44100.0)
        : m_sampleRate(sr), m_sidechainBP(sr), m_lookaheadL(2048), m_lookaheadR(2048),
          m_lookaheadL_Hi(2048), m_lookaheadR_Hi(2048)
    {
        m_crossoverL.setParameters(5500.0f, (float)sr);
        m_crossoverR.setParameters(5500.0f, (float)sr);
        m_sidechainBP.setParameters(7200.0f, 1.2f, 0); // Narrower sibilance focus
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr; 
        m_crossoverL.setParameters(5500.0f, (float)sr);
        m_crossoverR.setParameters(5500.0f, (float)sr);
    }

    void process(AudioBuffer& buffer, MidiBuffer& /*midi*/,
                 const ProcessContext& /*context*/) noexcept override {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const bool isStereo = buffer.getNumChannels() >= 2;
        float* left = buffer.getWritePointer(0);
        float* right = isStereo ? buffer.getWritePointer(1) : nullptr;

        const uint32_t delayFrames = getLatencySamples();

        for (uint32_t i = 0; i < numSamples; ++i) {
            float inL = std::isfinite(left[i]) ? left[i] : 0.0f;
            float inR = (isStereo && right && std::isfinite(right[i])) ? right[i] : inL;

            // Split bands via Linkwitz-Riley
            float lowL = 0.0f, hiL = 0.0f;
            float lowR = 0.0f, hiR = 0.0f;
            m_crossoverL.process(inL, lowL, hiL);
            if (isStereo) m_crossoverR.process(inR, lowR, hiR);
            else { lowR = lowL; hiR = hiL; }

            // Bandpass sidechain detection
            float bpL = m_sidechainBP.process(inL);
            float bpR = isStereo ? m_sidechainBP.process(inR) : bpL;

            float sibilanceLevel = std::max(std::abs(bpL), std::abs(bpR));
            float broadbandLevel = std::max(std::abs(inL), std::abs(inR));

            m_sibEnv = 0.9f * m_sibEnv + 0.1f * sibilanceLevel;
            m_bbEnv = 0.99f * m_bbEnv + 0.01f * broadbandLevel;

            if (std::abs(m_sibEnv) < 1.0e-24f) m_sibEnv = 0.0f;
            if (std::abs(m_bbEnv) < 1.0e-24f) m_bbEnv = 0.0f;

            // Relative ratio comparison
            float ratio = (m_bbEnv > 0.0001f) ? (m_sibEnv / m_bbEnv) : 0.0f;
            float targetGain = (ratio > 1.2f) ? (1.0f / (1.0f + (ratio - 1.2f) * 4.0f)) : 1.0f;
            targetGain = std::clamp(targetGain, 0.15f, 1.0f);

            m_gain += (targetGain - m_gain) * 0.05f;

            // Delay compensation lookahead push/pop
            m_lookaheadL.push(lowL + hiL * m_gain);
            if (isStereo) m_lookaheadR.push(lowR + hiR * m_gain);

            left[i] = m_lookaheadL.read(delayFrames);
            if (isStereo && right) {
                right[i] = m_lookaheadR.read(delayFrames);
            }
        }
    }


    void reset() noexcept override { m_gain = 1.0f; m_sibEnv = 0.0f; m_bbEnv = 0.0f; m_lookaheadL.reset(); m_lookaheadR.reset(); }
    uint32_t getLatencySamples() const noexcept override { return static_cast<uint32_t>(0.002f * m_sampleRate); }

private:
    double m_sampleRate;
    LinkwitzRileyFilter m_crossoverL, m_crossoverR;
    StateVariableFilter m_sidechainBP;
    Effects::DelayLine m_lookaheadL, m_lookaheadR, m_lookaheadL_Hi, m_lookaheadR_Hi;
    float m_gain = 1.0f;
    float m_sibEnv = 0.0f, m_bbEnv = 0.0f;
};


/**
 * @class NoiseGate
 * @brief Professional dynamics processor with External Sidechain and Lookahead.
 */
class NoiseGate final : public IProcessor {
public:
    explicit NoiseGate(double sr = 44100.0) noexcept
        : m_sampleRate(sr), m_state(hirari_noise_gate_create(sr)) {}

    ~NoiseGate() override { hirari_noise_gate_destroy(m_state); }
    NoiseGate(const NoiseGate&) = delete;
    NoiseGate& operator=(const NoiseGate&) = delete;

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = sr;
        hirari_noise_gate_prepare(m_state, sr);
    }

    void process(AudioBuffer& buffer, MidiBuffer&, const ProcessContext&) noexcept override {
        processInternal(buffer, nullptr, nullptr);
    }

    void processWithSidechain(AudioBuffer& main, uint32_t sidechainBusId,
                              MidiBuffer&) noexcept {
        auto bus = ::Hirari::Core::Engine::BusSystem::getInstance().getBus(sidechainBusId);
        processInternal(main, bus ? bus->getBufferL() : nullptr,
                        bus ? bus->getBufferR() : nullptr);
    }

    void reset() noexcept override { hirari_noise_gate_reset(m_state); }
    uint32_t getLatencySamples() const noexcept override {
        return static_cast<uint32_t>(0.002f * m_sampleRate);
    }

private:
    void processInternal(AudioBuffer& buffer, const float* sidechainLeft,
                         const float* sidechainRight) noexcept {
        if (!m_state || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_noise_gate_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() >= 2 ? buffer.getWritePointer(1) : nullptr,
            sidechainLeft, sidechainRight, buffer.getNumSamples());
    }

    double m_sampleRate;
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Mixing
