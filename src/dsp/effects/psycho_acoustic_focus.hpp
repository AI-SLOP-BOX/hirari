#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../../core/audio_buffer.hpp"
#include "../../core/atomic_parameter.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class PsychoAcousticFocus
 * @brief Professional 'Brain-Friendly' Harmonic Enhancer.
 */
class PsychoAcousticFocus : public IProcessor {
public:
    PsychoAcousticFocus(double sr = 44100.0) : m_sampleRate(
        std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        updateCoefficients();
    }

    std::string getName() const override { return "Psycho Acoustic Focus"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        updateCoefficients();
    }

    /**
     * @brief PROCESS: Generates musically-related even-order harmonics.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 32);
        const float amount = std::clamp(m_focusAmount.getNextValue(), 0.0f, 1.0f);
        for (uint32_t c = 0; c < channels; ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            float low = m_lastIn[c];
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float x = std::isfinite(data[i]) ? data[i] : 0.0f;
                low = m_alpha * low + (1.0f - m_alpha) * x;
                const float high = x - low;
                const float harmonic = high * high * std::copysign(1.0f, high);
                const float y = x + amount * 0.18f * harmonic;
                data[i] = std::isfinite(y) ? std::clamp(y, -4.0f, 4.0f) : 0.0f;
            }
            m_lastIn[c] = low;
        }
    }


    void reset() noexcept override {
        std::fill(m_hpfState.begin(), m_hpfState.end(), 0.0f);
        std::fill(m_lastIn.begin(), m_lastIn.end(), 0.0f);
    }

    void setFocusAmount(float val) { m_focusAmount.setTarget(val); }

private:
    void updateCoefficients() {
        float cutoff = 3500.0f;
        float dt = 1.0f / static_cast<float>(m_sampleRate);
        float rc = 1.0f / (2.0f * M_PI * cutoff);
        m_alpha = rc / (rc + dt);
        m_hpfState.assign(32, 0.0f);
        m_lastIn.assign(32, 0.0f);
    }

    Core::AtomicParameter m_focusAmount{0.5f};
    double m_sampleRate;
    float m_alpha = 0.9f;
    std::vector<float> m_hpfState;
    std::vector<float> m_lastIn;
};

} // namespace Aura::DSP::Effects
