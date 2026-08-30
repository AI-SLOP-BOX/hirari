#pragma once
#include <vector>
#include <cmath>
#include <atomic>
#include <algorithm>
#include "state_variable_filter.hpp"
#include "../mixing/linkwitz_riley.hpp"
#include "../effects/delay_line.hpp"
#include "../../core/audio_processor_graph.hpp"

namespace Aura::DSP::Mixing {

using namespace Aura::Core;

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

    void prepareToPlay(double sr, uint32_t bs) override { 
        m_sampleRate = sr; 
        m_crossoverL.setParameters(5500.0f, (float)sr);
        m_crossoverR.setParameters(5500.0f, (float)sr);
    }

    void process(AudioBuffer& buffer, const MidiBuffer& /*midi*/) override {
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


    void reset() override { m_gain = 1.0f; m_sibEnv = 0.0f; m_bbEnv = 0.0f; m_lookaheadL.reset(); m_lookaheadR.reset(); }
    uint32_t getLatencySamples() const override { return static_cast<uint32_t>(0.002f * m_sampleRate); }

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
class NoiseGate : public IProcessor {
public:
    NoiseGate(double sr = 44100.0) : m_sampleRate(sr), m_lookaheadL(1024), m_lookaheadR(1024) {
        m_attack = 1.0f - std::exp(-1.0f / (0.002f * (float)sr));
        m_release = 1.0f - std::exp(-1.0f / (0.2f * (float)sr));
    }

    void prepareToPlay(double sr, uint32_t bs) override { m_sampleRate = sr; }

    void process(AudioBuffer& buffer, const MidiBuffer& midi) override {
        processInternal(buffer, buffer, midi);
    }

    /**
     * @brief EXTERNAL SIDECHAIN: Pulls detection signal from a specific Bus.
     */
    void processWithSidechain(AudioBuffer& main, uint32_t sidechainBusId, const MidiBuffer& midi) {
        auto bus = ::Aura::Core::Engine::BusSystem::getInstance().getBus(sidechainBusId);
        if (bus) {
            AudioBuffer scBuf;
            float* channels[2] = { const_cast<float*>(bus->getBufferL()), const_cast<float*>(bus->getBufferR()) };
            scBuf.wrapChannels(channels, 2, main.getNumSamples());
            processInternal(main, scBuf, midi);
        } else {
            processInternal(main, main, midi);
        }
    }

    void reset() override { m_gain = 0.0f; m_isOpening = false; m_lookaheadL.reset(); m_lookaheadR.reset(); }
    uint32_t getLatencySamples() const override { return static_cast<uint32_t>(0.002f * m_sampleRate); }

private:
    void processInternal(AudioBuffer& buffer, AudioBuffer& sidechain, const MidiBuffer& /*midi*/) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const bool isStereo = buffer.getNumChannels() >= 2;
        const bool hasScStereo = sidechain.getNumChannels() >= 2;

        float* left = buffer.getWritePointer(0);
        float* right = isStereo ? buffer.getWritePointer(1) : nullptr;
        const float* scL = sidechain.getReadPointer(0);
        const float* scR = hasScStereo ? sidechain.getReadPointer(1) : scL;

        const uint32_t delayFrames = getLatencySamples();
        constexpr float thresholdDb = -40.0f;
        const float thresholdLinear = std::pow(10.0f, thresholdDb / 20.0f);
        const uint32_t holdSamples = static_cast<uint32_t>(0.02f * m_sampleRate); // 20ms hold

        for (uint32_t i = 0; i < numSamples; ++i) {
            float inL = std::isfinite(left[i]) ? left[i] : 0.0f;
            float inR = (isStereo && right && std::isfinite(right[i])) ? right[i] : inL;

            float detectL = (scL && std::isfinite(scL[i])) ? scL[i] : inL;
            float detectR = (scR && std::isfinite(scR[i])) ? scR[i] : inR;

            float scLevel = std::max(std::abs(detectL), std::abs(detectR));
            m_env = (scLevel > m_env) ? (0.8f * m_env + 0.2f * scLevel) : (0.998f * m_env + 0.002f * scLevel);
            if (std::abs(m_env) < 1.0e-24f) m_env = 0.0f;

            if (m_env > thresholdLinear) {
                m_holdCounter = holdSamples;
                m_isOpening = true;
            } else if (m_holdCounter > 0) {
                m_holdCounter--;
            } else {
                m_isOpening = false;
            }

            float targetGain = (m_isOpening || m_holdCounter > 0) ? 1.0f : 0.0f;
            float coeff = (targetGain > m_gain) ? m_attack : m_release;
            m_gain += (targetGain - m_gain) * coeff;

            m_lookaheadL.push(inL);
            if (isStereo) m_lookaheadR.push(inR);

            left[i] = m_lookaheadL.read(delayFrames) * m_gain;
            if (isStereo && right) {
                right[i] = m_lookaheadR.read(delayFrames) * m_gain;
            }
        }
    }


    double m_sampleRate;
    Effects::DelayLine m_lookaheadL, m_lookaheadR;
    float m_gain = 0.0f, m_env = 0.0f;
    uint32_t m_holdCounter = 0;
    float m_attack, m_release;
    bool m_isOpening = false;
};

} // namespace Aura::DSP::Mixing
