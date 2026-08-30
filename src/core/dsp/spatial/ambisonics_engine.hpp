#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Spatial {

/**
 * @struct BFormat
 * @brief First-Order Ambisonics (FOA) signals.
 */
struct BFormat {
    float w, x, y, z;
};

/**
 * @class AmbisonicsEngine
 * @brief First-Order Ambisonics encoder and decoder.
 * Provides the foundation for immersive spatial audio.
 */
class AmbisonicsEngine {
public:
    AmbisonicsEngine() = default;

    /**
     * @brief Encodes a mono signal into B-format.
     * @param azimuth: Angle in horizontal plane (radians).
     * @param elevation: Angle in vertical plane (radians).
     */
    BFormat encode(float input, float azimuth, float elevation) {
        BFormat b;
        float cosElev = std::cos(elevation);
        
        b.w = input * 0.7071f; // 1/sqrt(2)
        b.x = input * cosElev * std::cos(azimuth);
        b.y = input * cosElev * std::sin(azimuth);
        b.z = input * std::sin(elevation);
        
        return b;
    }

    /**
     * @brief Decodes B-format into Stereo.
     */
    void decodeStereo(const BFormat& b, float& l, float& r) {
        // Standard virtual cardioid decode at +/- 45 degrees
        l = b.w + (0.7071f * (b.x + b.y));
        r = b.w + (0.7071f * (b.x - b.y));
    }

    /**
     * @brief Decodes B-format into Binaural (Conceptual).
     * In a real implementation, this would use HRTF convolution for X, Y, Z channels.
     */
    void decodeBinaural(const BFormat& b, float& l, float& r) {
        // Simplified binaural approximation
        l = b.w + b.y;
        r = b.w - b.y;
    }
};

} // namespace Aura::DSP::Spatial
