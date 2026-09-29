#pragma once

#include <vector>
#include <cmath>

namespace Hirari::DSP::Spatial {

/**
 * @brief AmbisonicEncoder: Professional 360-degree spherical spatialization.
 * Ready for VR, Games, and YouTube 360 (First-Order Ambisonics).
 */
class AmbisonicEncoder {
public:
    struct Position {
        float azimuth;   // [-PI, PI]
        float elevation; // [-PI/2, PI/2]
    };

    /**
     * @brief ENCODE: Mono input -> First-Order Ambisonics (W, X, Y, Z).
     * @param output: Buffer for 4 channels.
     */
    void encode(float input, float* output, Position pos) {
        float cosAz = std::cos(pos.azimuth);
        float sinAz = std::sin(pos.azimuth);
        float cosEl = std::cos(pos.elevation);
        float sinEl = std::sin(pos.elevation);

        // First-Order Ambisonics (SN3D normalization)
        output[0] = input * 0.7071f;      // W (Omni)
        output[1] = input * cosAz * cosEl; // X (Front/Back)
        output[2] = input * sinAz * cosEl; // Y (Left/Right)
        output[3] = input * sinEl;         // Z (Up/Down)
    }

    /**
     * @brief DECODE (Omni-Stereo): Simple decode for monitoring.
     */
    void decodeToStereo(const float* bFormat, float& l, float& r) {
        l = bFormat[0] + bFormat[2]; // W + Y
        r = bFormat[0] - bFormat[2]; // W - Y
    }
};

} // namespace Hirari::DSP::Spatial
