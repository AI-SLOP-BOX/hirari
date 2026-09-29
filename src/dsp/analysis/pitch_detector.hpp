#pragma once

#include <cstddef>
#include <cstdint>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** Native API adapter for the Rust pitch estimation implementation. */
class PitchDetector {
public:
    explicit PitchDetector(double sampleRate = 44100.0) : m_sampleRate(sampleRate) {}

    float estimateFrequency(const float* buffer, size_t size) const {
        return hirari_pitch_detector_estimate(buffer, size, m_sampleRate);
    }

    static uint8_t frequencyToMidi(float frequency) {
        return hirari_pitch_detector_frequency_to_midi(frequency);
    }

private:
    double m_sampleRate;
};

} // namespace Hirari::DSP::Analysis
