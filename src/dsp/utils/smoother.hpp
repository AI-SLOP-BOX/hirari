#pragma once

#include <cstdint>
#include <utility>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Utils {

/** Compatibility API for the Rust-owned linear-ramp parameter smoother. */
class LinearSmoother {
public:
    LinearSmoother() : m_state(hirari_linear_ramp_smoother_create()) {}
    ~LinearSmoother() { hirari_linear_ramp_smoother_destroy(m_state); }

    LinearSmoother(const LinearSmoother& other)
        : m_state(hirari_linear_ramp_smoother_clone(other.m_state)) {}
    LinearSmoother& operator=(const LinearSmoother& other) {
        if (this != &other) {
            void* replacement = hirari_linear_ramp_smoother_clone(other.m_state);
            hirari_linear_ramp_smoother_destroy(m_state);
            m_state = replacement;
        }
        return *this;
    }
    LinearSmoother(LinearSmoother&& other) noexcept
        : m_state(std::exchange(other.m_state, nullptr)) {}
    LinearSmoother& operator=(LinearSmoother&& other) noexcept {
        if (this != &other) {
            hirari_linear_ramp_smoother_destroy(m_state);
            m_state = std::exchange(other.m_state, nullptr);
        }
        return *this;
    }

    void reset(double sampleRate, double timeMs) {
        hirari_linear_ramp_smoother_reset(m_state, sampleRate, timeMs);
    }
    void setTarget(float target) {
        hirari_linear_ramp_smoother_set_target(m_state, target);
    }
    float getNextValue() { return hirari_linear_ramp_smoother_next(m_state); }
    void skip(uint32_t samples) { hirari_linear_ramp_smoother_skip(m_state, samples); }
    float getCurrentValue() const {
        return hirari_linear_ramp_smoother_current(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Utils
