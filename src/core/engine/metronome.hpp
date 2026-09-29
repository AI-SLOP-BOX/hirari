#pragma once

#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade for the transport. Rust owns click timing, oscillator,
// envelope and rendering; C++ only forwards the existing engine API.
class Metronome {
public:
    explicit Metronome(double sampleRate = 44100.0)
        : m_state(hirari_metronome_create(sampleRate)) {}

    ~Metronome() { hirari_metronome_destroy(m_state); }

    Metronome(const Metronome&) = delete;
    Metronome& operator=(const Metronome&) = delete;

    void setSampleRate(double sampleRate) noexcept {
        hirari_metronome_set_sample_rate(m_state, sampleRate);
    }

    void reset() noexcept { hirari_metronome_reset(m_state); }

    void setEnabled(bool enabled) noexcept {
        hirari_metronome_set_enabled(m_state, enabled);
    }

    bool isEnabled() const noexcept {
        return hirari_metronome_is_enabled(m_state);
    }

    void process(float* left, float* right, uint32_t numSamples,
                 uint64_t playhead, double sampleRate, double bpm) noexcept {
        hirari_metronome_process(
            m_state, left, right, numSamples, playhead, sampleRate, bpm);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
