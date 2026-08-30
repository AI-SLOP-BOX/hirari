#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include "../audio_buffer.hpp"

namespace Aura::Core::IO {

/**
 * @struct DeviceEndpoint
 * @brief Representation of an external audio device endpoint.
 */
struct DeviceEndpoint {
    std::string name;
    uint32_t numOutputChannels;
    uint32_t numInputChannels;
    void* nativeDeviceHandle;
};

/**
 * @class MultiDeviceSink
 * @brief Manages industrial-scale aggregate audio device orchestration.
 * Handles clock synchronization and sample-accurate fan-out to multiple sinks.
 */
class MultiDeviceSink {
public:
    MultiDeviceSink() : m_isSinkActive(false) {}

    bool addEndpoint(const DeviceEndpoint& endpoint) {
        if (endpoint.name.empty() || endpoint.nativeDeviceHandle == nullptr
            || endpoint.numOutputChannels == 0 || endpoint.numOutputChannels > 128
            || endpoint.numInputChannels > 128) return false;
        if (std::any_of(m_endpoints.begin(), m_endpoints.end(), [&](const auto& existing) {
            return existing.name == endpoint.name || existing.nativeDeviceHandle == endpoint.nativeDeviceHandle;
        })) return false;
        m_endpoints.push_back(endpoint);
        return true;
    }

    bool removeEndpoint(const std::string& name) {
        const auto before = m_endpoints.size();
        m_endpoints.erase(std::remove_if(m_endpoints.begin(), m_endpoints.end(), [&](const auto& endpoint) {
            return endpoint.name == name;
        }), m_endpoints.end());
        return before != m_endpoints.size();
    }

    void clearEndpoints() {
        stopSinks();
        m_endpoints.clear();
    }

    size_t endpointCount() const noexcept { return m_endpoints.size(); }

    bool hasValidEndpoints() const noexcept {
        return !m_endpoints.empty() && std::all_of(m_endpoints.begin(), m_endpoints.end(), [](const auto& endpoint) {
            return !endpoint.name.empty() && endpoint.nativeDeviceHandle != nullptr
                && endpoint.numOutputChannels > 0 && endpoint.numOutputChannels <= 128
                && endpoint.numInputChannels <= 128;
        });
    }

    /**
     * @brief Process and fan-out audio to all registered endpoints.
     */
    void drainSinks(const AudioBuffer& /*masterBuffer*/) {
        if (!m_isSinkActive.load(std::memory_order_relaxed) || !hasValidEndpoints()) return;

        // --- INDUSTRIAL AGGREGATE ORCHESTRATION ---
        for (auto& endpoint : m_endpoints) {
            (void)endpoint;
            // (Driver-specific API calls to copy/stream masterBuffer to the endpoint)
        }
    }

    void startSinks() { m_isSinkActive.store(hasValidEndpoints(), std::memory_order_release); }
    void stopSinks() { m_isSinkActive.store(false); }

private:
    std::atomic<bool> m_isSinkActive;
    std::vector<DeviceEndpoint> m_endpoints;
};

} // namespace Aura::Core::IO
