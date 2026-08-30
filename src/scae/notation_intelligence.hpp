#pragma once
#include <vector>
#include <string>
#include <memory>

namespace Aura::Core::Engine { class Track; }
namespace Aura::Core { class MidiRegion; }

namespace Aura::SCAE::Intelligence {

/**
 * @class NotationIntelligence
 * @brief High-Intelligence Music Engraving & Analysis Library.
 */
class NotationIntelligence {
public:
    struct ScoreGlyph {
        float beat;
        float staffOffset;
        std::string symbolType;
        int duration;
        int voice = 2;
        // Source metadata lets notation edits update the real MIDI model
        // instead of only changing a rendered glyph.
        uint8_t sourceKind = 0; // 0 = audio anchor, 1 = MIDI note
        uint32_t sourceRegionId = 0;
        uint32_t sourceNoteIndex = 0;
    };

    static std::vector<ScoreGlyph> generateScoreManifest(const ::Aura::Core::Engine::Track& track);
    static std::vector<ScoreGlyph> generateMidiManifest(
        const std::vector<std::shared_ptr<::Aura::Core::MidiRegion>>& regions);

    static std::string conductHarmonicAudit(const std::vector<std::shared_ptr<::Aura::Core::Engine::Track>>& tracks);
};

} // namespace Aura::SCAE::Intelligence
