#pragma once
#include <cstdint>
#include <string>
#include <atomic>
#include <thread>
#include <chrono>
#include <unordered_map>
#include <mutex>

namespace Aura::Core::Plugins {

/**
 * @class PluginSandboxKernel
 * @brief Autonomous monitor for isolated plugin processes with node-specific watchdogs.
 */
class PluginSandboxKernel {
public:
    static PluginSandboxKernel& getInstance() {
        static PluginSandboxKernel instance;
        return instance;
    }

    struct NodeHealth {
        std::atomic<uint64_t> lastHeartbeat{0};
        std::atomic<bool> isStalled{false};
        std::chrono::steady_clock::time_point lastCheck;
    };

    /**
     * @brief Registers a new node for health monitoring.
     */
    void registerNode(const std::string& nodeId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_nodes[nodeId] = std::make_unique<NodeHealth>();
    }

    /**
     * @brief Performs load-adaptive health checks.
     * INDUSTRIAL: Stalling threshold scales with DSP pressure.
     */
    bool checkHealth(const std::string& nodeId, uint64_t currentHeartbeat, float engineLoad) {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_nodes.find(nodeId);
        if (it == m_nodes.end()) return false;

        auto& health = *it->second;
        uint64_t last = health.lastHeartbeat.load(std::memory_order_relaxed);
        
        // --- PHASE 48: LOAD-ADAPTIVE STALLING DETECTION ---
        // Threshold: 2ms base + 10ms scaling based on load
        float thresholdMs = 2.0f + (engineLoad * 10.0f);
        
        if (currentHeartbeat == last && currentHeartbeat != 0) {
            auto now = std::chrono::steady_clock::now();
            auto diff = std::chrono::duration_cast<std::chrono::milliseconds>(now - health.lastCheck).count();
            if (diff > thresholdMs) {
                health.isStalled.store(true, std::memory_order_release);
                return false; // STALL DETECTED
            }
        } else {
            health.lastHeartbeat.store(currentHeartbeat, std::memory_order_relaxed);
            health.lastCheck = std::chrono::steady_clock::now();
            health.isStalled.store(false, std::memory_order_release);
        }

        return true;
    }

private:
    PluginSandboxKernel() = default;
    std::unordered_map<std::string, std::unique_ptr<NodeHealth>> m_nodes;
    std::mutex m_mutex;
};

} // namespace Aura::Core::Plugins
