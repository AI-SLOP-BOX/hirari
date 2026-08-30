#pragma once

#include <vector>
#include <map>
#include <string>
#include <memory>
#include <mutex>
#include <atomic>
#include "track.hpp"
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @brief VCAGroup: A collection of tracks controlled by a single master fader.
 */
struct VCAGroup {
    uint32_t id;
    std::string name;
    float masterGain = 1.0f;
    std::vector<uint32_t> trackIds;
};

/**
 * @brief VCAControlSystem: Professional Large-Scale Console Workflow.
 * Standard for mixing projects with 100+ tracks (SSL/Neve console style).
 */
class VCAControlSystem {
public:
    static VCAControlSystem& getInstance() { static VCAControlSystem i; return i; }

    /// Control-thread snapshot used by project persistence.  Returning a copy
    /// keeps serializers from holding references into the live registry.
    std::vector<VCAGroup> snapshotGroups() const {
        const auto snapshot = std::atomic_load_explicit(&m_publishedGroups, std::memory_order_acquire);
        return snapshot ? *snapshot : std::vector<VCAGroup>{};
    }

    /**
     * @brief CREATE GROUP: Designates a master VCA fader for a set of tracks.
     */
    void createGroup(const std::string& name, const std::vector<uint32_t>& ids) {
        std::lock_guard<std::mutex> lock(m_controlMutex);
        uint32_t newId = static_cast<uint32_t>(m_groups.size());
        m_groups[newId] = { newId, name, 1.0f, ids };
        publishSnapshotLocked();
    }

    /**
     * @brief SET GAIN: Cascades gain reduction to all slave tracks.
     * Unlike Audio Busses, VCA affects the track faders directly (Pre-Post sends).
     */
    void setGroupGain(uint32_t groupId, float gain) {
        std::lock_guard<std::mutex> lock(m_controlMutex);
        auto it = m_groups.find(groupId);
        if (it == m_groups.end()) return;
        it->second.masterGain = std::isfinite(gain) ? std::clamp(gain, 0.0f, 8.0f) : 1.0f;
        publishSnapshotLocked();
    }

    float resolveTrackGain(uint32_t trackId, float baseGain) {
        float resolved = std::isfinite(baseGain) ? baseGain : 1.0f;
        const auto snapshot = std::atomic_load_explicit(&m_publishedGroups, std::memory_order_acquire);
        if (!snapshot) return std::clamp(resolved, 0.0f, 8.0f);
        for (const auto& group : *snapshot) {
            if (std::find(group.trackIds.begin(), group.trackIds.end(), trackId) != group.trackIds.end()) {
                resolved *= group.masterGain;
            }
        }
        return std::isfinite(resolved) ? std::clamp(resolved, 0.0f, 8.0f) : 1.0f;
    }

private:
    VCAControlSystem() = default;
    std::map<uint32_t, VCAGroup> m_groups;
    mutable std::mutex m_controlMutex;
    std::shared_ptr<const std::vector<VCAGroup>> m_publishedGroups =
        std::make_shared<const std::vector<VCAGroup>>();

    void publishSnapshotLocked() {
        auto next = std::make_shared<std::vector<VCAGroup>>();
        next->reserve(m_groups.size());
        for (const auto& [_, group] : m_groups) next->push_back(group);
        std::atomic_store_explicit(
            &m_publishedGroups,
            std::shared_ptr<const std::vector<VCAGroup>>(std::move(next)),
            std::memory_order_release);
    }
};


} // namespace Aura::Core::Engine
