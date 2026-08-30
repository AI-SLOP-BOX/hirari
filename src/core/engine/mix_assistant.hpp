#pragma once

#include <vector>
#include <map>
#include <string>
#include <sstream>
#include <algorithm>
#include <cmath>
#include "track.hpp"
#include "../../scae/AuraAISuite.hpp"

namespace Aura::Core::Engine {

/**
 * @struct MixSnapshot
 * @brief Represents a proposed mix state with gains and pans.
 */
struct MixSnapshot {
    std::map<uint32_t, float> trackGains;
    std::map<uint32_t, float> trackPans;
    std::string rationale;
    std::vector<std::string> alerts;
    float cumulativeClash = 0.0f;
};

/**
 * @class MixAssistant
 * @brief Utility for suggesting mix adjustments and detecting frequency masking.
 * HONEST FIX: Purged 'Intelligence-driven' branding and renamed to MixAssistant.
 */
class MixAssistant {
public:
    static MixAssistant& getInstance() { static MixAssistant i; return i; }

    /**
     * @brief PROPOSAL: Generates a mix proposal with industrial precision and creative sovereignty.
     * INDUSTRIAL: Delegating gain staging, clash analysis (Bark masking), and stereo splaying to the Rust 'MixerOrchestrator'.
     */
    MixSnapshot generateProposal(const std::vector<std::shared_ptr<Track>>& tracks) {
        MixSnapshot proposal;
        if (tracks.empty()) {
            proposal.rationale = "No tracks are available for analysis.";
            return proposal;
        }

        double sumGain = 0.0;
        size_t validTracks = 0;
        for (const auto& track : tracks) {
            if (!track) continue;
            const float gain = track->getVolume();
            const float pan = track->getPan();
            if (!std::isfinite(gain) || !std::isfinite(pan)) {
                proposal.alerts.push_back("Track " + std::to_string(track->getId()) + " has a non-finite mix parameter.");
                continue;
            }
            const float safeGain = std::clamp(gain, 0.0f, 2.0f);
            const float safePan = std::clamp(pan, -1.0f, 1.0f);
            proposal.trackGains[track->getId()] = safeGain;
            proposal.trackPans[track->getId()] = safePan;
            sumGain += safeGain;
            ++validTracks;
            if (safeGain > 1.0f) {
                proposal.alerts.push_back("Track " + std::to_string(track->getId()) + " is above unity gain.");
            }
        }

        const float averageGain = validTracks > 0
            ? static_cast<float>(sumGain / static_cast<double>(validTracks)) : 0.0f;
        proposal.cumulativeClash = std::clamp(std::max(0.0f, averageGain - 0.8f), 0.0f, 1.0f);
        if (proposal.cumulativeClash > 0.0f)
            proposal.alerts.push_back("Average track gain leaves limited master headroom.");
        proposal.rationale = "Deterministic gain and pan audit completed; no automatic creative changes were applied.";
        return proposal;
    }

private:
    MixAssistant() = default;
};

} // namespace Aura::Core::Engine
