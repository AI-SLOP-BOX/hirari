#pragma once
#include <cmath>
#include <vector>
#include <algorithm>
#include <atomic>
#include <limits>
#include "../audio_buffer.hpp"

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::Core::Engine {

/**
 * @class Metronome
 * @brief Generates sample-accurate click signals for transport synchronization.
 */
class Metronome {
public:
    explicit Metronome(double sr = 44100.0) { setSampleRate(sr); }

    void setSampleRate(double sr) noexcept {
        m_sampleRate = (std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0) ? sr : 44100.0;
        reset();
    }
    void reset() noexcept {
        m_clickSampleCount = 0;
        m_clickPhase = m_clickPhaseStep = m_clickEnvelope = m_clickEnvelopeDecay = 0.0f;
        m_lastPlayhead = 0;
        m_hasPlayhead = false;
    }

    void setEnabled(bool e) { m_isEnabled.store(e, std::memory_order_release); }
    bool isEnabled() const { return m_isEnabled.load(std::memory_order_acquire); }

    /**
     * @brief Render metronome click pulses into the output buffers on beat boundaries.
     */
    void process(float* l, float* r, uint32_t numSamples, uint64_t playhead, double sr, double bpm) {
        if (!l || !r || numSamples == 0 || !m_isEnabled.load(std::memory_order_acquire) ||
            !std::isfinite(bpm) || bpm <= 0.0 || !std::isfinite(sr) || sr < 8000.0 || sr > 384000.0) return;

        // A callback can be handed a block whose end crosses uint64_t's
        // boundary only after an astronomically long session.  Do not wrap
        // the sample position used for beat detection in that case.
        if (playhead > std::numeric_limits<uint64_t>::max() - numSamples) return;

        double samplesPerBeat = (60.0 / bpm) * sr;

        const bool transportJumpedBack = m_hasPlayhead && playhead < m_lastPlayhead;

        for (uint32_t i = 0; i < numSamples; ++i) {
            uint64_t pos = playhead + i;

            // Check if pos crosses a beat boundary
            // We calculate if there is an integer beat within [pos, pos+1)
            double currentBeat = static_cast<double>(pos) / samplesPerBeat;
            double nextBeat = static_cast<double>(pos + 1) / samplesPerBeat;

            const bool startsAtTransportHead = i == 0 && (!m_hasPlayhead || transportJumpedBack);
            if (startsAtTransportHead || std::floor(currentBeat) != std::floor(nextBeat)) {
                // Beat boundary crossed!
                const uint64_t beatIdx = startsAtTransportHead
                    ? static_cast<uint64_t>(std::floor(currentBeat))
                    : static_cast<uint64_t>(std::floor(nextBeat));

                // Downbeat (beat 1 of 4) gets a higher pitch click
                float freq = (beatIdx % 4 == 0) ? 1000.0f : 800.0f;

                m_clickSampleCount = static_cast<uint32_t>(0.04f * sr); // 40ms pulse
                m_clickPhase = 0.0f;
                m_clickPhaseStep = static_cast<float>(2.0 * M_PI * freq / sr);
                m_clickEnvelope = 0.4f; // Volume level
                m_clickEnvelopeDecay = m_clickEnvelope / m_clickSampleCount;
            }

            if (m_clickSampleCount > 0) {
                float clickVal = std::sin(m_clickPhase) * m_clickEnvelope;
                l[i] = std::isfinite(l[i]) ? std::clamp(l[i] + clickVal, -16.0f, 16.0f) : clickVal;
                r[i] = std::isfinite(r[i]) ? std::clamp(r[i] + clickVal, -16.0f, 16.0f) : clickVal;

                m_clickPhase += m_clickPhaseStep;
                m_clickEnvelope = std::max(0.0f, m_clickEnvelope - m_clickEnvelopeDecay);
                m_clickSampleCount--;
            }
        }
        m_lastPlayhead = playhead + numSamples;
        m_hasPlayhead = true;
    }

private:
    std::atomic<bool> m_isEnabled{false};
    double m_sampleRate = 44100.0;

    // Click generator state
    uint32_t m_clickSampleCount = 0;
    float m_clickPhase = 0.0f;
    float m_clickPhaseStep = 0.0f;
    float m_clickEnvelope = 0.0f;
    float m_clickEnvelopeDecay = 0.0f;
    uint64_t m_lastPlayhead = 0;
    bool m_hasPlayhead = false;
};

} // namespace Aura::Core::Engine
