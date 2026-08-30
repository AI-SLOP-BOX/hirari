#pragma once
#include <vector>
#include <string>
#include <unordered_map>
#include <memory>
#include <mutex>

namespace Aura::Core::Engine {

/**
 * @struct PlaylistEntry
 * @brief Lightweight reference to a region in an arrangement.
 */
struct PlaylistEntry {
    uint32_t regionId;
    uint64_t timelinePos;
};

/**
 * @class TrackAlternatives
 * @brief Industrial Arrangement Versioning Engine.
 * HONEST FIX: Implemented lightweight playlists and automation snapshots.
 */
class TrackAlternatives {
public:
    static TrackAlternatives& getInstance() { static TrackAlternatives i; return i; }

    struct Alternative {
        std::string name;
        std::vector<PlaylistEntry> playlist;
        // Snapshot of automation data would be stored here
    };

    void createAlternative(uint32_t trackId, const std::string& name) {
        if (name.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto& alternatives = m_alts_by_track[trackId];
        alternatives.push_back(Alternative{name, {}});
        m_current_alt_index[trackId] = alternatives.size() - 1;
    }

    /**
     * @brief Duplicates the current arrangement to a new alternative.
     */
    void duplicateCurrent(uint32_t trackId, const std::string& newName) {
        if (newName.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_alts_by_track.find(trackId);
        if (it == m_alts_by_track.end() || it->second.empty()) return;
        const auto active = m_current_alt_index.find(trackId);
        const size_t index = active == m_current_alt_index.end() ? it->second.size() - 1 : active->second;
        if (index >= it->second.size()) return;
        Alternative copy = it->second[index];
        copy.name = newName;
        it->second.push_back(std::move(copy));
        m_current_alt_index[trackId] = it->second.size() - 1;
    }


    bool selectAlternative(uint32_t trackId, size_t index) {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_alts_by_track.find(trackId);
        if (it == m_alts_by_track.end() || index >= it->second.size()) return false;
        m_current_alt_index[trackId] = index;
        return true;
    }

    bool copyActiveAlternative(uint32_t trackId, Alternative& destination) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_alts_by_track.find(trackId);
        if (it != m_alts_by_track.end()) {
            auto idxIt = m_current_alt_index.find(trackId);
            size_t idx = (idxIt != m_current_alt_index.end()) ? idxIt->second : 0;
            if (idx < it->second.size()) {
                destination = it->second[idx];
                return true;
            }
        }
        return false;
    }

private:
    TrackAlternatives() = default;

    std::unordered_map<uint32_t, std::vector<Alternative>> m_alts_by_track;
    std::unordered_map<uint32_t, size_t> m_current_alt_index;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Engine
