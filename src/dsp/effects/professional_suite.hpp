#pragma once

#include <vector>
#include <array>
#include <cmath>
#include <algorithm>
#include <limits>
#include "../iprocessor.hpp"
#include "../../core/audio_buffer.hpp"
#include "../mixing/state_variable_filter.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ProfessionalChorus
 * @brief Industrial-Grade Multi-Voice Chorus.
 * Features 8 LFO-modulated delay lines for lush cinematic textures.
 */
class ProfessionalChorus : public IProcessor {
public:
    ProfessionalChorus() {
        reset();
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        const size_t delayLength = static_cast<size_t>(m_sampleRate * 0.1); // 100ms max delay
        m_delays.resize(8);
        for (auto& d : m_delays) {
            d.assign(delayLength, 0.0f);
        }
        m_writeIdx = 0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty() || m_delays.empty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        const size_t delayLength = m_delays[0].size();
        if (delayLength == 0) return;

        const float depthSamples = m_depth * static_cast<float>(m_sampleRate);
        const float centerDelay = 0.02f * static_cast<float>(m_sampleRate); // 20ms baseline

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] : inL;

            // Write input to all circular delay lines
            for (size_t v = 0; v < 8; ++v) {
                m_delays[v][m_writeIdx] = (v % 2 == 0) ? inL : inR;
            }

            float wetL = 0.0f;
            float wetR = 0.0f;

            // 8-voice LFO-modulated read taps
            for (size_t v = 0; v < 8; ++v) {
                m_phases[v] += m_rates[v] / static_cast<float>(m_sampleRate);
                if (m_phases[v] >= 1.0f) m_phases[v] -= 1.0f;

                float lfo = std::sin(2.0f * static_cast<float>(M_PI) * m_phases[v]);
                float modDelay = centerDelay + lfo * depthSamples;
                modDelay = std::clamp(modDelay, 1.0f, static_cast<float>(delayLength - 2));

                float readPtr = static_cast<float>(m_writeIdx) + static_cast<float>(delayLength) - modDelay;
                while (readPtr >= static_cast<float>(delayLength)) readPtr -= static_cast<float>(delayLength);

                size_t idx0 = static_cast<size_t>(readPtr);
                size_t idx1 = (idx0 + 1) % delayLength;
                float frac = readPtr - static_cast<float>(idx0);

                // Linear interpolation
                float val = m_delays[v][idx0] * (1.0f - frac) + m_delays[v][idx1] * frac;

                // Stereo Panning
                if (v % 2 == 0) {
                    wetL += val * 0.35f;
                } else {
                    wetR += val * 0.35f;
                }
            }

            m_writeIdx = (m_writeIdx + 1) % delayLength;

            l[s] = inL * 0.6f + wetL * 0.4f;
            if (r) r[s] = inR * 0.6f + wetR * 0.4f;
        }
    }

    std::string getName() const override { return "ProChorus"; }
    uint32_t getTailSamples() const noexcept override {
        return static_cast<uint32_t>(std::min(0.1 * std::max(1.0, m_sampleRate),
                                              static_cast<double>(std::numeric_limits<uint32_t>::max())));
    }

    void reset() noexcept override {
        for (auto& d : m_delays) {
            std::fill(d.begin(), d.end(), 0.0f);
        }
        std::fill(m_phases.begin(), m_phases.end(), 0.0f);
        m_writeIdx = 0;
    }

private:
    double m_sampleRate = 44100.0;
    std::vector<std::vector<float>> m_delays;
    std::array<float, 8> m_phases{0};
    std::array<float, 8> m_rates{0.25f, 0.45f, 0.65f, 0.85f, 1.15f, 1.35f, 1.55f, 1.75f};
    float m_depth = 0.003f; // 3ms depth modulation
    size_t m_writeIdx = 0;
};

/**
 * @class DynamicEQPro
 * @brief Multi-Band Dynamic Equalizer with Industrial Sidechaining.
 */
class DynamicEQPro : public IProcessor {
public:
    struct Band {
        float freq = 1000.0f;
        float q = 1.0f;
        float thresholdDb = -20.0f;
        float ratio = 4.0f;
        float attackMs = 10.0f;
        float releaseMs = 100.0f;
        float currentGainReduction = 0.0f;
    };

    DynamicEQPro() {
        m_bands.push_back(Band{}); // 1 default band
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = sr > 0.0 ? sr : 44100.0;
        m_filterL.setSampleRate(m_sampleRate);
        m_filterR.setSampleRate(m_sampleRate);
        m_detectorL.setSampleRate(m_sampleRate);
        m_detectorR.setSampleRate(m_sampleRate);
        m_filterL.reset();
        m_filterR.reset();
        m_detectorL.reset();
        m_detectorR.reset();
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty() || m_bands.empty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        auto& band = m_bands[0];
        
        const float attackCoef = std::exp(-1.0f / (band.attackMs * 0.001f * static_cast<float>(m_sampleRate)));
        const float releaseCoef = std::exp(-1.0f / (band.releaseMs * 0.001f * static_cast<float>(m_sampleRate)));

        m_detectorL.setParameters(band.freq, band.q, 1);
        m_detectorR.setParameters(band.freq, band.q, 1);

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] : inL;

            float detL = m_detectorL.processSampleBP(inL);
            float detR = r ? m_detectorR.processSampleBP(inR) : detL;

            float rectL = std::abs(detL);
            float rectR = std::abs(detR);
            float peak = std::max(rectL, rectR);

            if (peak > m_envelope) {
                m_envelope = m_envelope * attackCoef + peak * (1.0f - attackCoef);
            } else {
                m_envelope = m_envelope * releaseCoef + peak * (1.0f - releaseCoef);
            }
            if (!std::isfinite(m_envelope)) m_envelope = 0.0f;

            float envDb = 20.0f * std::log10(m_envelope + 1e-6f);
            float gainReductionDb = 0.0f;
            if (envDb > band.thresholdDb) {
                gainReductionDb = (envDb - band.thresholdDb) * (1.0f - 1.0f / band.ratio);
            }
            band.currentGainReduction = gainReductionDb;

            float targetGainDb = -gainReductionDb;
            targetGainDb = std::clamp(targetGainDb, -24.0f, 0.0f);

            float filterOutL = m_filterL.processSampleBP(inL);
            float filterOutR = r ? m_filterR.processSampleBP(inR) : filterOutL;

            float gainFactor = std::pow(10.0f, targetGainDb / 20.0f);
            
            l[s] = inL - filterOutL * (1.0f - gainFactor);
            if (r) r[s] = inR - filterOutR * (1.0f - gainFactor);
        }
    }

    std::string getName() const override { return "DynamicEQPro"; }

    void reset() noexcept override {
        m_filterL.reset();
        m_filterR.reset();
        m_detectorL.reset();
        m_detectorR.reset();
        m_envelope = 0.0f;
    }

private:
    double m_sampleRate = 44100.0;
    std::vector<Band> m_bands;
    Mixing::StateVariableFilter m_filterL;
    Mixing::StateVariableFilter m_filterR;
    Mixing::StateVariableFilter m_detectorL;
    Mixing::StateVariableFilter m_detectorR;
    float m_envelope = 0.0f;
};

/**
 * @class TapeSaturationPro
 * @brief High-Fidelity Analogue Tape Emulation.
 * Models wow/flutter, and non-linear saturation.
 */
class TapeSaturationPro : public IProcessor {
public:
    TapeSaturationPro() {
        m_wowDelayL.resize(1024, 0.0f);
        m_wowDelayR.resize(1024, 0.0f);
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = sr > 0.0 ? sr : 44100.0;
        m_wowDelayL.assign(1024, 0.0f);
        m_wowDelayR.assign(1024, 0.0f);
        m_wowIdx = 0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] : inL;

            // Wow/Flutter LFO update
            m_wowPhase += 3.5f / static_cast<float>(m_sampleRate);
            if (m_wowPhase >= 1.0f) m_wowPhase -= 1.0f;
            
            m_flutterPhase += 18.0f / static_cast<float>(m_sampleRate);
            if (m_flutterPhase >= 1.0f) m_flutterPhase -= 1.0f;

            float wow = std::sin(2.0f * static_cast<float>(M_PI) * m_wowPhase) * 0.7f;
            float flutter = std::sin(2.0f * static_cast<float>(M_PI) * m_flutterPhase) * 0.3f;
            float totalMod = (wow + flutter) * m_wowDepth;

            m_wowDelayL[m_wowIdx] = inL;
            m_wowDelayR[m_wowIdx] = inR;

            float readPos = static_cast<float>(m_wowIdx) + 1024.0f - (10.0f + totalMod);
            while (readPos >= 1024.0f) readPos -= 1024.0f;

            size_t idx0 = static_cast<size_t>(readPos);
            size_t idx1 = (idx0 + 1) % 1024;
            float frac = readPos - static_cast<float>(idx0);

            float wowOutL = m_wowDelayL[idx0] * (1.0f - frac) + m_wowDelayL[idx1] * frac;
            float wowOutR = m_wowDelayR[idx0] * (1.0f - frac) + m_wowDelayR[idx1] * frac;

            m_wowIdx = (m_wowIdx + 1) % 1024;

            // Magnetic Saturation Shaping
            auto saturate = [](float x) {
                float ax = std::abs(x);
                if (ax > 1.0f) {
                    return (x > 0.0f) ? 1.0f : -1.0f;
                }
                return x * (1.5f - 0.5f * x * x);
            };

            float satOutL = saturate(wowOutL * m_drive);
            float satOutR = saturate(wowOutR * m_drive);

            // Tape High Frequency Rolloff Filter
            m_lastOutL = m_lastOutL * 0.85f + satOutL * 0.15f;
            m_lastOutR = m_lastOutR * 0.85f + satOutR * 0.15f;

            l[s] = m_lastOutL;
            if (r) r[s] = m_lastOutR;
        }
    }

    std::string getName() const override { return "TapeSatPro"; }

    void reset() noexcept override {
        std::fill(m_wowDelayL.begin(), m_wowDelayL.end(), 0.0f);
        std::fill(m_wowDelayR.begin(), m_wowDelayR.end(), 0.0f);
        m_wowIdx = 0;
        m_wowPhase = 0.0f;
        m_flutterPhase = 0.0f;
        m_lastOutL = 0.0f;
        m_lastOutR = 0.0f;
    }

private:
    double m_sampleRate = 44100.0;
    std::vector<float> m_wowDelayL;
    std::vector<float> m_wowDelayR;
    size_t m_wowIdx = 0;
    float m_wowPhase = 0.0f;
    float m_flutterPhase = 0.0f;
    float m_wowDepth = 2.0f;
    float m_drive = 1.2f;
    float m_lastOutL = 0.0f;
    float m_lastOutR = 0.0f;
};

} // namespace Aura::DSP::Effects
