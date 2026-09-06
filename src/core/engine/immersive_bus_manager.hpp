#pragma once

#include <vector>
#include <array>
#include <mutex>
#include <memory>
#include <algorithm>
#include "../audio_buffer.hpp"

namespace Aura::Core::Engine {

/**
 * @class ImmersiveBusManager
 * @brief Professional High-Density 3D Routing Infrastructure.
 * 
 * Manages up to 1024 virtual immersive busses for Atmos and spatial production.
 */
class ImmersiveBusManager {
public:
    static constexpr int kMaxBusses = 1024;
    static constexpr int kChannelsPerBus = 12; // 7.1.4 Support

    static ImmersiveBusManager& getInstance() { static ImmersiveBusManager i; return i; }

    void writeToBus(uint32_t busId, const AudioBuffer& buffer) {
        if (busId >= kMaxBusses || buffer.isEmpty()) return;
        std::lock_guard<std::mutex> lock(m_mutexes[busId]);
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), kChannelsPerBus);
        const uint32_t samples = buffer.getNumSamples();
        if (!m_busBuffers[busId] || m_busBuffers[busId]->getNumChannels() != channels ||
            m_busBuffers[busId]->getNumSamples() != samples) {
            std::lock_guard<std::mutex> allocLock(m_allocationMutex);
            auto next = std::make_unique<AudioBuffer>(channels, samples);
            if (!next || next->isEmpty()) return;
            m_busBuffers[busId] = std::move(next);
        }
        auto& destination = *m_busBuffers[busId];
        destination.clear();
        for (uint32_t c = 0; c < channels; ++c) {
            const float* src = buffer.getReadPointer(c);
            float* dst = destination.getWritePointer(c);
            if (src && dst) std::copy_n(src, samples, dst);
        }
    }

    void readFromBus(uint32_t busId, AudioBuffer& target) {
        if (busId >= kMaxBusses) return;
        std::lock_guard<std::mutex> lock(m_mutexes[busId]);
        const auto& source = m_busBuffers[busId];
        if (!source || source->isEmpty()) { target.clear(); return; }
        const uint32_t channels = std::min(source->getNumChannels(), target.getNumChannels());
        const uint32_t samples = std::min(source->getNumSamples(), target.getNumSamples());
        for (uint32_t c = 0; c < channels; ++c) {
            const float* src = source->getReadPointer(c);
            float* dst = target.getWritePointer(c);
            if (src && dst) std::copy_n(src, samples, dst);
        }
    }

private:
    ImmersiveBusManager() = default;

    std::array<std::unique_ptr<AudioBuffer>, kMaxBusses> m_busBuffers;
    std::mutex m_mutexes[kMaxBusses];
    std::mutex m_allocationMutex; // For industrial thread-safe lazy init
};

} // namespace Aura::Core::Engine
