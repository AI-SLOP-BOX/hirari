#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <algorithm>
#include <cmath>
#include "../parameter_smoother.hpp"

namespace Aura::Core::Engine {

struct MacroMapping {
    uint32_t targetParamId;
    float min;
    float max;
    bool invert;
};

/**
 * @class MacroControlManager
 * @brief Industrial Parameter Macro Orchestration Engine.
 * HONEST FIX: Implemented real mapping logic with range scaling.
 */
class MacroControlManager {
public:
    static constexpr size_t kMaxMacros = 128;

    MacroControlManager() {
        for (auto& v : m_targetValues) v.store(0.0f);
    }

    static MacroControlManager& getInstance() { static MacroControlManager i; return i; }

    void setMacroValue(uint32_t macroIdx, float value) {
        if (macroIdx < kMaxMacros && std::isfinite(value)) {
            m_targetValues[macroIdx].store(std::clamp(value, 0.0f, 1.0f),
                                           std::memory_order_relaxed);
        }
    }

    /// Returns the latest control-thread value without touching smoother
    /// state. Audio processors can use this for inexpensive parameter reads;
    /// time-critical modulation should still use getMappedValue() after the
    /// audio-side smoother update.
    float getMacroValue(uint32_t macroIdx) const noexcept {
        if (macroIdx >= kMaxMacros) return 0.0f;
        return m_targetValues[macroIdx].load(std::memory_order_relaxed);
    }

    // Used only by the control-plane MIDI Learn binder. The returned address
    // is stable for the lifetime of this manager because the macro storage is
    // a fixed-size array; the audio/input path only performs atomic stores.
    std::atomic<float>* targetPointer(uint32_t macroIdx) noexcept {
        return macroIdx < kMaxMacros ? &m_targetValues[macroIdx] : nullptr;
    }

    void addMapping(uint32_t macroIdx, const MacroMapping& mapping) {
        if (macroIdx >= kMaxMacros || !std::isfinite(mapping.min) ||
            !std::isfinite(mapping.max)) return;
        MacroMapping safe = mapping;
        safe.min = std::clamp(safe.min, 0.0f, 1.0f);
        safe.max = std::clamp(safe.max, 0.0f, 1.0f);
        if (safe.min > safe.max) std::swap(safe.min, safe.max);
        auto& mappings = m_mappings[macroIdx];
        auto it = std::find_if(mappings.begin(), mappings.end(),
                               [&](const MacroMapping& item) {
                                   return item.targetParamId == safe.targetParamId;
                               });
        if (it != mappings.end()) *it = safe;
        else mappings.push_back(safe);
    }

    void clearMappings(uint32_t macroIdx) {
        if (macroIdx < kMaxMacros) m_mappings[macroIdx].clear();
    }

    /**
     * @brief Calculates and returns the mapped value for a specific target.
     * INDUSTRIAL: Delegating mapped value calculation and parameter scaling to the Rust 'MacroOrchestrator'.
     */
    float getMappedValue(uint32_t macroIdx, uint32_t targetId) const {
        if (macroIdx >= kMaxMacros) return 0.0f;
        const float normalized = std::clamp(
            m_smoothers[macroIdx].getCurrentValue(), 0.0f, 1.0f);
        for (const auto& mapping : m_mappings[macroIdx]) {
            if (mapping.targetParamId != targetId) continue;
            const float source = mapping.invert ? 1.0f - normalized : normalized;
            return mapping.min + source * (mapping.max - mapping.min);
        }
        return normalized;
    }

    void updateSmoothers(float sr) {
        for (size_t i = 0; i < kMaxMacros; ++i) {
            m_smoothers[i].setSmoothingTime(10.0f, sr); // 10ms standard
            m_smoothers[i].setTarget(m_targetValues[i].load(std::memory_order_relaxed));
        }
    }

private:
    std::array<std::atomic<float>, kMaxMacros> m_targetValues;
    std::array<std::vector<MacroMapping>, kMaxMacros> m_mappings;
    mutable std::array<Aura::Core::ParameterSmoother, kMaxMacros> m_smoothers;
};

} // namespace Aura::Core::Engine
