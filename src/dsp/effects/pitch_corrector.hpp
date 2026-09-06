#pragma once

#include <vector>
#include <array>
#include <cmath>
#include <algorithm>
#include "state_variable_filter.hpp"
#include "../utils/pitch_shifter.hpp"
#include "../../core/engine/scale_system.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief PitchCorrector: Professional 'Autotune-style' vocal processing.
 * Corrects incoming frequency to the nearest scale degree.
 */
class PitchCorrector {
public:
    PitchCorrector(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr > 0.0 ? sr : 44100.0) {}

    /**
     * @brief ACCURATE CORRECTION: Detects pitch and applies shifting.
     * AI-SUPPORT: Can suggest correction speed, but defaults to manual control.
     */
    void process(float* l, float* r, uint32_t numSamples, float speed = 0.5f) {
        if (l == nullptr || numSamples < 32 || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) return;
        const float response = std::clamp(std::isfinite(speed) ? speed : 0.5f, 0.0f, 1.0f);

        // RT-safe thread local buffer for filtering
        constexpr uint32_t kMaxSamples = 4096;
        const uint32_t processCount = std::min(numSamples, kMaxSamples);
        
        static thread_local std::array<float, kMaxSamples> filteredBuf;
        std::copy(l, l + processCount, filteredBuf.begin());

        // Lightweight one-pole low-pass at 1 kHz for robust fundamental detection.
        const float alpha = std::exp(-2.0f * static_cast<float>(M_PI) * 1000.0f /
                                     static_cast<float>(m_sampleRate));
        float state = m_lpState;
        for (uint32_t i = 0; i < processCount; ++i) {
            state = alpha * state + (1.0f - alpha) * filteredBuf[i];
            filteredBuf[i] = state;
        }
        m_lpState = state;

        const float detected = estimateFrequency(filteredBuf.data(), processCount);
        if (!std::isfinite(detected) || detected < 40.0f || detected > 2000.0f) {
            // Decay correction ratio smoothly to 1.0f on detection loss
            m_correctionRatio += (1.0f - m_correctionRatio) * response;
            m_shifterL.process(l, numSamples, m_correctionRatio, static_cast<float>(m_sampleRate));
            if (r) m_shifterR.process(r, numSamples, m_correctionRatio, static_cast<float>(m_sampleRate));
            return;
        }

        const float semitone = 69.0f + 12.0f * std::log2(detected / 440.0f);
        const int chromaticTarget = static_cast<int>(std::round(semitone));
        
        // Snap chromatic note to the active DAW musical scale
        const int quantizedTarget = Core::Engine::ScaleSystem::getInstance().quantizeNote(chromaticTarget);

        const float targetFrequency = 440.0f * std::pow(2.0f, (static_cast<float>(quantizedTarget) - 69.0f) / 12.0f);
        const float desiredRatio = std::clamp(targetFrequency / detected, 0.5f, 2.0f);
        m_correctionRatio += (desiredRatio - m_correctionRatio) * response;
        if (!std::isfinite(m_correctionRatio)) m_correctionRatio = 1.0f;
        
        m_shifterL.process(l, numSamples, m_correctionRatio, static_cast<float>(m_sampleRate));
        if (r) m_shifterR.process(r, numSamples, m_correctionRatio, static_cast<float>(m_sampleRate));
    }


private:
    float estimateFrequency(const float* samples, uint32_t count) {
        uint32_t crossings = 0;
        float previous = 0.0f;
        bool havePrevious = false;
        for (uint32_t i = 0; i < count; ++i) {
            const float current = std::isfinite(samples[i]) ? samples[i] : 0.0f;
            if (havePrevious && previous < 0.0f && current >= 0.0f) ++crossings;
            previous = current;
            havePrevious = true;
        }
        if (crossings == 0) return 0.0f;
        return static_cast<float>(m_sampleRate) * static_cast<float>(crossings) /
               static_cast<float>(count);
    }

    double m_sampleRate;
    Utils::PitchShifter m_shifterL;
    Utils::PitchShifter m_shifterR;
    float m_correctionRatio = 1.0f;
    float m_lpState = 0.0f;
};

} // namespace Aura::DSP::Effects
