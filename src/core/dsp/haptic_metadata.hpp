#pragma once
#include <vector>
#include <atomic>
#include "../audio_buffer.hpp"

namespace Aura::Core::DSP {

/**
 * @class HapticMetadataGenerator
 * @brief Extracts tactile energy from audio streams for mobile feedback.
 */
class HapticMetadataGenerator {
public:
    static float extractStress(const AudioBuffer& buffer, uint32_t sz) {
        float peak = 0.0f;
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            const float* data = buffer.getReadPointer(c);
            for (uint32_t i = 0; i < sz; ++i) {
                peak = std::max(peak, std::abs(data[i]));
            }
        }
        return peak;
    }
};

} // namespace Aura::Core::DSP
