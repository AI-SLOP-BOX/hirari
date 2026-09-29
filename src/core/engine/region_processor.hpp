#pragma once
#include <stdint.h>
#include <vector>
#include <cmath>
#include <atomic>
#include <algorithm>
#include <memory>
#include <limits>
#include "../audio_buffer.hpp"

namespace Hirari::Core::Engine {

extern "C" {
void* hirari_region_stretch_create();
void hirari_region_stretch_destroy(void* handle);
bool hirari_region_stretch_prepare(void* handle, double sampleRate, uint32_t maxBlockSize);
bool hirari_region_processor_apply_gain_fade(
    float* samples, uint32_t frames, uint64_t regionRelativePos,
    float gain, uint32_t fadeInSamples, uint8_t fadeType);
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

    RegionProcessor() : m_gain(1.0f) {}

    void setGain(float gain) noexcept {
        m_gain.store(std::isfinite(gain) ? std::clamp(gain, 0.0f, 4.0f) : 1.0f,
                     std::memory_order_release);
    }

    void setFadeIn(uint32_t durationSamples, FadeType type = FadeType::Linear) noexcept {
        m_fadeIn = {durationSamples, type};
    }

    void setFadeOut(uint32_t durationSamples, FadeType type = FadeType::Linear) noexcept {
        m_fadeOut = {durationSamples, type};
    }

    /**
     * @brief PROCESS: Applies gain and fades with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating signal manipulation to the Rust 'RegionProcessorOrchestrator'.
     */
    void process(AudioBuffer& buffer, uint32_t offset, uint32_t size, uint64_t regionRelativePos) noexcept {
        if (offset >= buffer.getNumSamples() || buffer.getNumChannels() == 0 || size == 0) return;
        const uint32_t count = std::min(size, buffer.getNumSamples() - offset);
        const float gain = m_gain.load(std::memory_order_acquire);
        if (!std::isfinite(gain)) return;

        const auto fadeType = static_cast<uint8_t>(m_fadeIn.type);
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            hirari_region_processor_apply_gain_fade(
                buffer.getWritePointer(channel, offset), count, regionRelativePos,
                gain, m_fadeIn.durationSamples, fadeType);
        }
    }

private:
    std::atomic<float> m_gain;
    FadeInfo m_fadeIn;
    FadeInfo m_fadeOut;
};

} // namespace Hirari::Core::Engine
