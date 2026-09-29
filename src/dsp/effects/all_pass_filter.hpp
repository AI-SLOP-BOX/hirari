#pragma once

#include <cstddef>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Rust-owned all-pass diffusion line with a small C++ compatibility wrapper. */
class AllPassFilter {
public:
    AllPassFilter(size_t delaySamples, float feedback)
        : m_state(hirari_all_pass_create(delaySamples, feedback)) {}

    ~AllPassFilter() { hirari_all_pass_destroy(m_state); }

    AllPassFilter(const AllPassFilter&) = delete;
    AllPassFilter& operator=(const AllPassFilter&) = delete;

    AllPassFilter(AllPassFilter&& other) noexcept : m_state(other.m_state) {
        other.m_state = nullptr;
    }

    AllPassFilter& operator=(AllPassFilter&& other) noexcept {
        if (this != &other) {
            hirari_all_pass_destroy(m_state);
            m_state = other.m_state;
            other.m_state = nullptr;
        }
        return *this;
    }

    float process(float input) noexcept { return hirari_all_pass_process(m_state, input); }
    void reset() noexcept { hirari_all_pass_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
