#pragma once

#include "../../core/rust_ffi.hpp"
#include <cstdint>

namespace Hirari::DSP::Analysis {

class SmartTempoAnalyzer {
public:
    struct AnalysisResult {
        float bpm;
        float confidence;
    };

    static AnalysisResult detectBPM(const float* data, uint64_t length, double sample_rate) {
        const auto result = hirari_tempo_analyzer_detect_bpm(data, length, sample_rate);
        return {result.bpm, result.confidence};
    }
};

} // namespace Hirari::DSP::Analysis
