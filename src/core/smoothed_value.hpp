#pragma once

#include <cmath>
#include <algorithm>

namespace Aura::Core {

/**
 * @file smoothed_value.hpp
 * @brief Thread-Safety Contract:
 * Classes in this file are NOT thread-safe by default. Calling setTarget() / setSmoothingTime()
 * from a control/UI thread while calling next() / process() / applyGain() from the real-time audio thread
 * concurrently is undefined behavior (data race).
 * 
 * Synchronization MUST be managed by the caller, ideally by using thread-safe lock-free queues
 * to pass parameter update events into the real-time thread, where values are updated and processed sequentially.
 */

/**
 * @brief LinearSmoothedValue: Eliminates zipper noise by ramping values linearly.
 * Uses a countdown counter approach to ensure exact convergence regardless of float rounding errors.
 */
class LinearSmoothedValue {
public:
    LinearSmoothedValue(float initial, double sr)
        : m_current(std::isfinite(initial) ? initial : 0.0f)
        , m_target(std::isfinite(initial) ? initial : 0.0f)
        , m_sampleRate(std::isfinite(sr) && sr > 0.0 ? sr : 44100.0)
        , m_ms(20.0f)
        , m_step(0.0f)
        , m_stepsRemaining(0)
    {
        recalculateStep();
    }

    void setTarget(float value) {
        if (!std::isfinite(value)) return;
        m_target = value;
        recalculateStep();
    }

    void setSmoothingTime(float ms) {
        m_ms = std::max(0.0f, std::isfinite(ms) ? ms : 0.0f);
        recalculateStep();
    }

    void updateSR(double sr) {
        if (std::isfinite(sr) && sr > 0.0) {
            m_sampleRate = sr;
            recalculateStep();
        }
    }

    /**
     * @brief Gets the next smoothed sample value.
     */
    float next() {
        if (m_stepsRemaining <= 0) {
            m_current = m_target;
            return m_current;
        }
        --m_stepsRemaining;
        m_current += m_step;
        if (m_stepsRemaining <= 0) {
            m_current = m_target;
        }
        return m_current;
    }

    /**
     * @brief Applies smoothing directly to a block of samples (multiplicative/gain).
     * Highly optimized to avoid sample-by-sample checks once target is reached.
     */
    void applyGain(float* buffer, int numSamples) {
        if (numSamples <= 0) return;
        
        if (m_stepsRemaining <= 0) {
            m_current = m_target;
            if (m_target == 1.0f) return;
            for (int i = 0; i < numSamples; ++i) {
                buffer[i] *= m_target;
            }
            return;
        }

        int i = 0;
        while (i < numSamples && m_stepsRemaining > 0) {
            --m_stepsRemaining;
            m_current += m_step;
            buffer[i] *= m_current;
            ++i;
        }
        if (m_stepsRemaining <= 0) {
            m_current = m_target;
        }
        
        if (i < numSamples && m_target != 1.0f) {
            for (; i < numSamples; ++i) {
                buffer[i] *= m_target;
            }
        }
    }

    /**
     * @brief Writes smoothed values to a target buffer (e.g. for parameter control rate buffers).
     */
    void process(float* buffer, int numSamples) {
        if (numSamples <= 0) return;

        if (m_stepsRemaining <= 0) {
            m_current = m_target;
            std::fill_n(buffer, numSamples, m_target);
            return;
        }

        int i = 0;
        while (i < numSamples && m_stepsRemaining > 0) {
            --m_stepsRemaining;
            m_current += m_step;
            buffer[i] = m_current;
            ++i;
        }
        if (m_stepsRemaining <= 0) {
            m_current = m_target;
        }

        if (i < numSamples) {
            std::fill_n(buffer + i, numSamples - i, m_target);
        }
    }

    bool isSmoothing() const { return m_stepsRemaining > 0; }
    float getCurrentValue() const { return m_current; }
    float getTargetValue() const { return m_target; }

private:
    float m_current;
    float m_target;
    double m_sampleRate;
    float m_ms;
    float m_step;
    int m_stepsRemaining;

    void recalculateStep() {
        const double samples = static_cast<double>(m_ms) * 0.001 * m_sampleRate;
        if (samples <= 1.0 || !std::isfinite(samples)) {
            m_stepsRemaining = 0;
            m_step = 0.0f;
            m_current = m_target;
        } else {
            m_stepsRemaining = static_cast<int>(std::round(samples));
            m_step = (m_target - m_current) / static_cast<float>(m_stepsRemaining);
        }
    }
};

/**
 * @brief ExponentialSmoothedValue: Eliminates zipper noise using an exponential convergence curve.
 * Ideal for logarithmic parameters such as gain, volume faders, and frequency sweeps.
 */
class ExponentialSmoothedValue {
public:
    ExponentialSmoothedValue(float initial, double sr)
        : m_current(std::isfinite(initial) ? initial : 0.0f)
        , m_target(std::isfinite(initial) ? initial : 0.0f)
        , m_sampleRate(std::isfinite(sr) && sr > 0.0 ? sr : 44100.0)
        , m_ms(20.0f)
        , m_coeff(1.0)
    {
        recalculateCoeff();
    }

    void setTarget(float value) {
        if (!std::isfinite(value)) return;
        m_target = value;
    }

    void setSmoothingTime(float ms) {
        m_ms = std::max(0.0f, std::isfinite(ms) ? ms : 0.0f);
        recalculateCoeff();
    }

    void updateSR(double sr) {
        if (std::isfinite(sr) && sr > 0.0) {
            m_sampleRate = sr;
            recalculateCoeff();
        }
    }

    /**
     * @brief Gets the next smoothed sample value.
     */
    float next() {
        if (std::abs(m_target - m_current) < 1e-6f || (m_target == 0.0f && std::abs(m_current) < 1.0e-24f)) {
            m_current = m_target;
            return m_current;
        }
        m_current = m_current + static_cast<float>(m_coeff) * (m_target - m_current);
        if (std::abs(m_current) < 1.0e-24f) m_current = 0.0f;
        return m_current;
    }

    /**
     * @brief Applies exponential smoothing directly to a block of samples (multiplicative/gain).
     */
    void applyGain(float* buffer, int numSamples) {
        if (numSamples <= 0) return;
        
        if (std::abs(m_target - m_current) < 1e-6f || (m_target == 0.0f && std::abs(m_current) < 1.0e-24f)) {
            m_current = m_target;
            if (m_target == 1.0f) return;
            for (int i = 0; i < numSamples; ++i) {
                buffer[i] *= m_target;
            }
            return;
        }

        const float c = static_cast<float>(m_coeff);
        for (int i = 0; i < numSamples; ++i) {
            m_current = m_current + c * (m_target - m_current);
            if (std::abs(m_current) < 1.0e-24f) m_current = 0.0f;
            buffer[i] *= m_current;
        }
        
        if (std::abs(m_target - m_current) < 1e-6f || (m_target == 0.0f && std::abs(m_current) < 1.0e-24f)) {
            m_current = m_target;
        }
    }

    /**
     * @brief Writes smoothed values to a target buffer.
     */
    void process(float* buffer, int numSamples) {
        if (numSamples <= 0) return;

        if (std::abs(m_target - m_current) < 1e-6f || (m_target == 0.0f && std::abs(m_current) < 1.0e-24f)) {
            m_current = m_target;
            std::fill_n(buffer, numSamples, m_target);
            return;
        }

        const float c = static_cast<float>(m_coeff);
        for (int i = 0; i < numSamples; ++i) {
            m_current = m_current + c * (m_target - m_current);
            if (std::abs(m_current) < 1.0e-24f) m_current = 0.0f;
            buffer[i] = m_current;
        }

        if (std::abs(m_target - m_current) < 1e-6f || (m_target == 0.0f && std::abs(m_current) < 1.0e-24f)) {
            m_current = m_target;
        }
    }

    bool isSmoothing() const { return std::abs(m_target - m_current) >= 1e-6f && !(m_target == 0.0f && std::abs(m_current) < 1.0e-24f); }
    float getCurrentValue() const { return m_current; }
    float getTargetValue() const { return m_target; }

private:
    float m_current;
    float m_target;
    double m_sampleRate;
    float m_ms;
    double m_coeff;

    void recalculateCoeff() {
        if (m_ms <= 0.0f) {
            m_coeff = 1.0;
        } else {
            // Target ~99.9% convergence time
            // For 99.9% convergence in N samples: e^(-coeff * N) = 0.001
            // coeff = -ln(0.001) / N = 6.907755 / N
            const double N = static_cast<double>(m_ms) * 0.001 * m_sampleRate;
            if (N <= 1.0) {
                m_coeff = 1.0;
            } else {
                m_coeff = 1.0 - std::exp(-6.907755278982137 / N);
            }
        }
    }
};

} // namespace Aura::Core
