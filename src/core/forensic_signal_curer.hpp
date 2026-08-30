#pragma once
#include <cmath>
#include <algorithm>

namespace Aura::Core {

/**
 * @brief ForensicSignalCurer: Centralized signal integrity auditor.
 * INDUSTRIAL: Scans for NaNs, Infs, and Subnormals in a single pass.
 * This allows individual DSP components to omit safety checks for maximum speed.
 */
class ForensicSignalCurer {
public:
    /**
     * @brief Cures a buffer by replacing non-finite and subnormal values with zero.
     */
    static void cure(float* buffer, size_t count) {
        // INDUSTRIAL: This loop is a prime candidate for auto-vectorization.
        for (size_t i = 0; i < count; ++i) {
            float v = buffer[i];
            
            // Check for NaN or Infinity
            if (!std::isfinite(v)) {
                buffer[i] = 0.0f;
                continue;
            }
            
            // Kill subnormals (Denormals) to avoid CPU spikes on certain architectures
            // (Standard DAWs like Logic Pro do this at the end of every plugin chain)
            if (std::abs(v) < 1e-12f && v != 0.0f) {
                buffer[i] = 0.0f;
            }
        }
    }
    
    /**
     * @brief Specialized stereo curing.
     */
    static void cureStereo(float* l, float* r, size_t count) {
        cure(l, count);
        cure(r, count);
    }
};

} // namespace Aura::Core
