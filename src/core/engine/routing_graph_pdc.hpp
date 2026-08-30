#pragma once
#include <vector>
#include <unordered_map>
#include <memory>
#include <queue>
#include <algorithm>
#include <atomic>
#include <functional>
#include <limits>
#include <map>

#include "pdc_graph.hpp"

namespace Aura::Core::Engine {

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

        std::unordered_map<uint32_t, uint32_t> inDegrees;
        std::unordered_map<uint32_t, uint32_t> nodeDepths;
        std::queue<uint32_t> zeroInDegreeQueue;

        for (const auto& pair : m_nodes) {
            inDegrees[pair.first] = pair.second->inDegree;
            nodeDepths[pair.first] = 0;
            if (pair.second->inDegree == 0) {
                zeroInDegreeQueue.push(pair.first);
            }
        }

        // 1. Kahn's Topological Sort with Stage Level Calculations
        while (!zeroInDegreeQueue.empty()) {
            uint32_t curr = zeroInDegreeQueue.front();
            zeroInDegreeQueue.pop();
            m_executionOrder.push_back(curr);

            uint32_t currDepth = nodeDepths[curr];
            const auto& edges = m_nodes[curr]->outgoingEdges;
            for (uint32_t dest : edges) {
                nodeDepths[dest] = std::max(nodeDepths[dest], currDepth + 1);
                inDegrees[dest]--;
                if (inDegrees[dest] == 0) {
                    zeroInDegreeQueue.push(dest);
                }
            }
        }

        // Cycle check
        if (m_executionOrder.size() != m_nodes.size()) {
            return false;
        }

        // 2. Compile PipeWire-style Stages
        uint32_t maxDepth = 0;
        for (const auto& pair : nodeDepths) {
            maxDepth = std::max(maxDepth, pair.second);
        }

        m_stages.resize(maxDepth + 1);
        for (const auto& pair : nodeDepths) {
            m_stages[pair.second].nodeIds.push_back(pair.first);
        }
        for (auto& stage : m_stages) {
            std::sort(stage.nodeIds.begin(), stage.nodeIds.end());
        }

        // 3. Use the shared solver for the production PDC calculation.  Keeping
        // this at the graph boundary prevents the UI-facing PDC implementation
        // and the realtime routing graph from drifting apart.
        std::map<uint32_t, PDCGraphSolver::Node> solverNodes;
        for (const auto& [id, node] : m_nodes) {
            solverNodes.emplace(id, PDCGraphSolver::Node{
                id,
                node->processingLatency,
                0,
                0,
                true,
                node->outgoingEdges,
            });
        }

        PDCGraphSolver solver;
        solver.solve(solverNodes);
        if (solver.hasCycle()) return false;

        // 4. Publish the compensation offsets calculated by the shared solver.
        for (const auto& [id, node] : m_nodes) {
            node->cumulativeDelay = solverNodes.at(id).compensation;
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

} // namespace Aura::Core::Engine
