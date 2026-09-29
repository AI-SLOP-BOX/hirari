#pragma once

#include "../../core/rust_ffi.hpp"

#include <algorithm>
#include <cstddef>
#include <vector>

namespace Hirari::DSP::Analysis {

class SpectralMatcher {
public:
    static constexpr size_t kBins = 4096 / 2;

    SpectralMatcher() : m_state(hirari_spectral_matcher_create()) {}
    ~SpectralMatcher() { hirari_spectral_matcher_destroy(m_state); }

    SpectralMatcher(const SpectralMatcher&) = delete;
    SpectralMatcher& operator=(const SpectralMatcher&) = delete;

    void setReference(const std::vector<float>& target) {
        setReference(target.data(), target.size());
    }

    void setReference(const float* target, size_t length) {
        hirari_spectral_matcher_set_reference(m_state, target, length);
    }

    void updateAverage(const std::vector<float>& magnitudes) {
        updateAverage(magnitudes.data(), magnitudes.size());
    }

    void updateAverage(const float* magnitudes, size_t length) {
        hirari_spectral_matcher_update_average(m_state, magnitudes, length);
    }

    std::vector<float> calculateMatchCurve() const {
        std::vector<float> curve(kBins);
        hirari_spectral_matcher_calculate(m_state, curve.data(), curve.size());
        return curve;
    }

    void setPinkNoiseReference() {
        hirari_spectral_matcher_set_pink_noise_reference(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
