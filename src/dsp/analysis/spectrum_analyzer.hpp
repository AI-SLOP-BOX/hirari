#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstddef>
#include <cstdint>
#include <vector>

namespace Hirari::DSP::Analysis {

class SpectrumAnalyzer {
public:
    static constexpr uint32_t kFFTSize = 1024;
    static constexpr uint8_t kNumBands = 64;

    explicit SpectrumAnalyzer([[maybe_unused]] double sample_rate = 44100.0)
        : m_state(hirari_spectrum_analyzer_create()) {}

    ~SpectrumAnalyzer() { hirari_spectrum_analyzer_destroy(m_state); }

    SpectrumAnalyzer(const SpectrumAnalyzer&) = delete;
    SpectrumAnalyzer& operator=(const SpectrumAnalyzer&) = delete;

    void process(const float* samples, size_t frames, double sample_rate) {
        hirari_spectrum_analyzer_process(m_state, samples, frames, sample_rate);
    }

    std::vector<float> getCurrentBands() const {
        std::vector<float> bands(kNumBands);
        for (uint32_t band = 0; band < kNumBands; ++band) {
            bands[band] = hirari_spectrum_analyzer_get_band(m_state, band);
        }
        return bands;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
