#pragma once
#include <vector>
#include <string>
#include <mutex>

namespace Aura::Core::Network {

/**
 * @struct ComputeNode
 * @brief Represents a remote DSP resource.
 */
struct ComputeNode {
    std::string id;
    std::string ip;
    float cpuLoad;
    uint32_t availableCores;
};

/**
 * @class DSPFarmManager
 * @brief Orchestrates distributed computation resources.
 */
class DSPFarmManager {
public:
    static DSPFarmManager& getInstance() {
        static DSPFarmManager instance;
        return instance;
    }

    /**
     * @brief Discovers and registers compute nodes.
     */
    void discoverNodes() {
        std::lock_guard<std::mutex> lock(m_mutex);
        // INDUSTRIAL: Use mDNS/ZeroConf to discover nodes 
        // on the local network automatically.
    }

    const std::vector<ComputeNode>& getAvailableNodes() const {
        return m_nodes;
    }

private:
    DSPFarmManager() = default;
    mutable std::mutex m_mutex;
    std::vector<ComputeNode> m_nodes;
};

} // namespace Aura::Core::Network
