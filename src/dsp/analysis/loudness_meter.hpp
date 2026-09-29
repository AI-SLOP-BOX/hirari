#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstdint>

namespace Hirari::DSP::Analysis {

/** Integrated loudness meter backed by its Rust state machine. */
class LoudnessMeter {
public:
    struct Metrics {
        float integratedLUFS = -70.0f;
    };

    explicit LoudnessMeter(double sample_rate = 44100.0)
        : m_state(hirari_loudness_meter_create(sample_rate)) {}
    ~LoudnessMeter() { hirari_loudness_meter_destroy(m_state); }

    LoudnessMeter(const LoudnessMeter&) = delete;
    LoudnessMeter& operator=(const LoudnessMeter&) = delete;

    void setSampleRate(double sample_rate) {
        hirari_loudness_meter_set_sample_rate(m_state, sample_rate);
    }

    void reset() { hirari_loudness_meter_reset(m_state); }

    void process(const float* left, const float* right, uint32_t frames) {
        hirari_loudness_meter_process(m_state, left, right, frames);
    }

    float getIntegratedLUFS() const {
        return hirari_loudness_meter_integrated_lufs(m_state);
    }

    Metrics getMetrics() const { return {getIntegratedLUFS()}; }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
