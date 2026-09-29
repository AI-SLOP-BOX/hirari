#pragma once

#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/**
 * @class Analyzer8kHz
 * @brief Professional High-Frequency Energy Tracker.
 */
class Analyzer8kHz {
public:
    explicit Analyzer8kHz(double sr = 44100.0)
        : m_state(hirari_analyzer_8khz_create(sr)) {}
    ~Analyzer8kHz() { hirari_analyzer_8khz_destroy(m_state); }
    Analyzer8kHz(const Analyzer8kHz&) = delete;
    Analyzer8kHz& operator=(const Analyzer8kHz&) = delete;

    void setSampleRate(double sr) { hirari_analyzer_8khz_set_sample_rate(m_state, sr); }

    /**
     * @brief ACCELERATED BPF ANALYSIS.
     */
    void analyze(const float* buffer, size_t numFrames) {
        hirari_analyzer_8khz_analyze(m_state, buffer, numFrames);
    }

    float getEnergy() const { return hirari_analyzer_8khz_get_energy(m_state); }

private:
    void* m_state = nullptr;
};


} // namespace Hirari::DSP::Analysis
