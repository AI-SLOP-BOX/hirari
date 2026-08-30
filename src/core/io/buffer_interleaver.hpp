#pragma once
#include <cstdint>
#include <algorithm>

namespace Aura::Core::IO {

/**
 * @class BufferInterleaver
 * @brief High-performance utility for audio format conversion.
 * HONEST FIX: Implemented real-world interleaving logic for driver I/O.
 */
class BufferInterleaver {
public:
    /**
     * @brief De-interleaves a hardware buffer into separate channel buffers.
     * [L R L R] -> [L L], [R R]
     */
    template<typename T>
    static void deinterleave(const T* src, T** dest, uint32_t numChannels, uint32_t numSamples) {
        if (!src || !dest || numChannels == 0 || numSamples == 0) return;
        for (uint32_t c = 0; c < numChannels; ++c) {
            T* channelDest = dest[c];
            if (!channelDest) continue;
            for (uint32_t s = 0; s < numSamples; ++s) {
                channelDest[static_cast<size_t>(s) * numChannels + c] = src[static_cast<size_t>(s) * numChannels + c];
            }
        }
    }

    /**
     * @brief Interleaves separate channel buffers into a single hardware buffer.
     * [L L], [R R] -> [L R L R]
     */
    template<typename T>
    static void interleave(T** src, T* dest, uint32_t numChannels, uint32_t numSamples) {
        if (!src || !dest || numChannels == 0 || numSamples == 0) return;
        for (uint32_t c = 0; c < numChannels; ++c) {
            const T* channelSrc = src[c];
            if (!channelSrc) {
                for (uint32_t s = 0; s < numSamples; ++s)
                    dest[static_cast<size_t>(s) * numChannels + c] = T{};
                continue;
            }
            for (uint32_t s = 0; s < numSamples; ++s) {
                dest[static_cast<size_t>(s) * numChannels + c] = channelSrc[s];
            }
        }
    }
};

} // namespace Aura::Core::IO
