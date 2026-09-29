#pragma once
#include <vector>
#include <unordered_map>
#include <memory>
#include <algorithm>
#include <atomic>
#include <functional>
#include "../rust_ffi.hpp"

#include "pdc_graph.hpp"

namespace Hirari::Core::Engine {

// AudioNode represents a track, bus, or plugin processing unit
class AudioNode {
public:
    uint32_t id;
    uint32_t processingLatency = 0; // Pure latency of this node
    uint32_t cumulativeDelay = 0;   // Audio latency compensation offset
    std::vector<uint32_t> outgoingEdges; // Routing targets
    uint32_t inDegree = 0; // Inputs count for topological sorting
    std::atomic<int> dynamicInputsReady{0}; // Atomic dependency counter for lock-free scheduler
};

/**
 * @class RoutingGraphPDC
 * @brief DAG Routing & PipeWire/JACK-style Lock-Free Stage Scheduling Engine.
 */
class RoutingGraphPDC {
public:
    struct ProcessStage {
        std::vector<uint32_t> nodeIds;
    };

    void addNode(uint32_t nodeId, uint32_t latency) {
        auto node = std::make_shared<AudioNode>();
        node->id = nodeId;
        node->processingLatency = latency;
        m_nodes[nodeId] = node;
    }

    void connect(uint32_t sourceId, uint32_t destId) {
        if (m_nodes.count(sourceId) && m_nodes.count(destId)) {
            auto& edges = m_nodes[sourceId]->outgoingEdges;
            if (std::find(edges.begin(), edges.end(), destId) == edges.end()) {
                edges.push_back(destId);
                m_nodes[destId]->inDegree++;
            }
        }
    }

    /**
     * @brief Computes topological ordering, latency compensation, and PipeWire-style stages.
     */
    bool compileGraph() {
        m_executionOrder.clear();
        m_stages.clear();

        std::vector<uint32_t> nodeIds;
        std::vector<uint32_t> nodeLatencies;
        std::vector<uint32_t> edgeSources;
        std::vector<uint32_t> edgeDestinations;
        for (const auto& pair : m_nodes) {
            nodeIds.push_back(pair.first);
            nodeLatencies.push_back(pair.second->processingLatency);
            for (uint32_t destination : pair.second->outgoingEdges) {
                edgeSources.push_back(pair.first);
                edgeDestinations.push_back(destination);
            }
        }
        std::vector<uint32_t> executionOrder(nodeIds.size());
        std::vector<uint32_t> stageDepths(nodeIds.size());
        if (!hirari_compile_staged_routing_graph(
                nodeIds.empty() ? nullptr : nodeIds.data(), nodeIds.size(),
                edgeSources.empty() ? nullptr : edgeSources.data(),
                edgeDestinations.empty() ? nullptr : edgeDestinations.data(),
                edgeSources.size(),
                executionOrder.empty() ? nullptr : executionOrder.data(), executionOrder.size(),
                stageDepths.empty() ? nullptr : stageDepths.data(), stageDepths.size())) {
            return false;
        }
        m_executionOrder = std::move(executionOrder);

        // Compile PipeWire-style stages from Rust's longest-path depths.
        uint32_t maxDepth = 0;
        for (uint32_t depth : stageDepths) maxDepth = std::max(maxDepth, depth);
        m_stages.resize(nodeIds.empty() ? 1 : static_cast<size_t>(maxDepth) + 1);
        for (size_t index = 0; index < nodeIds.size(); ++index) {
            m_stages[stageDepths[index]].nodeIds.push_back(nodeIds[index]);
        }
        for (auto& stage : m_stages) {
            std::sort(stage.nodeIds.begin(), stage.nodeIds.end());
        }
        if (nodeIds.empty()) return true;

        std::vector<uint32_t> nodeCompensations(nodeIds.size());
        [[maybe_unused]] uint32_t globalLatency = 0;
        if (!hirari_pdc_solve_engine_graph(
                nodeIds.empty() ? nullptr : nodeIds.data(),
                nodeLatencies.empty() ? nullptr : nodeLatencies.data(), nodeIds.size(),
                edgeSources.empty() ? nullptr : edgeSources.data(),
                edgeDestinations.empty() ? nullptr : edgeDestinations.data(), edgeSources.size(),
                nullptr, 0, nullptr,
                nodeCompensations.empty() ? nullptr : nodeCompensations.data(),
                nodeCompensations.size(), &globalLatency)) {
            return false;
        }
        for (size_t index = 0; index < nodeIds.size(); ++index) {
            m_nodes.at(nodeIds[index])->cumulativeDelay = nodeCompensations[index];
        }

        return true;
    }

    /**
   * @brief Deterministic bounded stage scheduler for the audio graph.
   * Nodes within a stage have no mutual dependencies; the fallback executes them
   * in stable ID order so the audio callback never creates worker threads.
     */
    void executeStages(const std::function<void(uint32_t)>& processFunc) {
        for (const auto& stage : m_stages) {
            if (stage.nodeIds.empty()) continue;

            if (stage.nodeIds.size() == 1) {
                // Optimize single-node stage by executing synchronously on calling thread
                processFunc(stage.nodeIds[0]);
            } else {
                for (uint32_t nodeId : stage.nodeIds) {
                    processFunc(nodeId);
                }
            }
        }
    }

    const std::vector<uint32_t>& getExecutionOrder() const { return m_executionOrder; }
    const std::vector<ProcessStage>& getStages() const { return m_stages; }
    uint32_t getDelayForNode(uint32_t id) const { return m_nodes.at(id)->cumulativeDelay; }

private:
    std::unordered_map<uint32_t, std::shared_ptr<AudioNode>> m_nodes;
    std::vector<uint32_t> m_executionOrder;
    std::vector<ProcessStage> m_stages;
};

} // namespace Hirari::Core::Engine
