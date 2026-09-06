#pragma once

#include <vector>
#include <cmath>
#include <string>
#include <algorithm>

namespace Aura::DSP::Spatial {

/**
 * @brief SurroundPanner: Professional High-End Spatialization.
 * Ready for Dolby Atmos / 7.1.4 Immersive mixing.
 */
class SurroundPanner {
public:
    enum class Format { 
        Stereo, 
        Surround_5_1, // L, R, C, LFE, Ls, Rs
        Surround_7_1, // L, R, C, LFE, Ls, Rs, Lb, Rb
        Immersive_7_1_4 // Adds ceiling speakers
    };

    SurroundPanner(Format f = Format::Stereo) : m_format(f) {}

    /**
     * @brief PAN: Distributes mono input to multi-channel output based on X,Y,Z.
     * @param x: [-1.0, 1.0] (Left -> Right)
     * @param y: [-1.0, 1.0] (Front -> Back)
     * @param z: [0.0, 1.0] (Height)
     */
    void pan(float input, float* outputs, float x, float y, float z = 0.0f) {
        if (!outputs || !std::isfinite(input)) return;
        input = std::clamp(input, -4.0f, 4.0f);
        x = std::clamp(std::isfinite(x) ? x : 0.0f, -1.0f, 1.0f);
        y = std::clamp(std::isfinite(y) ? y : 0.0f, -1.0f, 1.0f);
        z = std::clamp(std::isfinite(z) ? z : 0.0f, 0.0f, 1.0f);
        switch (m_format) {
            case Format::Stereo:
                outputs[0] = input * std::sqrt(0.5f * (1.0f - x)); // L
                outputs[1] = input * std::sqrt(0.5f * (1.0f + x)); // R
                break;
            case Format::Surround_5_1:
                process51(input, outputs, x, y);
                break;
            case Format::Surround_7_1:
                process71(input, outputs, x, y);
                break;
            case Format::Immersive_7_1_4:
                process714(input, outputs, x, y, z);
                break;
        }
    }

private:
    void process51(float in, float* out, float x, float y) {
        // Simple Amplitude Panning across 5.1 layout
        // (Simplified logic for brevity)
        float fl = std::clamp((1.0f - x) * (1.0f + y), 0.0f, 1.0f);
        float fr = std::clamp((1.0f + x) * (1.0f + y), 0.0f, 1.0f);
        float c  = std::clamp(1.0f - std::abs(x), 0.0f, 1.0f) * std::max(0.0f, y);
        float bl = std::clamp((1.0f - x) * (1.0f - y), 0.0f, 1.0f);
        float br = std::clamp((1.0f + x) * (1.0f - y), 0.0f, 1.0f);
        
        out[0] = in * std::sqrt(fl); // L
        out[1] = in * std::sqrt(fr); // R
        out[2] = in * std::sqrt(c);  // Center
        out[3] = in * 0.1f;          // LFE (Sub leakage)
        out[4] = in * std::sqrt(bl); // Ls
        out[5] = in * std::sqrt(br); // Rs
    }

    static void normalizeAndApply(float in, float* out, const float* gains, size_t count) noexcept {
        float energy = 0.0f;
        for (size_t i = 0; i < count; ++i) energy += gains[i] * gains[i];
        const float scale = energy > 1.0e-12f ? in / std::sqrt(energy) : 0.0f;
        for (size_t i = 0; i < count; ++i) out[i] = std::isfinite(gains[i] * scale) ? gains[i] * scale : 0.0f;
    }

    void process71(float in, float* out, float x, float y) {
        // Channel order: L, R, C, LFE, Ls, Rs, Lb, Rb.
        const float front = std::sqrt(std::clamp((y + 1.0f) * 0.5f, 0.0f, 1.0f));
        const float rear = std::sqrt(std::clamp(1.0f - (y + 1.0f) * 0.5f, 0.0f, 1.0f));
        const float left = std::sqrt(std::clamp((1.0f - x) * 0.5f, 0.0f, 1.0f));
        const float right = std::sqrt(std::clamp((1.0f + x) * 0.5f, 0.0f, 1.0f));
        const float center = std::sqrt(std::clamp(1.0f - std::abs(x), 0.0f, 1.0f)) * front * 0.7f;
        const float gains[8] = {left * front, right * front, center, 0.0f,
                                left * rear, right * rear, left * rear * 0.7f, right * rear * 0.7f};
        normalizeAndApply(in, out, gains, 8);
    }

    void process714(float in, float* out, float x, float y, float z) {
        // Channel order: L, R, C, LFE, Ls, Rs, Lb, Rb, Ltf, Rtf, Ltr, Rtr.
        float floor[8]{};
        process71(1.0f, floor, x, y);
        const float ground = std::sqrt(std::clamp(1.0f - z, 0.0f, 1.0f));
        const float height = std::sqrt(std::clamp(z, 0.0f, 1.0f));
        float gains[12] = {
            floor[0] * ground, floor[1] * ground, floor[2] * ground, 0.0f,
            floor[4] * ground, floor[5] * ground, floor[6] * ground, floor[7] * ground,
            floor[0] * height, floor[1] * height, floor[4] * height, floor[5] * height
        };
        normalizeAndApply(in, out, gains, 12);
    }

    Format m_format;
};

} // namespace Aura::DSP::Spatial
