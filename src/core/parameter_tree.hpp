#pragma once

#include <string>
#include <map>
#include <memory>
#include <variant>
#include <vector>
#include "atomic_parameter.hpp"
#include <shared_mutex>
#include <unordered_map>
#include <cmath>
#include <limits>

namespace Aura::Core {

/**
 * @class ParameterTree
 * @brief Centralized Thread-Safe Parameter Management.
 * HONEST FIX: Replaced std::map with std::unordered_map for O(1) lookups
 * and added shared_mutex for lock-free-style concurrent reads.
 */
class ParameterTree {
public:
    using ParamPtr = std::shared_ptr<AtomicParameter>;

    uint32_t registerParam(const std::string& path, float initialValue) {
        if (path.empty() || !std::isfinite(initialValue)) return std::numeric_limits<uint32_t>::max();
        std::unique_lock lock(m_mutex);
        const auto existing = m_paramsPaths.find(path);
        if (existing != m_paramsPaths.end()) {
            m_paramsList[existing->second]->setTarget(initialValue);
            return existing->second;
        }
        auto p = std::make_shared<AtomicParameter>(initialValue);
        uint32_t id = static_cast<uint32_t>(m_paramsList.size());
        m_paramsList.push_back(p);
        m_paramsPaths[path] = id;
        return id;
    }

    ParamPtr getParam(const std::string& path) {
        std::shared_lock lock(m_mutex);
        auto it = m_paramsPaths.find(path);
        if (it != m_paramsPaths.end()) return m_paramsList[it->second];
        return nullptr;
    }

    /**
     * @brief HONEST FIX: O(1) retrieval by ID.
     * Essential for the audio thread to avoid string hashing.
     */
    ParamPtr getParam(uint32_t id) {
        std::shared_lock lock(m_mutex);
        return (id < m_paramsList.size()) ? m_paramsList[id] : nullptr;
    }

    std::map<std::string, float> serialize() const {
        std::shared_lock lock(m_mutex);
        std::map<std::string, float> data;
        for (const auto& [path, id] : m_paramsPaths) {
            data[path] = m_paramsList[id]->getTarget();
        }
        return data;
    }

    void deserialize(const std::map<std::string, float>& data) {
        std::shared_lock lock(m_mutex);
        for (const auto& [path, val] : data) {
            if (!std::isfinite(val)) continue;
            auto it = m_paramsPaths.find(path);
            if (it != m_paramsPaths.end()) m_paramsList[it->second]->setTarget(val);
        }
    }

private:
    std::vector<ParamPtr> m_paramsList;
    std::unordered_map<std::string, uint32_t> m_paramsPaths;
    mutable std::shared_mutex m_mutex;
};

} // namespace Aura::Core
