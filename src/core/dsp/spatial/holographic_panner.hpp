#pragma once

#include <cstdint>

#include "../../rust_ffi.hpp"

namespace Hirari::DSP::Spatial {

/** C++ ownership and call boundary for the Rust Track spatial processor. */
class HolographicPanner {
public:
    static constexpr uint32_t kMaxHrtfTaps = 128;

    HolographicPanner() : m_state(hirari_track_holographic_panner_create()) {}
    ~HolographicPanner() { hirari_track_holographic_panner_destroy(m_state); }

    HolographicPanner(const HolographicPanner&) = delete;
    HolographicPanner& operator=(const HolographicPanner&) = delete;
    HolographicPanner(HolographicPanner&&) = delete;
    HolographicPanner& operator=(HolographicPanner&&) = delete;

    bool setHrtfKernel(const float* left, const float* right, uint32_t taps) noexcept {
        return hirari_track_holographic_panner_set_kernel(m_state, left, right, taps);
    }

    void clearHrtfKernel() noexcept {
        hirari_track_holographic_panner_clear_kernel(m_state);
    }

    void setSampleRate(double sampleRate) noexcept {
        hirari_track_holographic_panner_set_sample_rate(m_state, sampleRate);
    }

    double getSampleRate() const noexcept {
        return hirari_track_holographic_panner_sample_rate(m_state);
    }

    void process(float* left, float* right, uint32_t frames,
                 float x, float y, float z) noexcept {
        hirari_track_holographic_panner_process(m_state, left, right, frames, x, y, z);
    }

    const void* nativeState() const noexcept { return m_state; }

private:
    void* m_state;
};

} // namespace Hirari::DSP::Spatial
