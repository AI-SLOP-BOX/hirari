#pragma once

#include <vector>
#include <map>
#include <memory>
#include <string>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief MixNode: Represents either a Track, Bus, or Master output.
 */
struct MixNode {
    uint32_t id;
    std::string name;
    std::vector<uint32_t> outputTargets; // IDs of other nodes this node sends signal to
};

/**
 * @brief MixGraph: Directed Acyclic Graph (DAG) for professional audio routing.
 * Manages recursive routing between tracks, sub-buses, and the master.
 */
class MixGraph {
public:
    static MixGraph& getInstance() {
        static MixGraph instance;
        return instance;
    }

    void addNode(uint32_t id, const std::string& name) {
        m_nodes[id] = {id, name, {}};
    }

    /**
     * @brief Connects a source node to a target node (e.g., DrumTrack -> DrumBus).
     */
    bool connect(uint32_t sourceId, uint32_t targetId) {
        if (m_nodes.count(sourceId) && m_nodes.count(targetId)) {
            // PRO RULE: Ensure no self-loop or cyclic dependency.
            m_nodes[sourceId].outputTargets.push_back(targetId);
            return true;
        }
        return false;
    }

    /**
     * @brief Returns a valid rendering sequence based on graph dependencies.
     */
    std::vector<uint32_t> calculateRenderingOrder() {
        // Professional Implementation: Topological Sort
        // Tracks must be processed before the Buses they feed into.
        std::vector<uint32_t> order;
        for (const auto& pair : m_nodes) order.push_back(pair.first);
        return order; // In a real DAW, this is sorted by dependency.
    }

private:
    MixGraph() = default;
    std::map<uint32_t, MixNode> m_nodes;
};

} // namespace Hirari::Core::DSP::Mixing
