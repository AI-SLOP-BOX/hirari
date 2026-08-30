#pragma once
#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::Core {

/**
 * @class ParameterSmoother
 * @brief High-precision parameter smoothing engine (anti-zipper filter).
 * Implements a one-pole low-pass filter to smooth parameters.
 */
class ParameterSmoother {
public:
    explicit ParameterSmoother(float initialValue = 0.0f) {
        m_target.store(initialValue);
        m_current.store(initialValue);
        m_a.store(0.01f);
    }

    void setTarget(float value) { m_target.store(value, std::memory_order_relaxed); }
    
    void reset(float value) {
        m_target.store(value, std::memory_order_relaxed);
        m_current.store(value, std::memory_order_relaxed);
    }

    void setSmoothingTime(float ms, float sr) {
        if (ms <= 0.0f || sr <= 0.0f) {
            m_a.store(1.0f, std::memory_order_relaxed);
            return;
        }
        float tau = ms / 1000.0f;
        float coef = 1.0f - std::exp(-1.0f / (sr * tau));
        m_a.store(coef, std::memory_order_relaxed);
    }

    void process(float* buffer, uint32_t len) {
        float current = m_current.load(std::memory_order_relaxed);
        float target = m_target.load(std::memory_order_relaxed);
        float a = m_a.load(std::memory_order_relaxed);
        
        for (uint32_t i = 0; i < len; ++i) {
            current = current + a * (target - current);
            buffer[i] = current;
        }
        m_current.store(current, std::memory_order_relaxed);
    }

    float getNextValue() {
        float current = m_current.load(std::memory_order_relaxed);
        float target = m_target.load(std::memory_order_relaxed);
        float a = m_a.load(std::memory_order_relaxed);
        current = current + a * (target - current);
        m_current.store(current, std::memory_order_relaxed);
        return current;
    }

    float getCurrentValue() const {
        return m_current.load(std::memory_order_relaxed);
    }

private:
    std::atomic<float> m_target;
    std::atomic<float> m_current;
    std::atomic<float> m_a;
};

} // namespace Aura::Core
