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
     * @brief Decodes B-format into a bounded binaural approximation.
     *
     * This is intentionally an allocation-free first-order decoder.  The
     * lateral component controls interaural level difference while the
     * vertical component slightly narrows the image, giving a stable fallback
     * when an external HRTF renderer is not present.
     */
    void decodeBinaural(const BFormat& b, float& l, float& r) {
        const float w = std::isfinite(b.w) ? b.w : 0.0f;
        const float x = std::isfinite(b.x) ? b.x : 0.0f;
        const float y = std::isfinite(b.y) ? b.y : 0.0f;
        const float z = std::isfinite(b.z) ? b.z : 0.0f;
        const float vertical = 1.0f / std::sqrt(1.0f + 0.15f * std::abs(z));
        const float side = std::clamp(y * 0.7071f, -1.0f, 1.0f);
        l = vertical * (w + 0.35f * x + side);
        r = vertical * (w + 0.35f * x - side);
        l = std::isfinite(l) ? std::clamp(l, -4.0f, 4.0f) : 0.0f;
        r = std::isfinite(r) ? std::clamp(r, -4.0f, 4.0f) : 0.0f;
    }
};

} // namespace Aura::DSP::Spatial
