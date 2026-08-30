#pragma once
#include "automation_mode.hpp"
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @class AutomationController
 * @brief Manages parameter automation state transitions.
 * HONEST FIX: Implemented real 'Touch' and 'Latch' logic with smoothing.
 */
class AutomationController {
public:
    AutomationController() 
        : m_mode(AutomationMode::Read)
        , m_isRecording(false)
        , m_lastValue(0.0f)
        , m_returnAlpha(0.08f) // Smoother transition (~100-200ms default control-rate return)
        , m_latchActive(false)
    {}

    void setMode(AutomationMode mode) { 
        m_mode = mode; 
        resetLatch();
    }

    void setReturnAlpha(float alpha) {
        if (std::isfinite(alpha)) {
            m_returnAlpha = std::clamp(alpha, 0.001f, 1.0f);
        }
    }

    void resetLatch() {
        m_latchActive = false;
        m_isRecording = false;
    }

    /**
     * @brief Processes the current automation state and returns the final parameter value.
     * INDUSTRIAL: State machine tracking for Touch/Latch/Write modes.
     */
    float process(float curveValue, float userValue, bool isUserInteracting) {
        if (!std::isfinite(curveValue)) curveValue = m_lastValue;
        if (!std::isfinite(userValue)) userValue = curveValue;

        switch (m_mode) {
        case AutomationMode::Read:
            m_isRecording = false;
            m_latchActive = false;
            m_lastValue = curveValue;
            return curveValue;

        case AutomationMode::Write:
            m_isRecording = true;
            m_latchActive = false;
            m_lastValue = userValue;
            return userValue;

        case AutomationMode::Touch:
            m_isRecording = isUserInteracting;
            m_latchActive = false;
            if (isUserInteracting) {
                m_lastValue = userValue;
                return userValue;
            }
            // Smoothly ramp back to the printed curve value
            m_lastValue += (curveValue - m_lastValue) * m_returnAlpha;
            return m_lastValue;

        case AutomationMode::Latch:
            if (isUserInteracting) {
                m_latchActive = true;
            }
            
            if (m_latchActive) {
                m_isRecording = true;
                if (isUserInteracting) {
                    m_lastValue = userValue;
                }
            } else {
                m_isRecording = false;
                m_lastValue = curveValue;
            }
            return m_lastValue;
        }
        
        m_isRecording = false;
        return curveValue;
    }

    bool isRecording() const { return m_isRecording; }

private:
    AutomationMode m_mode;
    bool m_isRecording;
    float m_lastValue;
    float m_returnAlpha;
    bool m_latchActive; // Track latch trigger across block boundaries
};

} // namespace Aura::Core::Engine
