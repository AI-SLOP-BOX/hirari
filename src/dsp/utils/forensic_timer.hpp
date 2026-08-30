#pragma once
#include <chrono>
#include "../../core/aura_message_bus.hpp"

namespace Aura::DSP::Utils {

/**
 * @class ForensicTimer
 * @brief High-resolution RAII timer for professional DSP performance auditing.
 * INDUSTRIAL: Measures execution time and reports it to the AuraMessageBus with zero-technical drift.
 */
class ForensicTimer {
public:
    /**
     * @brief Starts the timer for a specific component.
     */
    explicit ForensicTimer(uint32_t componentId) 
        : m_componentId(componentId), 
          m_start(std::chrono::high_resolution_clock::now()) {}

    /**
     * @brief Automatically calculates and reports duration on destruction.
     */
    ~ForensicTimer() {
        auto end = std::chrono::high_resolution_clock::now();
        auto duration = std::chrono::duration<float, std::milli>(end - m_start).count();
        
        // INDUSTRIAL: Dispatch performance telemetry to the sovereign message bus.
        ::Aura::Core::AuraMessageBus::getInstance().push(
            ::Aura::Core::AuraMessageBus::DSPPerformance{ m_componentId, duration }
        );
    }

private:
    uint32_t m_componentId;
    std::chrono::time_point<std::chrono::high_resolution_clock> m_start;
};

} // namespace Aura::DSP::Utils
