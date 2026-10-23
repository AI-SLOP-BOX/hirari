#pragma once
#include <stdint.h>
#include <vector>
#include <cmath>
#include "../audio_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

extern "C" {
void* hirari_region_stretch_create();
void hirari_region_stretch_destroy(void* handle);
bool hirari_region_stretch_prepare(void* handle, double sampleRate, uint32_t maxBlockSize);
bool hirari_region_processor_apply_gain_fade(
    float* samples, uint32_t frames, uint64_t regionRelativePos,
    float gain, uint32_t fadeInSamples, uint8_t fadeType);
void* hirari_region_processor_create();
void hirari_region_processor_destroy(void* state);
void hirari_region_processor_set_gain(void* state, float gain);
void hirari_region_processor_set_fade_in(void* state, uint32_t samples, uint8_t fadeType);
void hirari_region_processor_set_fade_out(void* state, uint32_t samples, uint8_t fadeType);
void hirari_region_processor_process(void* state, float** channels, uint32_t channelCount,
                                     uint32_t samples, uint32_t offset, uint32_t size,
                                     uint64_t regionRelativePos);
}

// Rust owns the streaming state, scratch buffers, source preparation and seek
// decisions. This RAII adapter preserves the current Track-facing API.
class RegionTimeStretch {
public:
    RegionTimeStretch() : m_rustState(hirari_region_stretch_create()) {}
    ~RegionTimeStretch() { hirari_region_stretch_destroy(m_rustState); }
    RegionTimeStretch(const RegionTimeStretch&) = delete;
    RegionTimeStretch& operator=(const RegionTimeStretch&) = delete;

    bool prepare(double sampleRate, uint32_t maxBlockSize) {
        return m_rustState && hirari_region_stretch_prepare(m_rustState, sampleRate, maxBlockSize);
    }

    void* rustState() const noexcept { return m_rustState; }

private:
    void* m_rustState = nullptr;
};

/**
 * @class RegionProcessor
 * @brief High-performance region-based signal manipulation engine.
 * HONEST FIX: Replaced expensive per-sample math with vectorized ramping and lookup.
 */
class RegionProcessor {
public:
    enum class FadeType { Linear, SCurve, Exponential };

    struct FadeInfo {
        uint32_t durationSamples = 0;
        FadeType type = FadeType::Linear;
    };

    RegionProcessor() : m_state(hirari_region_processor_create()) {}
    ~RegionProcessor() { hirari_region_processor_destroy(m_state); }
    RegionProcessor(const RegionProcessor&) = delete;
    RegionProcessor& operator=(const RegionProcessor&) = delete;

    void setGain(float gain) noexcept {
        hirari_region_processor_set_gain(m_state, gain);
    }

    void setFadeIn(uint32_t durationSamples, FadeType type = FadeType::Linear) noexcept {
        hirari_region_processor_set_fade_in(m_state, durationSamples,
                                             static_cast<uint8_t>(type));
    }

    void setFadeOut(uint32_t durationSamples, FadeType type = FadeType::Linear) noexcept {
        hirari_region_processor_set_fade_out(m_state, durationSamples,
                                              static_cast<uint8_t>(type));
    }

    /**
     * @brief PROCESS: Applies gain and fades with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating signal manipulation to the Rust 'RegionProcessorOrchestrator'.
     */
    void process(AudioBuffer& buffer, uint32_t offset, uint32_t size, uint64_t regionRelativePos) noexcept {
        hirari_region_processor_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples(), offset, size, regionRelativePos);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
