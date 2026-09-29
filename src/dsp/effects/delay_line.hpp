#pragma once

#include <cstdint>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Rust-backed integer-sample delay line used by PDC and lookahead DSP. */
class DelayLine {
public:
    explicit DelayLine(uint32_t maxDelaySamples)
        : m_state(hirari_integer_delay_create(maxDelaySamples)) {}

    ~DelayLine() { hirari_integer_delay_destroy(m_state); }

    DelayLine(const DelayLine&) = delete;
    DelayLine& operator=(const DelayLine&) = delete;

    DelayLine(DelayLine&& other) noexcept : m_state(other.m_state) {
        other.m_state = nullptr;
    }

    DelayLine& operator=(DelayLine&& other) noexcept {
        if (this != &other) {
            hirari_integer_delay_destroy(m_state);
            m_state = other.m_state;
            other.m_state = nullptr;
        }
        return *this;
    }

    float process(float sample, uint32_t delaySamples) noexcept {
        return hirari_integer_delay_process(m_state, sample, delaySamples);
    }

    void push(float sample) noexcept {
        hirari_integer_delay_push(m_state, sample);
    }

    float read(uint32_t delaySamples) const noexcept {
        return hirari_integer_delay_read(m_state, delaySamples);
    }

    void reset() noexcept { hirari_integer_delay_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
