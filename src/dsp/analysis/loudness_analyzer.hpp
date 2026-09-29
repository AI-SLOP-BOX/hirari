#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstddef>
#include <cstdint>

namespace Hirari::DSP::Analysis {

class LoudnessAnalyzer {
public:
    struct Metrics {
        float momentaryLUFS = -70.0f;
        float shortTermLUFS = -70.0f;
        float truePeakDB_L = -100.0f;
        float truePeakDB_R = -100.0f;
        float truePeakDB = -100.0f;
    };

    explicit LoudnessAnalyzer(double sample_rate = 44100.0)
        : m_state(hirari_loudness_analyzer_create(sample_rate)) {}
    ~LoudnessAnalyzer() { hirari_loudness_analyzer_destroy(m_state); }

    LoudnessAnalyzer(const LoudnessAnalyzer&) = delete;
    LoudnessAnalyzer& operator=(const LoudnessAnalyzer&) = delete;

    void prepareToPlay(double sample_rate, uint32_t /*block_size*/) {
        hirari_loudness_analyzer_prepare(m_state, sample_rate);
    }

    Metrics process(const float* left, const float* right, size_t frames) {
        if (!left || !right || frames == 0) return getMetrics();
        return fromRust(hirari_loudness_analyzer_process(m_state, left, right, frames));
    }

    Metrics getMetrics() const {
        return fromRust(hirari_loudness_analyzer_get_metrics(m_state));
    }
    float getShortTermLUFS() const { return getMetrics().shortTermLUFS; }
    float getTruePeakL() const { return getMetrics().truePeakDB_L; }
    float getTruePeakR() const { return getMetrics().truePeakDB_R; }
    float getTruePeak() const { return getMetrics().truePeakDB; }

private:
    static Metrics fromRust(HirariLoudnessMetrics value) {
        return {value.momentary_lufs, value.short_term_lufs,
                value.true_peak_db_l, value.true_peak_db_r, value.true_peak_db};
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
