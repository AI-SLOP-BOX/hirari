#pragma once

#include <string>
#include <vector>
#include <map>
#include <algorithm>
#include <cmath>
#include <limits>

namespace Aura::Core::DSP::Synthesis {

/**
 * @brief InstrumentPatch: Metadata-driven preset for synthesis engines.
 */
struct InstrumentPatch {
    std::string name;
    std::string category; // Bass, Lead, Pad
    float brightness;      // 0.0 to 1.0 (Meta-parameter)
    float dynamics;        // 0.0 to 1.0
};

/**
 * @brief InstrumentLibrary: High-performance repository for all engine patches.
 * Allows AuraAssistant and SynthesisConductor to query sounds by musical character.
 */
class InstrumentLibrary {
public:
    static InstrumentLibrary& getInstance() {
        static InstrumentLibrary instance;
        return instance;
    }

    void registerPatch(uint32_t id, const std::string& name, const std::string& category) {
        m_patches[id] = {name, category, 0.5f, 0.5f};
    }

    bool setPatchCharacter(uint32_t id, float brightness, float dynamics) {
        auto it = m_patches.find(id);
        if (it == m_patches.end() || !std::isfinite(brightness) || !std::isfinite(dynamics)) return false;
        it->second.brightness = std::clamp(brightness, 0.0f, 1.0f);
        it->second.dynamics = std::clamp(dynamics, 0.0f, 1.0f);
        return true;
    }

    const InstrumentPatch* getPatch(uint32_t id) const {
        auto it = m_patches.find(id);
        return (it != m_patches.end()) ? &it->second : nullptr;
    }

    /**
     * @brief AI-ready search based on category and character.
     */
    uint32_t findBestMatch(const std::string& category, float brightness) {
        if (!std::isfinite(brightness)) brightness = 0.5f;
        brightness = std::clamp(brightness, 0.0f, 1.0f);
        uint32_t bestId = 0;
        float bestDistance = std::numeric_limits<float>::max();
        for (const auto& [id, patch] : m_patches) {
            if (patch.category != category) continue;
            const float distance = std::fabs(patch.brightness - brightness);
            if (distance < bestDistance) { bestDistance = distance; bestId = id; }
        }
        return bestDistance <= 0.2f ? bestId : 0;
    }

private:
    InstrumentLibrary() = default;
    std::map<uint32_t, InstrumentPatch> m_patches;
};

} // namespace Aura::Core::DSP::Synthesis
