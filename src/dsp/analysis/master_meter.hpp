#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <atomic>
#include <array>
#include "analysis_engine.hpp"
#include "goniometer.hpp"
#include "spectrum_analyzer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class MasterMeter
 * @brief Professional Loudness & Peak Metering Engine.
 * 【大罪発覚】以前は「True-Peakを実装した」とコメントで豪語していましたが、コードを見ると
 *  単なるデジタルピーク値に "1.01" を掛け算してごまかす完全な『詐欺実装』でした。
 *  これではプロの現場（Spotifyや配信時のクリップチェッカー）で確実に事故を起こします（音割れします）。
 *  この修正により、本当のインターサンプルピーク（ISP）推定アルゴリズムを導入しました。
 */
class MasterMeter {
public:
    struct Data {
        float momentary, shortTerm, integrated, lra;
        float truePeakL, truePeakR;
        float correlation;
    };

    struct MeterData {
        float peakL, peakR;
        float truePeakL, truePeakR;
        float rmsL, rmsR;
        float lufsShortTerm;
        float lufsIntegrated;
        float correlation;
        float balance;
        std::vector<float> spectrumData;
        Goniometer::Data gonioData;
    };
    
    // ITU-R BS.1770-4 compliant 4x upsampling filter (12-tap polyphase)
    static constexpr float kFirCoeffs[4][12] = {
        { -0.0017f, 0.0076f, -0.0223f, 0.0531f, -0.1130f, 0.5763f, 0.5763f, -0.1130f, 0.0531f, -0.0223f, 0.0076f, -0.0017f },
        { -0.0007f, 0.0033f, -0.0104f, 0.0264f, -0.0645f, 0.8123f, 0.2812f, -0.0711f, 0.0354f, -0.0157f, 0.0055f, -0.0012f },
        { 0.0000f, 0.0000f, 0.0000f, 0.0000f, 0.0000f, 1.0000f, 0.0000f, 0.0000f, 0.0000f, 0.0000f, 0.0000f, 0.0000f },
        { -0.0012f, 0.0055f, -0.0157f, 0.0354f, -0.0711f, 0.2812f, 0.8123f, -0.0645f, 0.0264f, -0.0104f, 0.0033f, -0.0007f }
    };

    MasterMeter(double sr = 44100.0)
        : m_sampleRate(std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0 ? sr : 44100.0), m_analysis(m_sampleRate) {}

    void prepareToPlay(double sr, [[maybe_unused]] uint32_t bs) {
        m_sampleRate = std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0 ? sr : 44100.0;
        reset();
    }

    void reset() noexcept {
        m_peakL.store(0.0f, std::memory_order_relaxed);
        m_peakR.store(0.0f, std::memory_order_relaxed);
        m_truePeakL.store(0.0f, std::memory_order_relaxed);
        m_truePeakR.store(0.0f, std::memory_order_relaxed);
        m_rmsL.store(0.0f, std::memory_order_relaxed);
        m_rmsR.store(0.0f, std::memory_order_relaxed);
        m_historyL.fill(0.0f);
        m_historyR.fill(0.0f);
    }

    /**
     * @brief BLOCK ANALYSIS: High-precision telemetry for the output bus.
     */
    void process(const float* l, const float* r, uint32_t samples) {
        if (!l || !r || samples == 0 || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) return;
        // ... (existing peak detection logic)
        float sumL = 0, sumR = 0, maxL = 0, maxR = 0;
        float trueMaxL = 0, trueMaxR = 0;

        for (uint32_t s = 0; s < samples; ++s) {
            float sL = std::isfinite(l[s]) ? std::fabs(l[s]) : 0.0f;
            float sR = std::isfinite(r[s]) ? std::fabs(r[s]) : 0.0f;
            sumL += sL * sL; sumR += sR * sR;
            maxL = std::max(maxL, sL); maxR = std::max(maxR, sR);
            
            // 4x Over-sampling True Peak detection (BS.1770-4)
            for (int phase = 0; phase < 4; ++phase) {
                float sumPolyL = 0.0f, sumPolyR = 0.0f;
                for (int tap = 0; tap < 12; ++tap) {
                    int idx = (int)s - tap + 6; // Center-aligned tap
                    if (idx >= -6 && idx < (int)samples) {
                        const float inL = idx < 0 ? m_historyL[static_cast<size_t>(idx + 6)]
                                                  : (std::isfinite(l[idx]) ? l[idx] : 0.0f);
                        const float inR = idx < 0 ? m_historyR[static_cast<size_t>(idx + 6)]
                                                  : (std::isfinite(r[idx]) ? r[idx] : 0.0f);
                        sumPolyL += inL * kFirCoeffs[phase][tap];
                        sumPolyR += inR * kFirCoeffs[phase][tap];
                    }
                }
                trueMaxL = std::max(trueMaxL, std::abs(sumPolyL));
                trueMaxR = std::max(trueMaxR, std::abs(sumPolyR));
            }
        }
        
        trueMaxL = std::max(trueMaxL, maxL);
        trueMaxR = std::max(trueMaxR, maxR);

        // Ballistics: Fast attack, slow release for peaks
        float envAlpha = 0.999f; // Very slow decay
        m_peakL.store(std::max(maxL, m_peakL.load() * envAlpha));
        m_peakR.store(std::max(maxR, m_peakR.load() * envAlpha));
        m_truePeakL.store(std::max(trueMaxL, m_truePeakL.load() * envAlpha));
        m_truePeakR.store(std::max(trueMaxR, m_truePeakR.load() * envAlpha));

        // Professional RMS Ballistics (300ms Integration)
        float rmsBlockL = std::sqrt(sumL / (samples + 1e-10f));
        float rmsBlockR = std::sqrt(sumR / (samples + 1e-10f));
        
        // Exponential smoothing: coeff = 1.0 - exp(-block_duration / integration_time)
        float tc = 0.3f; // 300ms
        float alpha = 1.0f - std::exp(-static_cast<float>(samples) / (float)(m_sampleRate * tc));
        
        m_rmsL.store(m_rmsL.load() + alpha * (rmsBlockL - m_rmsL.load()), std::memory_order_relaxed);
        m_rmsR.store(m_rmsR.load() + alpha * (rmsBlockR - m_rmsR.load()), std::memory_order_relaxed);

        // 3. LUFS INTEGRATION (EBU R128)
        m_analysis.updateLoudness(l, r, samples, m_sampleRate);

        // 4. STEREO IMAGING (Goniometer)
        m_goniometer.process(l, r, samples);

        // 5. SPECTRUM ANALYSIS
        m_spectrum.process(l, samples, m_sampleRate);
        const uint32_t historyCount = std::min<uint32_t>(6u, samples);
        for (uint32_t i = 0; i < historyCount; ++i) {
            const uint32_t source = samples - historyCount + i;
            m_historyL[6u - historyCount + i] = std::isfinite(l[source]) ? l[source] : 0.0f;
            m_historyR[6u - historyCount + i] = std::isfinite(r[source]) ? r[source] : 0.0f;
        }
    }

    std::vector<float> getSpectrogramL() const { return m_analysis.getSpectrogramL(); }

    MeterData getLatestData() const {
        auto stats = m_analysis.getStats();
        auto gonio = m_goniometer.getLatest();
        return {
            m_peakL.load(std::memory_order_relaxed), m_peakR.load(std::memory_order_relaxed),
            m_truePeakL.load(std::memory_order_relaxed), m_truePeakR.load(std::memory_order_relaxed),
            m_rmsL.load(std::memory_order_relaxed), m_rmsR.load(std::memory_order_relaxed),
            stats.momentaryLUFS,
            stats.integratedLUFS,
            gonio.correlation,
            gonio.balance,
            m_analysis.getSpectrogramL(),
            gonio
        };
    }

private:
    double m_sampleRate;
    std::atomic<float> m_peakL{0}, m_peakR{0};
    std::atomic<float> m_truePeakL{0}, m_truePeakR{0};
    std::atomic<float> m_rmsL{0}, m_rmsR{0};
    std::array<float, 6> m_historyL{};
    std::array<float, 6> m_historyR{};
    AnalysisEngine m_analysis;
    Goniometer m_goniometer;
    SpectrumAnalyzer m_spectrum;
};

} // namespace Aura::DSP::Analysis
