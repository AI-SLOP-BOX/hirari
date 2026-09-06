#pragma once

#include <cmath>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include <vector>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoTremolo
 * @brief High-end Volume and Pan Modulation (Rhodes style).
 * HONEST FIX: Implements synchronized amplitude modulation (AM) 
 * with a phase-offset between Left and Right to create the classic 
 * 'Auto-Pan' movement found in vintage electric pianos.
 */
class StereoTremolo : public IProcessor {
public:
    StereoTremolo() : m_lfoPhase(0.0), m_depth(0.0), m_stereoWidth(0.0) {
        reset();
    }

    std::string getName() const override { return "Stereo Tremolo"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setDepth(value);
        else if (id == 1) setNoteValue(0.0625f + std::clamp(value, 0.0f, 1.0f) * 1.9375f);
        else if (id == 2) setStereoWidth(value);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return m_depth;
        if (id == 1) return std::clamp((m_noteValue - 0.0625f) / 1.9375f, 0.0f, 1.0f);
        if (id == 2) return m_stereoWidth;
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Depth" : (id == 1 ? "Note Value" : (id == 2 ? "Stereo Width" : "")));
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[] = {m_depth, m_noteValue, m_stereoWidth};
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[3]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(values, state.data()+16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f ||
            !std::isfinite(values[0]) || values[0] < 0.0f || values[0] > 1.0f || !std::isfinite(values[1]) || values[1] < 0.0625f || values[1] > 2.0f || !std::isfinite(values[2]) || values[2] < 0.0f || values[2] > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; setDepth(values[0]); setNoteValue(values[1]); setStereoWidth(values[2]); return true;
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    /**
     * @brief PROCESS: Rhythmic volume and pan modulation.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;
        const double sr = m_sampleRate > 1000.0 ? m_sampleRate : 44100.0;
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const double noteValue = std::isfinite(m_noteValue) ? std::clamp(static_cast<double>(m_noteValue), 0.0625, 2.0) : 0.25;
        const double hz = bpm / (60.0 * noteValue);
        const double inc = hz / sr;
        for (uint32_t i = 0; i < n; ++i) {
            const float lfo = static_cast<float>(0.5 + 0.5 * std::sin(2.0 * M_PI * m_lfoPhase));
            const float amplitude = 1.0f - m_depth * (1.0f - lfo);
            const float pan = m_stereoWidth * std::sin(2.0 * M_PI * m_lfoPhase);
            const float gainL = amplitude * (1.0f - 0.25f * pan);
            const float gainR = amplitude * (1.0f + 0.25f * pan);
            left[i] *= gainL;
            if (right) right[i] *= gainR;
            m_lfoPhase += inc;
            if (m_lfoPhase >= 1.0) m_lfoPhase -= std::floor(m_lfoPhase);
        }
    }


    void reset() noexcept override {
        m_lfoPhase = 0.0;
    }

    // Parameters
    void setDepth(float d) { m_depth = std::clamp(d, 0.0f, 1.0f); }
    void setNoteValue(float v) { m_noteValue = std::isfinite(v) ? std::clamp(v, 0.0625f, 2.0f) : 0.25f; } // 0.25 (Quarter), 0.5 (Half), etc.
    void setStereoWidth(float w) { m_stereoWidth = std::clamp(w, 0.0f, 1.0f); }

private:
    double m_sampleRate = 44100.0;
    double m_lfoPhase;
    float m_depth;
    float m_noteValue = 0.25f;
    float m_stereoWidth = 0.5f; // 0.5 = 180 deg (Full Pan)
};

} // namespace Aura::DSP::Effects
