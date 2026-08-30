#pragma once
#include <cmath>
#include <atomic>
#include <algorithm>
#include <string>
#include <array>
#include <limits>
#include "../diagnostics/forensic_kernel.hpp"
#include "../composition/harmonic_context_tracker.hpp"

namespace Aura::Core::Mixing {

/**
 * @class MasteringKernel
 * @brief Master-bus dynamics and finalization processor.
 * HONEST FIX: Implements functional RMS-driven gain control and limiting.
 */
class MasteringKernel {
public:
    MasteringKernel() : m_gainReduction(1.0f), m_currentRMSdBFS(-std::numeric_limits<float>::infinity()), m_targetRMS(0.18f), m_writePointers{nullptr, nullptr} {} // -15dBFS approx

    void setTargetLoudness(float rms) { if (std::isfinite(rms)) m_targetRMS = std::clamp(rms, 1e-6f, 1.0f); }

    void process(float* l, float* r, uint32_t sz) {
        m_writePointers[0] = l;
        m_writePointers[1] = r;
        if (!l || !r || sz == 0) return;
        double sumSq = 0.0;
        for (uint32_t i = 0; i < sz; ++i) {
            const float left = l[i];
            const float right = r[i];
            if (std::isfinite(left)) {
                const double sample = static_cast<double>(left) - m_dcEstimateL;
                sumSq += sample * sample;
            }
            if (std::isfinite(right)) {
                const double sample = static_cast<double>(right) - m_dcEstimateR;
                sumSq += sample * sample;
            }
        }

        const double sampleCount = static_cast<double>(sz) * 2.0;
        const float currentRMS = static_cast<float>(
            std::sqrt(sumSq / sampleCount));
        const float currentRMSdBFS = 20.0f * std::log10(std::max(currentRMS, 1e-6f));
        m_currentRMSdBFS.store(currentRMSdBFS, std::memory_order_relaxed);
        float targetGain = m_targetRMS / (currentRMS + 1e-6f);
        
        // Smooth gain transition (very fast attack/release for AGC)
        float currentGR = m_gainReduction.load(std::memory_order_relaxed);
        float smoothing = 0.01f;
        currentGR = currentGR + (targetGain - currentGR) * smoothing;
        m_gainReduction.store(std::min(1.0f, currentGR), std::memory_order_relaxed);

        float gr = m_gainReduction.load(std::memory_order_relaxed);
        for (uint32_t i = 0; i < sz; ++i) {
            if (std::isfinite(l[i])) {
                m_dcEstimateL += m_dcBlockAlpha * (static_cast<double>(l[i]) - m_dcEstimateL);
                l[i] = static_cast<float>((static_cast<double>(l[i]) - m_dcEstimateL) * gr);
            } else l[i] = 0.0f;
            if (std::isfinite(r[i])) {
                m_dcEstimateR += m_dcBlockAlpha * (static_cast<double>(r[i]) - m_dcEstimateR);
                r[i] = static_cast<float>((static_cast<double>(r[i]) - m_dcEstimateR) * gr);
            } else r[i] = 0.0f;
            
            // Hard Limit to prevent clipping
            l[i] = std::clamp(l[i], -0.99f, 0.99f);
            r[i] = std::clamp(r[i], -0.99f, 0.99f);
        }
    }

    // This is unweighted RMS level, expressed in dBFS; it is not LUFS.
    float getCurrentRMSdBFS() const {
        return m_currentRMSdBFS.load(std::memory_order_relaxed);
    }

    // Kept for source compatibility. The legacy name is misleading: the
    // returned value is RMS dBFS, not standards-compliant LUFS.
    float getLoudnessLUFS() const {
        return getCurrentRMSdBFS();
    }

    float* getWritePointer(int channel) {
        // For compatibility with the engine's hacky access
        if (channel < 0 || channel > 1) return nullptr;
        return m_writePointers[static_cast<size_t>(channel)];
    }

private:
    std::atomic<float> m_gainReduction;
    std::atomic<float> m_currentRMSdBFS;
    float m_targetRMS;
    std::array<float*, 2> m_writePointers;
    double m_dcEstimateL = 0.0;
    double m_dcEstimateR = 0.0;
    static constexpr double m_dcBlockAlpha = 0.0005;
};

} // namespace Aura::Core::Mixing
