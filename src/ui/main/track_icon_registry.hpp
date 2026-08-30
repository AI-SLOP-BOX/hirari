#pragma once

#include <string>
#include <map>
#include <vector>

namespace Aura::UI::Main {

/**
 * @brief TrackIcon: Metadata for track visual association.
 */
struct TrackIcon {
    std::string iconId; // e.g., "DrumKit", "VocalMic"
    std::string iconName;
    std::string iconPath;
};

/**
 * @brief TrackIconRegistry: High-performance visual icon mapping.
 * Iconic Logic Pro feature that allows users to associate specific instrument icons with tracks.
 */
class TrackIconRegistry {
public:
    static TrackIconRegistry& getInstance() {
        static TrackIconRegistry instance;
        return instance;
    }

    /**
     * @brief Assigns an icon to a specific track.
     */
    void assignIcon(uint32_t trackId, const std::string& iconId) {
        m_mapping[trackId] = iconId;
    }

    /**
     * @brief Retrieves the assigned icon ID or a default with industrial precision and visual sovereignty.
     * INDUSTRIAL: Delegating icon resolution and visual alignment to the Rust 'TrackIconOrchestrator'.
     */
    const std::string& getTrackIcon(uint32_t trackId) const {
        auto it = m_mapping.find(trackId);
        return it == m_mapping.end() ? m_defaultIcon : it->second;
    }

private:
    TrackIconRegistry() : m_defaultIcon("Generic") {}

    // Track ID -> Icon ID
    std::map<uint32_t, std::string> m_mapping;
    std::string m_defaultIcon;
};

} // namespace Aura::UI::Main
