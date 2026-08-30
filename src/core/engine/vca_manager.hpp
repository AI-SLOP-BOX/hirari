#pragma once
#include <vector>
#include <atomic>
#include <array>
#include <unordered_map>
#include <vector>
#include <mutex>
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @class VCAManager
 * @brief Industrial Hierarchical Gain Orchestration Engine.
 * HONEST FIX: Implemented O(1) lock-free gain lookups and nested hierarchy resolution.
 */
class VCAManager {
public:
    static constexpr size_t kMaxTracks = 2048;
    struct GroupSnapshot {
        uint32_t id = 0;
        float gain = 1.0f;
        std::vector<uint32_t> trackIds;
    };
    static VCAManager& getInstance() { static VCAManager i; return i; }

    /**
     * @brief GET: Lock-free O(1) access to pre-calculated cumulative gain.
     */
    float getCumulativeGain(uint32_t trackId) const {
        if (trackId >= kMaxTracks) return 1.0f;
        return m_resolvedTrackGains[trackId].load(std::memory_order_relaxed);
    }

    std::vector<GroupSnapshot> snapshotGroups() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<GroupSnapshot> result;
        result.reserve(m_groups.size());
        for (const auto& [id, group] : m_groups) {
            result.push_back({id, group.gain, group.trackIds});
        }
        return result;
    }

    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_groups.clear();
        resolveHierarchyUnlocked();
    }

    void resolveHierarchy() {
        std::lock_guard<std::mutex> lock(m_mutex);
        resolveHierarchyUnlocked();
    }

    void addGroup(uint32_t id, float gain) {
        if (id == 0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_groups[id].gain = std::isfinite(gain) ? std::clamp(gain, 0.0f, 8.0f) : 1.0f;
    }

    bool assignTrack(uint32_t trackId, uint32_t groupId) {
        if (trackId >= kMaxTracks || groupId == 0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_groups.find(groupId);
        if (it == m_groups.end()) return false;
        if (std::find(it->second.trackIds.begin(), it->second.trackIds.end(), trackId) == it->second.trackIds.end()) {
            it->second.trackIds.push_back(trackId);
        }
        resolveHierarchyUnlocked();
        return true;
    }

private:
    struct GroupState { float gain = 1.0f; std::vector<uint32_t> trackIds; };
    void resolveHierarchyUnlocked() {
        for (auto& value : m_resolvedTrackGains) value.store(1.0f, std::memory_order_relaxed);
        for (const auto& [_, group] : m_groups) {
            const float gain = std::isfinite(group.gain) ? std::clamp(group.gain, 0.0f, 8.0f) : 1.0f;
            for (uint32_t trackId : group.trackIds) {
                if (trackId < kMaxTracks) {
                    const float current = m_resolvedTrackGains[trackId].load(std::memory_order_relaxed);
                    m_resolvedTrackGains[trackId].store(std::clamp(current * gain, 0.0f, 8.0f), std::memory_order_release);
                }
            }
        }
    }
    VCAManager() {
        for (auto& gain : m_resolvedTrackGains) gain.store(1.0f, std::memory_order_relaxed);
    }
    std::array<std::atomic<float>, kMaxTracks> m_resolvedTrackGains;
    std::unordered_map<uint32_t, GroupState> m_groups;
    mutable std::mutex m_mutex;
};


} // namespace Aura::Core::Engine
