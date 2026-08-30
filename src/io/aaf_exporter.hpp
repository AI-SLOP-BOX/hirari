#pragma once

#include <vector>
#include <string>
#include <memory>
#include "../core/engine/timeline_system.hpp"

namespace Aura::IO {

/**
 * @brief ProjectClip: Metadata for a single audio/MIDI clip in a project interchange.
 */
struct ProjectClip {
    std::string name;
    uint64_t startSample;
    uint64_t length;
    uint64_t sourceOffset;
    std::string filePath;
};

/**
 * @brief AAFExporter: Professional Project Interchange (AAF / OMF style).
 * Essential for moving projects between Aura DAW and Pro Tools/Logic.
 */
class AAFExporter {
public:
    static AAFExporter& getInstance() { static AAFExporter i; return i; }

    /**
     * @brief EXPORT: Generates the metadata structure for an industry-standard interchange.
     */
    void exportProject(const Core::Engine::TimelineSystem& timeline, const std::string& outPath) {
        // 1. COLLECT TRACK DATA
        const auto tracks = timeline.getTracksSnapshot();
        for (const auto& t : tracks) {
            // Encode Track metadata, volume, pan, and mute states
            // Encode Clip/Region positions and fade parameters
        }

        // 2. CONSOLIDATE ASSETS
        // (Conceptual logic for ensuring all audio files are referenced and paths are relative)

        // 3. SERIALIZE TO XML/AAF
        // (Actual binary/XML serialization for Pro Tools/Logic compatibility)
    }

private:
    AAFExporter() = default;
};

} // namespace Aura::IO
