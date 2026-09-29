#pragma once

#include <cstdint>
#include <map>
#include <utility>
#include <vector>

#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * Compatibility adapter for the Rust DAG latency solver. Graph storage stays
 * in the existing C++ callers; topology, path latency, and compensation are
 * calculated by `hirari_pdc_solve_engine_graph`.
 */
class PDCGraphSolver {
public:
    using Edge = std::pair<uint32_t, uint32_t>;

    struct Node {
        uint32_t id;
        uint32_t ownLatency;
        uint32_t totalLatency;
        uint32_t compensation = 0;
        bool isDirty = true;
        std::vector<uint32_t> downstream;
    };

    void solve(std::map<uint32_t, Node>& nodes) {
        m_globalProjectLatency = 0;
        m_hasCycle = false;
        m_edgeCompensations.clear();
        if (nodes.empty()) return;

        std::vector<uint32_t> nodeIds;
        std::vector<uint32_t> nodeLatencies;
        std::vector<uint32_t> edgeSources;
        std::vector<uint32_t> edgeDestinations;
        std::vector<uint32_t> nodeOutputLatencies(nodes.size());
        std::vector<uint32_t> nodeCompensations(nodes.size());
        for (const auto& [id, node] : nodes) {
            nodeIds.push_back(id);
            nodeLatencies.push_back(node.ownLatency);
            for (uint32_t destination : node.downstream) {
                edgeSources.push_back(id);
                edgeDestinations.push_back(destination);
            }
        }
        std::vector<uint32_t> edgeCompensations(edgeSources.size());
        if (!hirari_pdc_solve_engine_graph(
                nodeIds.data(), nodeLatencies.data(), nodeIds.size(),
                edgeSources.empty() ? nullptr : edgeSources.data(),
                edgeDestinations.empty() ? nullptr : edgeDestinations.data(),
                edgeSources.size(),
                edgeCompensations.empty() ? nullptr : edgeCompensations.data(),
                edgeCompensations.size(), nodeOutputLatencies.data(),
                nodeCompensations.data(), nodes.size(), &m_globalProjectLatency)) {
            m_hasCycle = true;
            for (auto& [id, node] : nodes) {
                (void)id;
                node.compensation = 0;
                node.isDirty = true;
            }
            m_globalProjectLatency = 0;
            return;
        }

        size_t nodeIndex = 0;
        for (auto& [id, node] : nodes) {
            node.totalLatency = nodeOutputLatencies[nodeIndex];
            node.compensation = nodeCompensations[nodeIndex];
            node.isDirty = false;
            ++nodeIndex;
        }
        for (size_t edge = 0; edge < edgeSources.size(); ++edge) {
            if (nodes.contains(edgeDestinations[edge])) {
                m_edgeCompensations[{edgeSources[edge], edgeDestinations[edge]}] =
                    edgeCompensations[edge];
            }
        }
    }

    uint32_t globalProjectLatency() const noexcept { return m_globalProjectLatency; }
    bool hasCycle() const noexcept { return m_hasCycle; }
    uint32_t edgeCompensation(uint32_t source, uint32_t destination) const noexcept {
        const auto it = m_edgeCompensations.find({source, destination});
        return it == m_edgeCompensations.end() ? 0 : it->second;
    }

private:
    uint32_t m_globalProjectLatency = 0;
    bool m_hasCycle = false;
    std::map<Edge, uint32_t> m_edgeCompensations;
};

} // namespace Hirari::Core::Engine
