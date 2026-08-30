#pragma once

#include <atomic>
#include <cmath>
#include "../utils/panning_law.hpp"
#include "../../core/audio_buffer.hpp"
#include "../../core/atomic_parameter.hpp"
#if defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace Aura::DSP::Mixing {

/**
 * @brief ChannelStrip: Standard mixing unit for mono/stereo sources.
 * HONEST FIX: Replaced raw atomics with AtomicParameter to eliminate zipper noise.
 */
class ChannelStrip {
public:
    ChannelStrip()
        : m_gain(1.0f, Core::AtomicParameter::DisplayMode::Unipolar)
        , m_pan(0.0f, Core::AtomicParameter::DisplayMode::Bipolar) {
        m_gain.setSmoothingTime(20.0);
        m_pan.setSmoothingTime(20.0);
    }

    void setSampleRate(double sr) {
        m_sr = sr;
        m_gain.setSampleRate(sr);
        m_pan.setSampleRate(sr);
    }

    double getSampleRate() const { return m_sr; }

    void reset() {
        // A transport reset must not leave a gain/pan ramp half-way through.
        // Keep the targets, but snap the audio state to them.
        m_gain.resetToTarget();
        m_pan.resetToTarget();
    }

    void setGain(float gain) { m_gain.setTarget(gain); }
    void setPan(float pan) { m_pan.setTarget(pan); } // -1.0 (L) to 1.0 (R)
    void setMute(bool mute) { m_mute.store(mute); }
    void setSolo(bool solo) { m_solo.store(solo); }

    /**
     * @brief Processes a stereo block with vectorized gain/panning.
     */
    void process(Core::AudioBuffer& buffer) {
        process(buffer, 0, buffer.getNumSamples());
    }

    void process(Core::AudioBuffer& buffer, uint32_t offset, uint32_t numSamples, const float* /*extGain*/ = nullptr, const float* /*extPan*/ = nullptr) {
        if (m_mute.load(std::memory_order_relaxed)) {
            buffer.clear(offset, numSamples);
            return;
        }

        if (buffer.getNumChannels() == 0 || numSamples == 0) return;

        if (buffer.getNumChannels() == 1) {
            float* samples = buffer.getWritePointer(0, offset);
            for (uint32_t i = 0; i < numSamples; ++i) {
                // Advance the smoother per sample, not once per block. This
                // prevents audible gain steps when a UI fader moves during
                // playback.
                samples[i] *= m_gain.getNextValue();
            }
            return;
        }

        float* left = buffer.getWritePointer(0, offset);
        float* right = buffer.getWritePointer(1, offset);
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float gain = m_gain.getNextValue();
            const float pan = std::clamp(m_pan.getNextValue(), -1.0f, 1.0f);
            const float angle = (pan + 1.0f) * 0.25f * 3.14159265358979323846f;
            left[i] *= gain * std::cos(angle);
            right[i] *= gain * std::sin(angle);
        }
    }


private:
    Core::AtomicParameter m_gain{1.0f};
    Core::AtomicParameter m_pan{0.0f};
    std::atomic<bool> m_mute{false};
    std::atomic<bool> m_solo{false};
    double m_sr = 44100.0;
};

} // namespace Aura::DSP::Mixing
