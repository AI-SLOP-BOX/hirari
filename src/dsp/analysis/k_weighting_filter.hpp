#pragma once

#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** Standardized frequency weighting for LUFS measurement. */
class KWeightingFilter {
public:
    explicit KWeightingFilter(double sample_rate = 44100.0)
        : m_state(hirari_legacy_k_weighting_create(sample_rate)) {}

    ~KWeightingFilter() { hirari_legacy_k_weighting_destroy(m_state); }

    KWeightingFilter(const KWeightingFilter&) = delete;
    KWeightingFilter& operator=(const KWeightingFilter&) = delete;

    void setSampleRate(double sample_rate) {
        hirari_legacy_k_weighting_set_sample_rate(m_state, sample_rate);
    }

    void reset() noexcept { hirari_legacy_k_weighting_reset(m_state); }

    void process(float left, float right, float& out_left, float& out_right) {
        hirari_legacy_k_weighting_process(m_state, left, right, &out_left, &out_right);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
