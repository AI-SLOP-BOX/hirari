#pragma once
#include <vector>
#include <algorithm>
#include <set>
#include <unordered_map>
#include <map>
#include <cstdint>

namespace Aura::Core::Engine {

/**
 * @class BusRouter
 * @brief Industrial Signal Routing Orchestrator.
 * HONEST FIX: Implemented cycle detection and parallel execution levels.
 */
class BusRouter {
public:
    void addDependency(uint32_t fromId, uint32_t toId) {
        if (fromId >= m_adj.size()) m_adj.resize(fromId + 1);
        m_adj[fromId].push_back(toId);
        m_allNodes.insert(fromId);
        m_allNodes.insert(toId);
        m_isDirty = true;
    }

    struct Level { std::vector<uint32_t> nodes; };

    const std::vector<Level>& getParallelLevels() const {
        if (m_isDirty) {
            rebuildGraph();
            m_isDirty = false;
        }
        return m_cachedLevels;
    }

private:
    void rebuildGraph() const {
        m_cachedLevels.clear();
        std::map<uint32_t, uint32_t> indegree;
        for (uint32_t node : m_allNodes) indegree[node] = 0;
        for (uint32_t from = 0; from < m_adj.size(); ++from) {
            for (uint32_t to : m_adj[from]) if (indegree.count(to)) ++indegree[to];
        }
        std::set<uint32_t> ready;
        for (const auto& [node, degree] : indegree) if (degree == 0) ready.insert(node);
        size_t visited = 0;
        while (!ready.empty()) {
            Level level;
            const auto current = ready;
            ready.clear();
            for (uint32_t node : current) {
                level.nodes.push_back(node); ++visited;
                if (node >= m_adj.size()) continue;
                for (uint32_t target : m_adj[node]) {
                    if (--indegree[target] == 0) ready.insert(target);
                }
            }
            m_cachedLevels.push_back(std::move(level));
        }
        if (visited != m_allNodes.size()) m_hasCycle = true;
        else m_hasCycle = false;
    }

private:
    std::vector<std::vector<uint32_t>> m_adj;
    std::set<uint32_t> m_allNodes;
    mutable std::vector<Level> m_cachedLevels;
    mutable bool m_isDirty = true;
    mutable bool m_hasCycle = false;
public:
    bool hasCycle() const { getParallelLevels(); return m_hasCycle; }
    std::vector<std::pair<uint32_t, uint32_t>> dependencies() const {
        std::vector<std::pair<uint32_t, uint32_t>> result;
        for (uint32_t from = 0; from < m_adj.size(); ++from)
            for (const uint32_t to : m_adj[from]) result.emplace_back(from, to);
        return result;
    }
};


} // namespace Aura::Core::Engine
