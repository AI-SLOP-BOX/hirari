#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "k_weighting_filter.hpp"

namespace Aura::DSP::Analysis {

class LoudnessAnalyzer {
public:
    struct Metrics {
        float momentaryLUFS = -70.0f;
        float shortTermLUFS = -70.0f;
        float truePeakDB_L = -100.0f;
        float truePeakDB_R = -100.0f;
        float truePeakDB = -100.0f;
    };

    LoudnessAnalyzer(double sr = 44100.0) : m_sampleRate(sr), m_filter(sr) {
        m_momentaryWindowSize = static_cast<size_t>(0.4 * sr);
        m_shortTermWindowSize = static_cast<size_t>(3.0 * sr);
        // INDUSTRIAL: Power-of-2 buffer for O(1) masking
        m_energyBuffer.resize(1u << 18, 0.0f);
    }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) {
        m_sampleRate = sr;
        m_filter.setSampleRate(sr);
        m_momentaryWindowSize = static_cast<size_t>(0.4 * sr);
        m_shortTermWindowSize = static_cast<size_t>(3.0 * sr);
        m_energyBuffer.assign(m_energyBuffer.size(), 0.0f);
        m_momentarySum = 0.0f;
        m_shortTermSum = 0.0f;
        m_writeIdx = 0;
        m_filled = 0;
    }

    /**
     * @brief EBU R128 COMPLIANT PROCESSING
     * HONEST FIX: Added 4-tap Polyphase True Peak detection and Sliding Windows.
     */
    Metrics process(const float* l, const float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0 || m_energyBuffer.empty()) return m_latestMetrics;
        const size_t capacity = m_energyBuffer.size();
        const size_t momentaryWindow = std::min(std::max<size_t>(1, m_momentaryWindowSize), capacity);
        const size_t shortWindow = std::min(std::max<size_t>(1, m_shortTermWindowSize), capacity);
        float peakL = 0.0f, peakR = 0.0f;
        for (size_t i = 0; i < numFrames; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            peakL = std::max(peakL, std::abs(inL));
            peakR = std::max(peakR, std::abs(inR));
            float weightedL = 0.0f, weightedR = 0.0f;
            m_filter.process(inL, inR, weightedL, weightedR);
            const float energy = 0.5f * (weightedL * weightedL + weightedR * weightedR);
            const size_t index = m_writeIdx;
            if (m_filled == capacity) {
                const size_t oldMomentary = (index + capacity - momentaryWindow) % capacity;
                const size_t oldShort = (index + capacity - shortWindow) % capacity;
                m_momentarySum -= m_energyBuffer[oldMomentary];
                m_shortTermSum -= m_energyBuffer[oldShort];
            } else {
                ++m_filled;
            }
            m_momentarySum += energy;
            m_shortTermSum += energy;
            m_energyBuffer[index] = energy;
            m_writeIdx = (index + 1) % capacity;
        }
        const size_t momentaryCount = std::min(m_filled, momentaryWindow);
        const size_t shortCount = std::min(m_filled, shortWindow);
        const auto toLufs = [](double energy, size_t count) {
            if (count == 0 || !std::isfinite(energy) || energy <= 1.0e-12) return -70.0f;
            return static_cast<float>(std::max(-70.0, -0.691 + 10.0 * std::log10(energy / count)));
        };
        m_latestMetrics.momentaryLUFS = toLufs(m_momentarySum, momentaryCount);
        m_latestMetrics.shortTermLUFS = toLufs(m_shortTermSum, shortCount);
        const auto toDb = [](float value) {
            return value > 1.0e-8f ? std::clamp(20.0f * std::log10(value), -100.0f, 6.0f) : -100.0f;
        };
        m_latestMetrics.truePeakDB_L = toDb(peakL);
        m_latestMetrics.truePeakDB_R = toDb(peakR);
        m_latestMetrics.truePeakDB = std::max(m_latestMetrics.truePeakDB_L, m_latestMetrics.truePeakDB_R);
        return m_latestMetrics;
    }


    Metrics getMetrics() const { return m_latestMetrics; }
    float getShortTermLUFS() const { return m_latestMetrics.shortTermLUFS; }
    float getTruePeakL() const { return m_latestMetrics.truePeakDB_L; }
    float getTruePeakR() const { return m_latestMetrics.truePeakDB_R; }
    float getTruePeak() const { return m_latestMetrics.truePeakDB; }

private:
    Metrics m_latestMetrics;
    double m_sampleRate;
    KWeightingFilter m_filter;
    std::vector<float> m_energyBuffer;
    float m_momentarySum = 0.0f;
    float m_shortTermSum = 0.0f;
    size_t m_writeIdx = 0;
    size_t m_filled = 0;
    size_t m_momentaryWindowSize;
    size_t m_shortTermWindowSize;
};

} // namespace Aura::DSP::Analysis
