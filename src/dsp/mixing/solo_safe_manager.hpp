#pragma once

#include <set>
#include <mutex>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief SoloSafeManager: Professional "Solo Defeat" system.
 * Prevents Bus/Aux tracks from being muted when other tracks are turned to Solo.
 * Critical Logic Pro feature for reliable effect sub-mixing.
 */
class SoloSafeManager {
public:
    static SoloSafeManager& getInstance() {
        static SoloSafeManager instance;
        return instance;
    }

    /**
     * @brief Marks a track (usually a Reverb Aux) as "Solo Safe."
     */
    void setSoloSafe(uint32_t trackId, bool safe) {
        if (trackId < MaxTracks) {
            m_safeTracks[trackId].store(safe, std::memory_order_release);
        }
    }

    /**
     * @brief Determines if a track should stay audible during global Solo mode.
     * RT-SAFE: Lock-free atomic load.
     */
    bool isProtected(uint32_t trackId) const {
        if (trackId < MaxTracks) {
            return m_safeTracks[trackId].load(std::memory_order_acquire);
        }
        return false;
    }

private:
    SoloSafeManager() {
        for (auto& t : m_safeTracks) t.store(false);
    }

    static constexpr uint32_t MaxTracks = 2048;
    std::atomic<bool> m_safeTracks[MaxTracks];
};

} // namespace Aura::Core::DSP::Mixing
