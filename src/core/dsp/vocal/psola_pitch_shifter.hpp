#pragma once

#include <cstdint>
#include <utility>

#include "../../rust_ffi.hpp"

namespace Hirari::Core::DSP::Vocal {

/** Low-latency two-tap delay-modulation pitch shifter backed by Rust. */
class DelayModPitchShifter {
public:
    explicit DelayModPitchShifter(double sampleRate = 48000.0)
        : m_state(hirari_delay_pitch_create(sampleRate)) {}
    ~DelayModPitchShifter() { hirari_delay_pitch_destroy(m_state); }

    DelayModPitchShifter(const DelayModPitchShifter& other)
        : m_state(hirari_delay_pitch_clone(other.m_state)) {}
    DelayModPitchShifter& operator=(const DelayModPitchShifter& other) {
        if (this == &other) return *this;
        void* replacement = hirari_delay_pitch_clone(other.m_state);
        if (replacement) {
            hirari_delay_pitch_destroy(m_state);
            m_state = replacement;
        }
        return *this;
    }
    DelayModPitchShifter(DelayModPitchShifter&& other) noexcept
        : m_state(std::exchange(other.m_state, nullptr)) {}
    DelayModPitchShifter& operator=(DelayModPitchShifter&& other) noexcept {
        if (this != &other) {
            hirari_delay_pitch_destroy(m_state);
            m_state = std::exchange(other.m_state, nullptr);
        }
        return *this;
    }

    void reset() noexcept { hirari_delay_pitch_reset(m_state); }

    void process(const float* input, float* output, uint32_t samples,
                 float ratio, float f0, double sampleRate,
                 float formantRatio = 1.0f, float timingRatio = 1.0f) {
        hirari_delay_pitch_process(m_state, input, output, samples, ratio, f0,
                                   sampleRate, formantRatio, timingRatio);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::DSP::Vocal
