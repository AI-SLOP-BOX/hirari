#pragma once

#include <cmath>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Mixing {

/** Native API adapter for the Rust-owned zero-delay-feedback SVF. */
class StateVariableFilter {
public:
    explicit StateVariableFilter(double sampleRate = 44100.0)
        : m_state(hirari_svf_create(sampleRate)) {}
    ~StateVariableFilter() { hirari_svf_destroy(m_state); }

    StateVariableFilter(const StateVariableFilter&) = delete;
    StateVariableFilter& operator=(const StateVariableFilter&) = delete;

    void reset() { hirari_svf_reset(m_state); }
    void setSampleRate(double sampleRate) noexcept {
        hirari_svf_set_sample_rate(m_state, sampleRate);
    }
    void setParameters(float frequency, float resonance, int mode = 0) {
        hirari_svf_set_parameters(m_state, frequency, resonance, mode);
    }

    void processBlockLP(float* data, uint32_t frames) {
        hirari_svf_process_block(m_state, data, frames, 0);
    }
    void processBlockBP(float* data, uint32_t frames) {
        hirari_svf_process_block(m_state, data, frames, 1);
    }
    void processBlockHP(float* data, uint32_t frames) {
        hirari_svf_process_block(m_state, data, frames, 2);
    }
    float processSampleLP(float input) {
        return hirari_svf_process_sample(m_state, input, 0);
    }
    float processSampleBP(float input) {
        return hirari_svf_process_sample(m_state, input, 1);
    }
    float processSampleHP(float input) {
        return hirari_svf_process_sample(m_state, input, 2);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Mixing
