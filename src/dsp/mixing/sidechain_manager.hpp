#pragma once

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <map>
#include <mutex>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief SidechainBus: A high-performance auxiliary audio buffer for routing control signals.
 */
struct SidechainBus {
    std::array<float, 4096> buffer{};
    uint32_t frames = 0;
    uint32_t sourceTrackId = 0;
};

/**
 * @brief SidechainManager: Global router for track interaction and ducking triggers.
 * Enables professional-grade "Kick vs Bass" ducking and cross-track modulation.
 */
class SidechainManager {
public:
    static SidechainManager& getInstance() {
        static SidechainManager instance;
        return instance;
    }

    /**
     * @brief Writes the current track output into a sidechain bus for others to read.
     */
    // Compatibility-only control-plane API. New audio code must use
    // Aura::Core::Engine::SidechainManager::copySidechainBlock(), which
    // publishes bounded snapshots without exposing internal storage.
    bool writeSource(uint32_t trackId, const float* data, size_t numFrames) {
        if (data == nullptr || numFrames == 0 || numFrames > kMaxBlockSize) return false;
        std::lock_guard<std::mutex> lock(m_busMutex);
        auto& bus = m_buses[trackId];
        std::copy_n(data, numFrames, bus.buffer.begin());
        std::fill(bus.buffer.begin() + static_cast<std::ptrdiff_t>(numFrames),
                  bus.buffer.end(), 0.0f);
        bus.frames = static_cast<uint32_t>(numFrames);
        bus.sourceTrackId = trackId;
        return true;
    }

    size_t copySource(uint32_t trackId, float* destination, size_t capacity) const {
        if (destination == nullptr || capacity == 0) return 0;
        std::lock_guard<std::mutex> lock(m_busMutex);
        const auto it = m_buses.find(trackId);
        if (it == m_buses.end()) return 0;
        const size_t count = std::min<size_t>(it->second.frames, capacity);
        std::copy_n(it->second.buffer.begin(), count, destination);
        return count;
    }

    /**
     * @brief Reads a specific source bus for an effect. (Real-time safe READ).
     */
    // The returned pointer refers to a thread-local copy, never to a buffer
    // owned by the manager. This preserves the old pointer-shaped API while
    // removing the use-after-reallocation/data-race hazard. It is not a
    // realtime API; migrate callers to the core engine snapshot API.
    [[deprecated("use Engine::SidechainManager::copySidechainBlock")]]
    const float* readSource(uint32_t trackId, uint32_t* frames = nullptr) const {
        thread_local std::array<float, kMaxBlockSize> copy{};
        const size_t count = copySource(trackId, copy.data(), copy.size());
        if (count == 0) {
            if (frames != nullptr) *frames = 0;
            return nullptr;
        }
        if (frames != nullptr) *frames = static_cast<uint32_t>(count);
        return copy.data();
    }

private:
    static constexpr size_t kMaxBlockSize = 4096;
    SidechainManager() = default;

    mutable std::map<uint32_t, SidechainBus> m_buses;
    mutable std::mutex m_busMutex;
};

} // namespace Aura::Core::DSP::Mixing
