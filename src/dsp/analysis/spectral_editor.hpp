#pragma once

#include <cstdint>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

// Stable C++ facade for the Rust streaming spectral-restoration engine.
class SpectralEditor {
public:
    static constexpr uint32_t kMaxFFTSize = 4096;
    static constexpr uint32_t kMaxBlockSize = kMaxFFTSize * 4;

    explicit SpectralEditor(uint32_t fftSize = 2048)
        : m_state(hirari_spectral_editor_create(fftSize)) {}

    ~SpectralEditor() { hirari_spectral_editor_destroy(m_state); }

    SpectralEditor(const SpectralEditor&) = delete;
    SpectralEditor& operator=(const SpectralEditor&) = delete;
    SpectralEditor(SpectralEditor&&) = delete;
    SpectralEditor& operator=(SpectralEditor&&) = delete;

    void process(const float* input, float* output, uint32_t len) {
        if (!m_state || !input || !output || len == 0 || len > kMaxBlockSize) return;
        if (input == output) {
            hirari_spectral_editor_process_in_place(m_state, output, len);
        } else {
            hirari_spectral_editor_process(m_state, input, output, len);
        }
    }

    uint32_t latencySamples() const noexcept {
        return hirari_spectral_editor_latency(m_state);
    }
    uint32_t tailSamples() const noexcept {
        return hirari_spectral_editor_tail(m_state);
    }
    void flush(float* output, uint32_t len) {
        if (m_state) hirari_spectral_editor_flush(m_state, output, len);
    }
    void setLearnMode(bool active) {
        hirari_spectral_editor_set_learn(m_state, active);
    }
    void clearNoiseProfile() noexcept {
        hirari_spectral_editor_clear_profile(m_state);
    }
    bool noiseProfileReady() const noexcept {
        return hirari_spectral_editor_profile_ready(m_state);
    }
    void setRestorationActive(bool active) {
        hirari_spectral_editor_set_active(m_state, active);
    }
    void setDenoiseThreshold(float threshold) {
        hirari_spectral_editor_set_threshold(m_state, threshold);
    }
    void requestEraseHarmonics(float fundamental, float sampleRate, float bandwidth) {
        hirari_spectral_editor_set_harmonics(m_state, fundamental, sampleRate, bandwidth);
    }
    void reset() noexcept { hirari_spectral_editor_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
