#pragma once

#include <cstdint>
#include "rust_ffi.hpp"

namespace Hirari::Core {

/**
 * @brief Native API adapter for the Rust-owned lock-free parameter smoother.
 *
 * The target can be updated from a control thread while the audio thread
 * advances the current value. The audio operations perform no allocation.
 */
class ParameterSmoother {
public:
    explicit ParameterSmoother(float initialValue = 0.0f)
        : m_state(hirari_parameter_smoother_create(initialValue)) {}

    ~ParameterSmoother() { hirari_parameter_smoother_destroy(m_state); }

    ParameterSmoother(const ParameterSmoother&) = delete;
    ParameterSmoother& operator=(const ParameterSmoother&) = delete;
    ParameterSmoother(ParameterSmoother&&) = delete;
    ParameterSmoother& operator=(ParameterSmoother&&) = delete;

    void setTarget(float value) {
        hirari_parameter_smoother_set_target(m_state, value);
    }

    void reset(float value) {
        hirari_parameter_smoother_reset(m_state, value);
    }

    void setSmoothingTime(float milliseconds, float sampleRate) {
        hirari_parameter_smoother_set_time(m_state, milliseconds, sampleRate);
    }

    void process(float* buffer, uint32_t length) {
        hirari_parameter_smoother_process(m_state, buffer, length);
    }

    float getNextValue() {
        return hirari_parameter_smoother_next(m_state);
    }

    float getCurrentValue() const {
        return hirari_parameter_smoother_current(m_state);
    }

private:
    void* m_state;
};

} // namespace Hirari::Core
