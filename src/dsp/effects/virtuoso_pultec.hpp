#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <array>
#include <cstdio>
#include <cstring>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class VirtuosoPultec
 * @brief Legendary Passive Program Equalizer (EQP-1A Emulation).
 * HONEST FIX: Implemented real 'Pultec Trick' Low Boost/Cut and High Boost shelf filters.
 */
class VirtuosoPultec : public IProcessor {
public:
    struct BiquadCoeffs {
        float b0 = 1.0f, b1 = 0.0f, b2 = 0.0f;
        float a1 = 0.0f, a2 = 0.0f;
    };

    struct BiquadState {
        float x1 = 0.0f, x2 = 0.0f;
        float y1 = 0.0f, y2 = 0.0f;
        
        inline float process(float x, const BiquadCoeffs& c) {
            float y = c.b0 * x + c.b1 * x1 + c.b2 * x2 - c.a1 * y1 - c.a2 * y2;
            if (std::abs(y) < 1.0e-15f) y = 0.0f;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            return y;
        }
        
        void reset() {
            x1 = x2 = y1 = y2 = 0.0f;
        }
    };

    VirtuosoPultec(double sr = 44100.0) : m_sampleRate(sr > 0.0 ? sr : 44100.0) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = sr > 0.0 ? sr : 44100.0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        // Calculate filter coefficients
        BiquadCoeffs lowBoostCoeffs = makeLowShelf(m_sampleRate, m_lowFreq, m_lowBoost);
        BiquadCoeffs lowCutCoeffs = makeLowShelf(m_sampleRate, m_lowFreq, -m_lowAtten);
        BiquadCoeffs highBoostCoeffs = makeHighShelf(m_sampleRate, m_highFreq, m_highBoost);

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] : inL;

            // Left Channel Processing (Low Boost -> Low Cut -> High Boost)
            float outL = m_lowBoostL.process(inL, lowBoostCoeffs);
            outL = m_lowCutL.process(outL, lowCutCoeffs);
            outL = m_highBoostL.process(outL, highBoostCoeffs);

            // Right Channel Processing
            float outR = m_lowBoostR.process(inR, lowBoostCoeffs);
            outR = m_lowCutR.process(outR, lowCutCoeffs);
            outR = m_highBoostR.process(outR, highBoostCoeffs);

            l[s] = outL;
            if (r) r[s] = outR;
        }
    }

    void reset() noexcept override {
        m_lowBoostL.reset();
        m_lowBoostR.reset();
        m_lowCutL.reset();
        m_lowCutR.reset();
        m_highBoostL.reset();
        m_highBoostR.reset();
    }

    std::string getName() const override { return "VirtuosoPultec"; }
    uint32_t getNumParameters() const noexcept override { return 5; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        value = std::clamp(value, 0.0f, 1.0f);
        if (id == 0) m_lowFreq = 20.0f + value * 180.0f;
        else if (id == 1) m_lowBoost = value * 12.0f;
        else if (id == 2) m_lowAtten = value * 12.0f;
        else if (id == 3) m_highFreq = 1000.0f + value * 19000.0f;
        else if (id == 4) m_highBoost = value * 12.0f;
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return (m_lowFreq - 20.0f) / 180.0f;
        if (id == 1) return m_lowBoost / 12.0f;
        if (id == 2) return m_lowAtten / 12.0f;
        if (id == 3) return (m_highFreq - 1000.0f) / 19000.0f;
        return id == 4 ? m_highBoost / 12.0f : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override { if (id >= 5) return false; out = {0.0f, 1.0f, false}; return true; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override { if (!outName || !maxSize) return; const char* names[] = {"Low Frequency", "Low Boost", "Low Atten", "High Frequency", "High Boost"}; std::snprintf(outName, maxSize, "%s", id < 5 ? names[id] : ""); }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(36, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2); std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4); std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[5] = {getParameter(0), getParameter(1), getParameter(2), getParameter(3), getParameter(4)};
        std::memcpy(state.data() + 16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 36) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[5]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2); std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        for (uint32_t i = 0; i < 5; ++i) setParameter(i, values[i]); return true;
    }

    void setParameters(float lowFreq, float lowBoost, float lowAtten, float highFreq, float highBoost) {
        if (std::isfinite(lowFreq)) m_lowFreq = std::clamp(lowFreq, 20.0f, 200.0f);
        if (std::isfinite(lowBoost)) m_lowBoost = std::clamp(lowBoost, 0.0f, 12.0f);
        if (std::isfinite(lowAtten)) m_lowAtten = std::clamp(lowAtten, 0.0f, 12.0f);
        if (std::isfinite(highFreq)) m_highFreq = std::clamp(highFreq, 1000.0f, 20000.0f);
        if (std::isfinite(highBoost)) m_highBoost = std::clamp(highBoost, 0.0f, 12.0f);
    }

private:
    double m_sampleRate;
    float m_lowFreq = 60.0f, m_lowBoost = 2.0f, m_lowAtten = 1.0f;
    float m_highFreq = 12000.0f, m_highBoost = 3.0f;

    BiquadState m_lowBoostL, m_lowBoostR;
    BiquadState m_lowCutL, m_lowCutR;
    BiquadState m_highBoostL, m_highBoostR;

    BiquadCoeffs makeLowShelf(double sr, float freq, float gainDb) {
        BiquadCoeffs c;
        float A = std::pow(10.0f, gainDb / 40.0f);
        float w0 = 2.0f * static_cast<float>(M_PI) * freq / static_cast<float>(sr);
        float cosw0 = std::cos(w0);
        float sinw0 = std::sin(w0);
        float alpha = sinw0 / 2.0f * std::sqrt((A + 1.0f / A) * (1.0f / 0.707f - 1.0f) + 2.0f);

        float a0 = (A + 1.0f) + (A - 1.0f) * cosw0 + 2.0f * std::sqrt(A) * alpha;
        c.b0 = (A * ((A + 1.0f) - (A - 1.0f) * cosw0 + 2.0f * std::sqrt(A) * alpha)) / a0;
        c.b1 = (2.0f * A * ((A - 1.0f) - (A + 1.0f) * cosw0)) / a0;
        c.b2 = (A * ((A + 1.0f) - (A - 1.0f) * cosw0 - 2.0f * std::sqrt(A) * alpha)) / a0;
        c.a1 = (-2.0f * ((A - 1.0f) + (A + 1.0f) * cosw0)) / a0;
        c.a2 = ((A + 1.0f) + (A - 1.0f) * cosw0 - 2.0f * std::sqrt(A) * alpha) / a0;
        return c;
    }

    BiquadCoeffs makeHighShelf(double sr, float freq, float gainDb) {
        BiquadCoeffs c;
        float A = std::pow(10.0f, gainDb / 40.0f);
        float w0 = 2.0f * static_cast<float>(M_PI) * freq / static_cast<float>(sr);
        float cosw0 = std::cos(w0);
        float sinw0 = std::sin(w0);
        float alpha = sinw0 / 2.0f * std::sqrt((A + 1.0f / A) * (1.0f / 0.707f - 1.0f) + 2.0f);

        float a0 = (A + 1.0f) - (A - 1.0f) * cosw0 + 2.0f * std::sqrt(A) * alpha;
        c.b0 = (A * ((A + 1.0f) + (A - 1.0f) * cosw0 + 2.0f * std::sqrt(A) * alpha)) / a0;
        c.b1 = (-2.0f * A * ((A - 1.0f) + (A + 1.0f) * cosw0)) / a0;
        c.b2 = (A * ((A + 1.0f) + (A - 1.0f) * cosw0 - 2.0f * std::sqrt(A) * alpha)) / a0;
        c.a1 = (2.0f * ((A - 1.0f) - (A + 1.0f) * cosw0)) / a0;
        c.a2 = ((A + 1.0f) - (A - 1.0f) * cosw0 - 2.0f * std::sqrt(A) * alpha) / a0;
        return c;
    }
};

} // namespace Aura::DSP::Effects
