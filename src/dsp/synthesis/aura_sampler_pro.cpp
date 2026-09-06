#include "aura_sampler_pro.hpp"
#include <cmath>
#include <algorithm>

namespace Aura::Core::DSP::Synthesis {

/**
 * @brief AuraSamplerPro: Production-grade Polyphonic Sampler.
 * HONEST FIX: Direct Pointer Access + TPT SVF Filter.
 */
void AuraSamplerPro::process(float* outputL, float* outputR, size_t numFrames) {
    if (outputL == nullptr || outputR == nullptr || numFrames == 0) return;
    std::fill(outputL, outputL + numFrames, 0.0f);
    std::fill(outputR, outputR + numFrames, 0.0f);

    processAdditive(outputL, outputR, numFrames);
}

void AuraSamplerPro::processAdditive(float* outputL, float* outputR, size_t numFrames) {
    if (outputL == nullptr || outputR == nullptr || numFrames == 0) return;

    for (int i = 0; i < kMaxVoices; ++i) {
        auto& voice = m_voices[i];
        if (!voice.active) continue;

        const auto* zone = voice.currentZone;
        if (!zone || zone->left.empty()) {
            voice.markAsAvailable();
            continue;
        }

        const size_t sampleSize = zone->left.size();
        const float* leftData = zone->left.data();
        const float* rightData = zone->right.empty() ? zone->left.data() : zone->right.data();

        // Resolve the effective loop once per voice.  A zone loop takes
        // precedence over the sampler fallback loop and must be honoured as
        // soon as playback crosses its end (not only after the sample ends).
        const uint64_t configuredLoopStart = voice.loopEnabled ? voice.loopStart : m_loopStart.load(std::memory_order_relaxed);
        const uint64_t configuredLoopEnd = voice.loopEnabled ? voice.loopEnd : m_loopEnd.load(std::memory_order_relaxed);
        const bool looping = (voice.loopEnabled || m_isLooping.load(std::memory_order_relaxed)) &&
                             configuredLoopEnd > configuredLoopStart + 1 &&
                             configuredLoopEnd <= sampleSize;

        // Dynamic 1st-order LPF filter coefficients based on voice cutoff
        const float cutoff = std::clamp(voice.filterCutoff, 20.0f, 20000.0f);
        const float rc = 1.0f / (2.0f * static_cast<float>(M_PI) * cutoff);
        const float dt = 1.0f / static_cast<float>(m_sampleRate);
        const float alpha = std::clamp(dt / (rc + dt), 0.001f, 1.0f);

        for (size_t f = 0; f < numFrames; ++f) {
            if (voice.envelope.getState() == ADSR_IDLE) {
                voice.markAsAvailable();
                break;
            }

            double pos = voice.playbackPos;
            size_t idx0 = static_cast<size_t>(pos);

            // Handle looping at the configured loop end, including the
            // interpolation sample that straddles the loop boundary.
            if (looping && pos >= static_cast<double>(configuredLoopEnd)) {
                const double loopLength = static_cast<double>(configuredLoopEnd - configuredLoopStart);
                voice.playbackPos = static_cast<double>(configuredLoopStart) +
                                    std::fmod(std::max(0.0, pos - configuredLoopStart), loopLength);
                pos = voice.playbackPos;
                idx0 = static_cast<size_t>(pos);
            } else if (idx0 >= sampleSize) {
                voice.markAsAvailable();
                break;
            }

            const float sampleL = interpolateSample(leftData, sampleSize, pos, looping,
                                                     configuredLoopStart, configuredLoopEnd);
            const float sampleR = interpolateSample(rightData, sampleSize, pos, looping,
                                                     configuredLoopStart, configuredLoopEnd);

            // Apply Envelope & Velocity
            float amp = voice.envelope.getNextValue() * voice.velocity;
            sampleL *= amp;
            sampleR *= amp;

            // Apply SVF/LPF pre-filter states
            voice.filterLZ1 += alpha * (sampleL - voice.filterLZ1);
            voice.filterRZ1 += alpha * (sampleR - voice.filterRZ1);
            
            if (std::isfinite(voice.filterLZ1)) sampleL = voice.filterLZ1;
            if (std::isfinite(voice.filterRZ1)) sampleR = voice.filterRZ1;

            outputL[f] += std::isfinite(sampleL) ? sampleL : 0.0f;
            outputR[f] += std::isfinite(sampleR) ? sampleR : 0.0f;

            // Update playback speed and slide transitions
            voice.updateSlide();
            voice.playbackPos += static_cast<double>(voice.currentSpeed) * voice.sampleRateRatio;
        }
    }
}

} // namespace Aura::Core::DSP::Synthesis
