#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstdint>
#include <vector>

namespace Hirari::DSP::Analysis {

struct LoudnessStats {
    float momentaryLUFS = -70.0f;
    float shortTermLUFS = -70.0f;
    float integratedLUFS = -70.0f;
    float truePeakDB = -100.0f;
};

class AnalysisEngine {
public:
    explicit AnalysisEngine(double sample_rate = 44100.0)
        : m_state(hirari_analysis_engine_create(sample_rate)) {}

    ~AnalysisEngine() { hirari_analysis_engine_destroy(m_state); }

    AnalysisEngine(const AnalysisEngine&) = delete;
    AnalysisEngine& operator=(const AnalysisEngine&) = delete;

    void updateLoudness(const float* left, const float* right, uint32_t frames, double sample_rate) {
        hirari_analysis_engine_update(m_state, left, right, frames, sample_rate);
    }

    LoudnessStats getStats() const {
        float values[4] = {-70.0f, -70.0f, -70.0f, -100.0f};
        hirari_analysis_engine_get_stats(m_state, values);
        return {values[0], values[1], values[2], values[3]};
    }

    std::vector<float> getSpectrogramL() const { return getSpectrogram(0); }
    std::vector<float> getSpectrogramR() const { return getSpectrogram(1); }

private:
    static constexpr uint32_t kNumBands = 64;

    std::vector<float> getSpectrogram(uint32_t channel) const {
        std::vector<float> bands(kNumBands);
        for (uint32_t band = 0; band < kNumBands; ++band) {
            bands[band] = hirari_analysis_engine_get_band(m_state, channel, band);
        }
        return bands;
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
