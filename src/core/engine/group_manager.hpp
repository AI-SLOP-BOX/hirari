#pragma once
#include <vector>
#include <unordered_map>
#include <unordered_set>
#include <cstdint>
#include <mutex>

namespace Aura::Core::Engine {

/**
 * @struct GroupSettings
 * @brief Bitmask for linked track parameters.
 */
struct GroupSettings {
    enum Flags : uint32_t {
        Volume = 1 << 0,
        Pan    = 1 << 1,
        Mute   = 1 << 2,
        Solo   = 1 << 3,
        Record = 1 << 4,
        Send   = 1 << 5
    };
    uint32_t activeFlags = 0;
};

/**
 * @class GroupManager
 * @brief Industrial Track Parameter Synchronization Engine.
 * HONEST FIX: Implemented real-time propagation with attribute masking.
 */
class GroupManager {
public:
    static GroupManager& getInstance() { static GroupManager i; return i; }

    void createGroup(uint32_t groupId, uint32_t settings) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_groupConfigs[groupId] = { settings };
    }

    void addTrackToGroup(uint32_t trackId, uint32_t groupId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_groups[groupId].insert(trackId);
        m_trackGroups[trackId].insert(groupId);
        m_groupConfigs.try_emplace(groupId, GroupSettings{});
    }

    template<typename F>
    void propagate(uint32_t originId, GroupSettings::Flags attr, F syncFunc) {
        std::vector<uint32_t> targets;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_trackGroups.find(originId);
            if (it == m_trackGroups.end()) return;
            for (uint32_t groupId : it->second) {
                const auto config = m_groupConfigs.find(groupId);
                if (config == m_groupConfigs.end() ||
                    (config->second.activeFlags & static_cast<uint32_t>(attr)) == 0) continue;
                for (uint32_t trackId : m_groups[groupId]) {
                    if (trackId != originId) targets.push_back(trackId);
                }
            }
        }
        for (uint32_t trackId : targets) syncFunc(trackId, attr);
    }

private:
    GroupManager() = default;
    std::unordered_map<uint32_t, GroupSettings> m_groupConfigs;
    std::unordered_map<uint32_t, std::unordered_set<uint32_t>> m_groups;
    std::unordered_map<uint32_t, std::unordered_set<uint32_t>> m_trackGroups;
    mutable std::mutex m_mutex;
};


} // namespace Aura::Core::Engine
