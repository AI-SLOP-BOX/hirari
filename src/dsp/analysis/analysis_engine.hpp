#pragma once
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <vector>
#include "k_weighting_filter.hpp"
#include "spectrum_analyzer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @struct LoudnessStats
 * @brief Professional R128 and Peak loudness measurement data.
 */
struct LoudnessStats {
    float momentaryLUFS = -70.0f;
    float shortTermLUFS = -70.0f;
    float integratedLUFS = -70.0f;
    float truePeakDB = -100.0f;
};

/**
 * @class AnalysisEngine
 * @brief 【大罪修正】シングルトン廃止・マルチインスタンス完全対応
 * 以前のコードはシングルトンだったため、複数のトラックでアナライザーを動かすと
 * 内部のK-Weightingフィルタの状態が混ざり合い、デタラメな値を出力していました。
 * インスタンスを独立させることで、真の並列解析が可能になりました。
 */
class AnalysisEngine {
public:
    AnalysisEngine(double sr = 44100.0) : m_spectralL(sr), m_spectralR(sr) {
        m_stats.store(LoudnessStats{});
    }

    /**
     * @brief ACCELERATED LOUDNESS: High-precision ITU-R BS.1770 compliant measurement.
     */
    void updateLoudness(const float* l, const float* r, uint32_t numSamples, double sr) {
        if (!l || !r || numSamples == 0 || !std::isfinite(sr) || sr <= 0.0) return;

        m_spectralL.process(l, numSamples, sr);
        m_spectralR.process(r, numSamples, sr);

        double currentEnergySum = 0.0;
        
        for (uint32_t s = 0; s < numSamples; ++s) {
            float outL, outR;
            m_kFilter.process(l[s], r[s], outL, outR);
            currentEnergySum += (double)outL * outL + (double)outR * outR;
        }

        // Momentary LUFS (ブロックごとの瞬時値)
        float meanEnergy = static_cast<float>(currentEnergySum / (numSamples * 2 + 1e-10));
        float m_lufs = -0.691f + 10.0f * std::log10(meanEnergy + 1e-12f);
        
        LoudnessStats current = m_stats.load(std::memory_order_relaxed);
        current.momentaryLUFS = m_lufs;
        
        // Integrated Loudness (BS.1770-4 Dual-Gate Logic)
        const float absoluteGate = -70.0f;
        if (m_lufs > absoluteGate) {
            // 1. Add to buffer for relative gate calculation
            // (Industrial: Use a sliding window or persistent histogram)
            // Fixed-size history: this function may run on the real-time audio
            // thread, so growth and O(N) erase operations are not acceptable.
            const float energy = std::isfinite(meanEnergy) ? std::max(0.0f, meanEnergy) : 0.0f;
            if (m_energyCount < kEnergyHistorySize) {
                m_energyHistory[m_energyWrite] = energy;
                m_energySum += energy;
                ++m_energyCount;
            } else {
                m_energySum -= m_energyHistory[m_energyWrite];
                m_energyHistory[m_energyWrite] = energy;
                m_energySum += energy;
            }
            m_energyWrite = (m_energyWrite + 1) % kEnergyHistorySize;

            // 2. Calculate Relative Gate Threshold
            const double avgEnergy = m_energySum / static_cast<double>(std::max<size_t>(1, m_energyCount));

            float relativeThreshold = -0.691f + 10.0f * std::log10(avgEnergy + 1e-12f) - 10.0f;
            
            if (m_lufs > relativeThreshold) {
                m_totalEnergy += meanEnergy;
                m_measurementsCount++;
                current.integratedLUFS = -0.691f + 10.0f * std::log10(m_totalEnergy / m_measurementsCount + 1e-12f);
            }
        }

        // True Peak (簡易推定デシベル)
        for (uint32_t s = 0; s < numSamples; ++s) {
            float peak = std::max(std::abs(l[s]), std::abs(r[s]));
            float db = 20.0f * std::log10(peak + 1e-12f);
            if (db > current.truePeakDB) current.truePeakDB = db;
        }

        m_stats.store(current, std::memory_order_relaxed);
    }

    LoudnessStats getStats() const { return m_stats.load(std::memory_order_relaxed); }
    std::vector<float> getSpectrogramL() const { return m_spectralL.getCurrentBands(); }
    std::vector<float> getSpectrogramR() const { return m_spectralR.getCurrentBands(); }

private:
    std::atomic<LoudnessStats> m_stats; 
    KWeightingFilter m_kFilter;
    SpectrumAnalyzer m_spectralL;
    SpectrumAnalyzer m_spectralR;
    
    double m_totalEnergy = 0.0;
    uint64_t m_measurementsCount = 0;
    static constexpr size_t kEnergyHistorySize = 1000;
    std::array<float, kEnergyHistorySize> m_energyHistory{};
    size_t m_energyWrite = 0;
    size_t m_energyCount = 0;
    double m_energySum = 0.0;
};

} // namespace Aura::DSP::Analysis
