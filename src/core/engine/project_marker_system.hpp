#pragma once
#include <string>
#include <vector>
#include <map>
#include <memory>
#include <mutex>
#include <algorithm>
#include <cctype>
#include "../composition/harmonic_context_tracker.hpp"

namespace Aura::Core::Engine {

/**
 * @struct SectionMetadata
 * @brief Industrial Structural Sovereignty.
 */
struct SectionMetadata {
    std::string type; // Verse, Chorus, Bridge
    float tensionTarget;
    float valenceTarget;
};

/**
 * @struct Marker
 * @brief Industrial Structural Node for Aura Studio Pro.
 */
struct Marker {
    uint64_t samplePosition;
    std::string name;
    uint32_t colorHex;
    SectionMetadata section; // --- PHASE 71: SECTION SOVEREIGNTY ---
};

/**
 * @class ProjectMarkerSystem
 * @brief Industrial Storyboarding Engine for Aura Studio Pro.
 * Implements autonomous boundary detection and structural re-flow.
 */
class ProjectMarkerSystem {
public:
    static ProjectMarkerSystem& getInstance() {
        static ProjectMarkerSystem instance;
        return instance;
    }

    /**
     * @brief Performs AUTOMATED ARRANGEMENT ANALYSIS.
     * INDUSTRIAL: Delegating structure detection to the Rust 'ArrangementOrchestrator'.
     */
    void updateNarrativeStructure() {
        std::lock_guard<std::mutex> lock(m_mutex);
        // Keep structure inference deterministic and local: marker names are
        // user-authored input, so this is intentionally a lightweight naming
        // convention rather than an opaque AI decision.
        for (auto& [position, marker] : m_markers) {
            (void)position;
            std::string normalized = marker.name;
            std::transform(normalized.begin(), normalized.end(), normalized.begin(),
                [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
            if (normalized.find("chorus") != std::string::npos ||
                normalized.find("hook") != std::string::npos) {
                marker.section = SectionMetadata{"Chorus", 0.85f, 0.65f};
                marker.colorHex = 0xD39A3A;
            } else if (normalized.find("bridge") != std::string::npos) {
                marker.section = SectionMetadata{"Bridge", 0.70f, 0.45f};
                marker.colorHex = 0x8E8E93;
            } else if (normalized.find("verse") != std::string::npos) {
                marker.section = SectionMetadata{"Verse", 0.55f, 0.50f};
                marker.colorHex = 0x2A82E4;
            } else if (normalized.find("intro") != std::string::npos ||
                       normalized.find("outro") != std::string::npos) {
                marker.section = SectionMetadata{"Transition", 0.35f, 0.50f};
                marker.colorHex = 0x30A46C;
            } else {
                marker.section.type = "Marker";
                marker.section.tensionTarget = std::clamp(marker.section.tensionTarget, 0.0f, 1.0f);
                marker.section.valenceTarget = std::clamp(marker.section.valenceTarget, 0.0f, 1.0f);
            }
        }
        m_lastUpdatePos = m_markers.empty() ? 0 : m_markers.rbegin()->first;
    }

    void addSmartMarker(uint64_t pos, const std::string& name, uint32_t color) {
        if (name.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        Marker marker{};
        marker.samplePosition = pos;
        marker.name = name.substr(0, 256);
        marker.colorHex = color;
        marker.section = SectionMetadata{"Marker", 0.5f, 0.5f};
        m_markers[pos] = std::move(marker);
    }

    bool removeMarker(uint64_t pos) {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_markers.erase(pos) != 0;
    }

    std::vector<Marker> getMarkers() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<Marker> result;
        result.reserve(m_markers.size());
        for (const auto& [position, marker] : m_markers) {
            (void)position;
            result.push_back(marker);
        }
        return result;
    }

    bool findMarkerAtOrBefore(uint64_t pos, Marker& result) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_markers.upper_bound(pos);
        if (it == m_markers.begin()) return false;
        --it;
        result = it->second;
        return true;
    }

private:
    ProjectMarkerSystem() : m_lastUpdatePos(0) {}
    std::map<uint64_t, Marker> m_markers;
    mutable std::mutex m_mutex;
    uint64_t m_lastUpdatePos;
};

} // namespace Aura::Core::Engine
