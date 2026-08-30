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

    // Clear output buffers
    std::fill(outputL, outputL + numFrames, 0.0f);
    std::fill(outputR, outputR + numFrames, 0.0f);

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
            size_t idx1 = idx0 + 1;

            // Handle looping or end-of-sample
            if (idx0 >= sampleSize) {
                if (m_isLooping && m_loopEnd > m_loopStart && m_loopEnd <= sampleSize) {
                    voice.playbackPos = static_cast<double>(m_loopStart);
                    pos = voice.playbackPos;
                    idx0 = static_cast<size_t>(pos);
                    idx1 = idx0 + 1;
                } else {
                    voice.markAsAvailable();
                    break;
                }
            }

            float frac = static_cast<float>(pos - idx0);

            // Interpolate samples
            float sampleL = leftData[idx0] * (1.0f - frac) + (idx1 < sampleSize ? leftData[idx1] : 0.0f) * frac;
            float sampleR = rightData[idx0] * (1.0f - frac) + (idx1 < sampleSize ? rightData[idx1] : 0.0f) * frac;

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
            voice.playbackPos += voice.currentSpeed;
        }
    }
}

} // namespace Aura::Core::DSP::Synthesis
