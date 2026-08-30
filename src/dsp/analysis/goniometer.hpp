#pragma once
#include <vector>
#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @class Goniometer
 * @brief Professional Stereo Imaging & Phase Correlation Analyzer.
 * Extracts the relationship between Left and Right channels for stereo balance 
 * and phase compatibility checks (Essential for Logic Pro parity).
 */
class Goniometer {
public:
    static constexpr size_t kHistorySize = 1024;
    struct Data {
        float correlation;
        float balance;
        const float* xyHistoryL;
        const float* xyHistoryR;
    };

    Goniometer() {
        m_correlation.store(1.0f);
        m_balance.store(0.0f);
        m_historyIdx.store(0);
        m_activeBuffer.store(0);
        for(int b=0; b<2; ++b) {
            for(size_t i=0; i<kHistorySize; ++i) {
                m_historyL[b][i] = 0; m_historyR[b][i] = 0;
            }
        }
    }

    void process(const float* l, const float* r, uint32_t samples) {
        if (samples == 0) return;

        double sumL = 0, sumR = 0, sumLR = 0;
        size_t hIdx = m_historyIdx.load(std::memory_order_relaxed);
        int activeB = m_activeBuffer.load(std::memory_order_relaxed);

        for (uint32_t s = 0; s < samples; ++s) {
            float sL = l[s];
            float sR = r[s];
            
            if (s % 4 == 0) { 
                m_historyL[activeB][hIdx] = sL;
                m_historyR[activeB][hIdx] = sR;
                hIdx++;
                if (hIdx >= kHistorySize) {
                    hIdx = 0;
                    // Switch buffer when one is full
                    m_activeBuffer.store(1 - activeB, std::memory_order_relaxed);
                    activeB = 1 - activeB;
                }
            }

            sumL += static_cast<double>(sL * sL);
            sumR += static_cast<double>(sR * sR);
            sumLR += static_cast<double>(sL * sR);
        }
        m_historyIdx.store(hIdx, std::memory_order_relaxed);

        // ... (Ballistics logic remains same)
        double denominator = std::sqrt(sumL * sumR) + 1e-12;
        float corr = static_cast<float>(sumLR / denominator);
        float alphaCorr = 0.05f; 
        float prevCorr = m_correlation.load(std::memory_order_relaxed);
        m_correlation.store(prevCorr + alphaCorr * (std::clamp(corr, -1.0f, 1.0f) - prevCorr), std::memory_order_relaxed);

        float totalEnergy = static_cast<float>(sumL + sumR) + 1e-12f;
        float bal = (static_cast<float>(sumR) - static_cast<float>(sumL)) / totalEnergy;
        float alphaBal = 0.1f;
        float prevBal = m_balance.load(std::memory_order_relaxed);
        m_balance.store(prevBal + alphaBal * (std::clamp(bal, -1.0f, 1.0f) - prevBal), std::memory_order_relaxed);
    }

    Data getLatest() const {
        // Return the "inactive" buffer which is currently stable
        int stableB = 1 - m_activeBuffer.load(std::memory_order_relaxed);
        return {
            m_correlation.load(std::memory_order_relaxed),
            m_balance.load(std::memory_order_relaxed),
            m_historyL[stableB],
            m_historyR[stableB]
        };
    }

private:
    std::atomic<float> m_correlation{1.0f};
    std::atomic<float> m_balance{0.0f};
    std::atomic<size_t> m_historyIdx{0};
    std::atomic<int> m_activeBuffer{0};
    float m_historyL[2][kHistorySize];
    float m_historyR[2][kHistorySize];
};

} // namespace Aura::DSP::Analysis
