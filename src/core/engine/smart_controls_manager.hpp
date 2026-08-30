#pragma once
#include <vector>
#include <unordered_map>
#include <algorithm>
#include <cmath>
#include "macro_control_manager.hpp"

namespace Aura::Core::Engine {

/**
 * @struct ControlMapping
 * @brief High-level mapping from UI knob to Plugin parameter.
 */
struct ControlMapping {
    uint32_t trackId;
    uint32_t pluginId;
    uint32_t paramId;
    float rangeMin;
    float rangeMax;
    bool inverted;
};

/**
 * @class SmartControlsManager
 * @brief High-level Orchestration for "One Knob" UI macros.
 * HONEST FIX: Decoupled UI logic from audio-thread math by using MacroControlManager.
 */
class SmartControlsManager {
public:
    static SmartControlsManager& getInstance() { static SmartControlsManager i; return i; }

    void addMapping(uint32_t smartId, const ControlMapping& m) {
        if (smartId >= MacroControlManager::kMaxMacros ||
            !std::isfinite(m.rangeMin) || !std::isfinite(m.rangeMax)) return;
        MacroMapping mapping;
        mapping.targetParamId = targetId(m.pluginId, m.paramId);
        mapping.min = std::clamp(m.rangeMin, 0.0f, 1.0f);
        mapping.max = std::clamp(m.rangeMax, 0.0f, 1.0f);
        mapping.invert = m.inverted;
        MacroControlManager::getInstance().addMapping(smartId, mapping);
        m_mappings[smartId].push_back(m);
    }

    /**
     * @brief Sets the value from the UI side with industrial-grade resolution.
     */
    void setSmartValue(uint32_t smartId, float normalizedValue) {
        if (smartId >= MacroControlManager::kMaxMacros || !std::isfinite(normalizedValue)) return;
        MacroControlManager::getInstance().setMacroValue(smartId, normalizedValue);
    }

    float getMappedValue(uint32_t smartId, uint32_t pluginId, uint32_t paramId) const {
        if (smartId >= MacroControlManager::kMaxMacros) return 0.0f;
        return MacroControlManager::getInstance().getMappedValue(smartId, targetId(pluginId, paramId));
    }

    void clearMappings(uint32_t smartId) {
        if (smartId < MacroControlManager::kMaxMacros) {
            MacroControlManager::getInstance().clearMappings(smartId);
            m_mappings.erase(smartId);
        }
    }

private:
    static uint32_t targetId(uint32_t pluginId, uint32_t paramId) noexcept {
        return (pluginId * 4096u) ^ (paramId & 0xFFFu);
    }

    std::unordered_map<uint32_t, std::vector<ControlMapping>> m_mappings;
};

} // namespace Aura::Core::Engine
