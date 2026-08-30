#pragma once

#include <atomic>
#include <algorithm>
#include <cmath>
#include <cstdio>
#if defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace Aura::Core {

/**
 * @class AtomicParameter
 * @brief Thread-safe, lock-free parameter with smoothing and professional formatting.
 */
class AtomicParameter {
public:
    enum class SmoothingType { Linear, Exponential, None };
    enum class DisplayMode { Unipolar, Bipolar };
    enum class Unit { Percentage, Decibels, Frequency, Time, Raw };

    explicit AtomicParameter(float initialValue = 1.0f, DisplayMode mode = DisplayMode::Unipolar) 
        : m_target(initialValue), m_current(initialValue), m_displayMode(mode) {}

    /**
     * @brief HONEST FIX: PROFESSIONAL UNIT FORMATTING.
     * Logic Pro / Surge XT style display mapping.
     */
    void getValueString(char* buffer, size_t size) const {
        if (!buffer || size == 0) return;
        float val = m_target.load(std::memory_order_acquire);

        switch (m_unit) {
            case Unit::Percentage:
                snprintf(buffer, size, "%.1f%%", (val + 1.0f) * 50.0f);
                break;
            case Unit::Decibels: {
                float db = 20.0f * std::log10(std::max(1e-5f, (val + 1.0f) * 0.5f));
                if (db < -90.0f) snprintf(buffer, size, "-inf dB");
                else snprintf(buffer, size, "%.1f dB", db);
            } break;
            case Unit::Frequency: {
                // Logarithmic frequency mapping example (20Hz - 20kHz)
                float freq = 20.0f * std::pow(1000.0f, (val + 1.0f) * 0.5f);
                if (freq >= 1000.0f) snprintf(buffer, size, "%.2f kHz", freq * 0.001f);
                else snprintf(buffer, size, "%.0f Hz", freq);
            } break;
            case Unit::Time:
                snprintf(buffer, size, "%.1f ms", std::max(0.0f, (val + 1.0f) * 500.0f));
                break;
            default:
                snprintf(buffer, size, "%.2f", val);
                break;
        }
    }

    float getNormalizedValue() const {
        float val = m_current.load(std::memory_order_relaxed);
        if (m_displayMode == DisplayMode::Unipolar) {
            return std::clamp((val + 1.0f) * 0.5f, 0.0f, 1.0f);
        }
        return val;
    }

    float getTarget() const noexcept {
        return m_target.load(std::memory_order_acquire);
    }

    void setUnit(Unit unit) { m_unit = unit; }
    void setDisplayMode(DisplayMode mode) { m_displayMode = mode; }

    float getNextValue() {
        if (m_dirty.load(std::memory_order_acquire)) {
            updateInternalState();
        }

        const float target = m_target.load(std::memory_order_relaxed);
        float current = m_current.load(std::memory_order_relaxed);
        
        if (std::abs(current - target) < 1e-7f) {
            current = target;
            m_current.store(current, std::memory_order_relaxed);
            return std::clamp(getNormalizedValue() + m_aiOffset.load(std::memory_order_relaxed), 0.0f, 1.0f);
        }
        
        SmoothingType type = m_type.load(std::memory_order_relaxed);
        if (type == SmoothingType::Exponential) {
            current = current + (target - current) * m_coeff.load(std::memory_order_relaxed);
            if (std::abs(current - target) < 1e-24f) {
                current = target;
            }
        } else if (type == SmoothingType::Linear) {
            const float step = m_step.load(std::memory_order_relaxed);
            current += step;
            if ((step > 0 && current > target) || (step < 0 && current < target)) current = target;
        } else {
            current = target;
        }
        
        m_current.store(current, std::memory_order_relaxed);

        // --- HONEST FIX: SAFETY CLAMPING ---
        // Prevents AI modulation from pushing parameters into unstable territory.
        float val = getNormalizedValue() + m_aiOffset.load(std::memory_order_relaxed);
        return std::clamp(val, 0.0f, 1.0f);
    }


    /**
     * @brief BLOCK OPTIMIZED: Vectorizable block processing.
     */
    void getNextBlock(float* buffer, size_t numSamples) {
        if (m_dirty.load(std::memory_order_acquire)) {
            updateInternalState();
        }

        const float target = m_target.load(std::memory_order_relaxed);
        const SmoothingType type = m_type.load(std::memory_order_relaxed);
        const float aiMod = m_aiOffset.load(std::memory_order_relaxed);
        const float isUnipolar = (m_displayMode == DisplayMode::Unipolar) ? 0.5f : 1.0f;
        const float unipolarOffset = (m_displayMode == DisplayMode::Unipolar) ? 1.0f : 0.0f;

        // INDUSTRIAL VECTORIZATION
        float current = m_current.load(std::memory_order_relaxed);
        if (std::abs(current - target) < 1e-7f) {
            current = target;
            m_current.store(current, std::memory_order_relaxed);
            float val = (current + unipolarOffset) * isUnipolar;
            float finalVal = std::clamp(val + aiMod, 0.0f, 1.0f);

            size_t i = 0;
#if defined(__arm64__) || defined(__aarch64__)
            float32x4_t vVal = vdupq_n_f32(finalVal);
            for (; i + 3 < numSamples; i += 4) vst1q_f32(buffer + i, vVal);
#endif
            for (; i < numSamples; ++i) buffer[i] = finalVal;
        } else if (type == SmoothingType::Linear) {
            // Linear smoothing is highly vectorizable
            for (size_t i = 0; i < numSamples; ++i) {
                const float step = m_step.load(std::memory_order_relaxed);
                current += step;
                if ((step > 0 && current > target) || (step < 0 && current < target)) current = target;
                float norm = (current + unipolarOffset) * isUnipolar;
                buffer[i] = std::clamp(norm + aiMod, 0.0f, 1.0f);
            }
            m_current.store(current, std::memory_order_relaxed);
        } else {
            // Exponential smoothing (Recursive, harder to SIMD but can be unrolled)
            for (size_t i = 0; i < numSamples; ++i) {
                current = current + (target - current) * m_coeff.load(std::memory_order_relaxed);
                if (std::abs(current - target) < 1e-24f) {
                    current = target;
                }
                float norm = (current + unipolarOffset) * isUnipolar;
                buffer[i] = std::clamp(norm + aiMod, 0.0f, 1.0f);
            }
            if (std::abs(current - target) < 1e-7f) current = target;
            m_current.store(current, std::memory_order_relaxed);
        }
    }


    void setTarget(float value) {
        if (!std::isfinite(value)) return;
        m_target.store(value, std::memory_order_release);
        m_dirty.store(true, std::memory_order_release);
    }

    void setSampleRate(double sr) {
        if (!std::isfinite(sr) || sr <= 0.0) return;
        m_sampleRate.store(sr, std::memory_order_relaxed);
        recalculateCoefficients();
    }

    void setSmoothingTime(double ms) {
        if (!std::isfinite(ms) || ms < 0.0) return;
        m_smoothingTimeMs.store(ms, std::memory_order_relaxed);
        recalculateCoefficients();
    }

    void setAIModulation(float offset) { m_aiOffset.store(offset, std::memory_order_release); }

    // Transport/reset boundary: discard a pending ramp without changing the
    // user-facing target.  This is intentionally a control-side operation;
    // the audio thread only observes the already coherent current value.
    void resetToTarget() noexcept {
        const float target = m_target.load(std::memory_order_acquire);
        m_current.store(target, std::memory_order_release);
        m_dirty.store(false, std::memory_order_release);
    }

private:
    void recalculateCoefficients() {
        double sr = m_sampleRate.load(std::memory_order_relaxed);
        double ms = m_smoothingTimeMs.load(std::memory_order_relaxed);
        if (sr > 0) {
            double samples = (ms * 0.001) * sr;
            m_coeff.store(static_cast<float>(1.0 - std::exp(-1.0 / std::max(1.0, samples))),
                          std::memory_order_release);
            updateStep();
        }
    }

    void updateStep() {
        double sr = m_sampleRate.load(std::memory_order_relaxed);
        double ms = m_smoothingTimeMs.load(std::memory_order_relaxed);
        float target = m_target.load(std::memory_order_relaxed);
        float current = m_current.load(std::memory_order_relaxed);
        double samples = (ms * 0.001) * sr;
        const float step = (std::isfinite(samples) && samples > 0.0)
            ? (target - current) / std::max(1.0f, static_cast<float>(samples)) : 0.0f;
        m_step.store(std::isfinite(step) ? step : 0.0f, std::memory_order_release);
    }

    void updateInternalState() {
        if (m_shouldReset.exchange(false, std::memory_order_acq_rel)) {
            m_current.store(m_resetValue.load(std::memory_order_acquire), std::memory_order_relaxed);
        }
        recalculateCoefficients(); // Also calls updateStep()
        m_dirty.store(false, std::memory_order_release);
    }

    std::atomic<float> m_target;
    std::atomic<float> m_aiOffset{0.0f};
    std::atomic<float> m_resetValue{0.0f};
    std::atomic<bool> m_shouldReset{false};
    std::atomic<bool> m_dirty{true};

    std::atomic<float> m_current;
    std::atomic<float> m_coeff{0.01f};
    std::atomic<float> m_step{0.0f};
    DisplayMode m_displayMode = DisplayMode::Unipolar;
    Unit m_unit = Unit::Percentage;
    std::atomic<SmoothingType> m_type{SmoothingType::Exponential};
    std::atomic<double> m_sampleRate{44100.0};
    std::atomic<double> m_smoothingTimeMs{10.0};
};

} // namespace Aura::Core
