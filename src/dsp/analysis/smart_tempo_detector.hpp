#pragma once

#include <vector>
#include <cmath>
#include <map>
#include <algorithm>
#include "tempo_analyzer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @brief SmartTempoDetector: Logic Pro-style automatic BPM recognition.
 * Analyzes transient energy to guess the project tempo from a raw recording.
 */
class SmartTempoDetector {
public:
    explicit SmartTempoDetector(double sr) : m_sampleRate(std::isfinite(sr) && sr >= 8000.0 ? sr : 44100.0) {}

    /**
     * @brief Detects the primary BPM of an audio buffer with industrial precision and rhythmic sovereignty.
     * INDUSTRIAL: Delegating envelope extraction and autocorrelation to the Rust 'TempoOrchestrator'.
     */
    double detectBPM(const std::vector<float>& buffer) {
        if (buffer.empty()) return 120.0;
        return SmartTempoAnalyzer::detectBPM(buffer.data(), buffer.size(), m_sampleRate).bpm;
    }

private:
    double m_sampleRate;
};

} // namespace Aura::DSP::Analysis
