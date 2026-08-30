#pragma once
#include <vector>
#include <map>
#include <set>
#include <memory>
#include <algorithm>
#include <unordered_map>
#include <atomic>
#include "track.hpp"

namespace Aura::Core::Engine {

/**
 * @class ProcessNode
 * @brief Represents a processable entity in the graph (Track or Bus).
 */
struct ProcessNode {
    enum Type { TrackNode, BusNode };
    Type type;
    uint32_t id;
    void* ptr; // Pointer to Track or Bus
    std::vector<uint32_t> dependencies; // IDs of nodes this node depends on
};

/**
 * @class ExecutionStage
 * @brief block of nodes that can be processed in parallel.
 */
struct ExecutionStage {
    std::vector<ProcessNode> nodes;
};

/**
 * @class ProcessGraph
 * @brief THE NERVE CENTER: Converts routing topography into parallel stages.
 */
class ProcessGraph {
public:
    /**
     * @brief COMPILE: Builds the execution stages from the raw node list.
     */
    void compile(const std::vector<ProcessNode>& nodes) {
        m_stages.clear();
        if (nodes.empty()) {
            m_version.fetch_add(1, std::memory_order_release);
            return;
        }
        std::unordered_map<uint32_t, size_t> indexById;
        std::vector<uint32_t> indegree(nodes.size(), 0);
        std::vector<std::vector<size_t>> dependents(nodes.size());
        for (size_t i = 0; i < nodes.size(); ++i) indexById[nodes[i].id] = i;
        for (size_t i = 0; i < nodes.size(); ++i) {
            for (uint32_t dependency : nodes[i].dependencies) {
                auto it = indexById.find(dependency);
                if (it == indexById.end() || it->second == i) continue;
                ++indegree[i];
                dependents[it->second].push_back(i);
            }
        }
        std::vector<size_t> ready;
        for (size_t i = 0; i < nodes.size(); ++i) if (indegree[i] == 0) ready.push_back(i);
        size_t visited = 0;
        while (!ready.empty()) {
            ExecutionStage stage;
            std::vector<size_t> next;
            for (size_t index : ready) {
                stage.nodes.push_back(nodes[index]);
                ++visited;
                for (size_t dependent : dependents[index]) {
                    if (--indegree[dependent] == 0) next.push_back(dependent);
                }
            }
            m_stages.push_back(std::move(stage));
            ready = std::move(next);
        }
        // A cycle must not silently drop nodes. Keep the remaining nodes in a
        // final serial stage; routing diagnostics can report the cycle later.
        if (visited != nodes.size()) {
            ExecutionStage cycleStage;
            for (size_t i = 0; i < nodes.size(); ++i) if (indegree[i] != 0) cycleStage.nodes.push_back(nodes[i]);
            if (!cycleStage.nodes.empty()) m_stages.push_back(std::move(cycleStage));
        }
        m_version.fetch_add(1, std::memory_order_release);
    }


    const std::vector<ExecutionStage>& getStages() const { return m_stages; }
    uint64_t getVersion() const { return m_version.load(std::memory_order_acquire); }

private:
    std::vector<ExecutionStage> m_stages;
    std::atomic<uint64_t> m_version{0};
};

} // namespace Aura::Core::Engine
