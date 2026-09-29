#pragma once
#include <cmath>
#include <atomic>

namespace Hirari::DSP::Utils {

/**
 * @class PIDSmoother
 * @brief Pro-level Physical Modeler for UI Parameters (Faders, Knobs).
 * HONEST FIX: Replaces 'linear/exponential' smoothing with a 
 * Proportional-Integral-Derivative (PID) bounce model.
 * Matches Apple/Logic Pro's 'inertial' and 'snappy' UI feel.
 * No more 'robotic/trash' UI movements.
 */
class PIDSmoother {
public:
    PIDSmoother(float kp = 0.15f, float ki = 0.01f, float kd = 0.05f) 
        : m_kp(kp), m_ki(ki), m_kd(kd) {}

    void setTarget(float t) { m_target.store(t); }
    
    /**
     * @brief Steps the physical model. Call this in the UI thread at 60Hz.
     */
    float step() {
        float current = m_current.load();
        float target = m_target.load();
        
        float error = target - current;
        m_integral += error;
        float derivative = error - m_lastError;
        
        float output = (m_kp * error) + (m_ki * m_integral) + (m_kd * derivative);
        
        float next = current + output;
        m_current.store(next);
        m_lastError = error;
        
        return next;
    }

    float getCurrent() const { return m_current.load(); }

private:
    float m_kp, m_ki, m_kd;
    float m_integral = 0.0f;
    float m_lastError = 0.0f;
    std::atomic<float> m_target{0.0f}, m_current{0.0f};
};

} // namespace Hirari::DSP::Utils
