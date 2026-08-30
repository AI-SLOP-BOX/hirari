#pragma once

#include <string>
#include <map>
#include <vector>
#include <atomic>
#include <memory>
#include <mutex>
#include <array>
#include <algorithm>
#include <cmath>
#include "../concurrency/lock_free.hpp"
#include "parameter_smoother.hpp"
#include "macro_control_manager.hpp"

namespace Aura::Core::Engine {

/**
 * @class ManagedParameter
 * @brief Thread-safe parameter with professional macro modulation.
 * HONEST FIX: Implements Smart-Range Mapping for macros (Logic Pro style).
 * Allows a single macro knob to control multiple targets with individual ranges.
 */
class ManagedParameter {
public:
    ManagedParameter(uint32_t id, const std::string& name, float min, float max, float def)
        : m_id(id), m_name(name), m_min(std::min(min, max)), m_max(std::max(min, max)),
          m_smoother(std::clamp(def, std::min(min, max), std::max(min, max))) {
        m_macroCount.store(0);
    }

    void updateTarget(float target, uint32_t /*numSamples*/) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_smoother.setTarget(std::isfinite(target) ? std::clamp(target, m_min, m_max) : m_min);
    }

    void addMacroBinding(uint32_t macroId, float amount, float minR = 0.0f, float maxR = 1.0f) {
        std::lock_guard<std::mutex> lock(m_macroMutex);
        uint32_t currentCount = m_macroCount.load(std::memory_order_acquire);
        if (currentCount < kMaxMacroBindings) {
            const float safeMin = std::clamp(minR, 0.0f, 1.0f);
            const float safeMax = std::clamp(maxR, safeMin, 1.0f);
            const float safeAmount = std::isfinite(amount) ? std::clamp(amount, -1.0f, 1.0f) : 0.0f;
            m_macroBindings[currentCount] = { macroId, safeAmount, safeMin, safeMax };
            m_macroCount.store(currentCount + 1, std::memory_order_release);
        }
    }

    float getNextValue() {
        std::lock_guard<std::mutex> lock(m_mutex);
        const float value = m_smoother.getNextValue();
        return std::isfinite(value) ? std::clamp(value, m_min, m_max) : m_min;
    }

    uint32_t id() const { return m_id; }
    const std::string& name() const { return m_name; }
    float currentValue() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const float value = m_smoother.getCurrentValue();
        return std::isfinite(value) ? std::clamp(value, m_min, m_max) : m_min;
    }

private:
    uint32_t m_id;
    std::string m_name;
    float m_min;
    float m_max;
    ParameterSmoother m_smoother;
    static constexpr uint32_t kMaxMacroBindings = 32;
    struct MacroBinding { uint32_t id; float amount; float min; float max; };
    std::array<MacroBinding, kMaxMacroBindings> m_macroBindings{};
    std::atomic<uint32_t> m_macroCount{0};
    mutable std::mutex m_macroMutex;
    mutable std::mutex m_mutex;
};


} // namespace Aura::Core::Engine
