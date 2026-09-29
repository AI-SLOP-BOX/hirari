#pragma once

#include <vector>
#include <cmath>
#include <array>
#include "../../core/audio_buffer.hpp"

namespace Hirari::DSP::Spatial {

/**
 * @brief AtmosObjectPanner: 7.1.4 Object-Based Spatializer.
 * Places any mono sound source into a coordinate-based 3D immersive space.
 */
class AtmosObjectPanner {
public:
    struct Vec3 { float x, y, z; }; // x: -1(L) to 1(R), y: -1(B) to 1(F), z: 0(FL) to 1(TOP)

    AtmosObjectPanner() {
        // Speaker Positions in normalized space [x, y, z]
        m_speakers = {{
            {-1,  1, 0}, {1,  1, 0}, {0,  1, 0}, {0, 0, -1}, // L, R, C, LFE (approx)
            {-1,  0, 0}, {1,  0, 0},                         // Ls, Rs
            {-1, -1, 0}, {1, -1, 0},                         // Lb, Rb
            {-1,  1, 1}, {1,  1, 1},                         // Ltf, Rtf
            {-1, -1, 1}, {1, -1, 1}                          // Ltr, Rtr
        }};
    }

    /**
     * @brief Spatially renders a mono signal into the Atmos buffer.
     */
    void process(float input, float* outputsAtmos, Vec3 pos) {
        for (size_t i = 0; i < 12; ++i) {
            float dx = pos.x - m_speakers[i].x;
            float dy = pos.y - m_speakers[i].y;
            float dz = pos.z - m_speakers[i].z;
            float dist = std::sqrt(dx*dx + dy*dy + dz*dz + 0.1f);
            
            float gain = 1.0f / (dist * dist); // Inverse Square Law
            outputsAtmos[i] = input * std::clamp(gain, 0.0f, 1.0f);
        }
    }

private:
    std::array<Vec3, 12> m_speakers;
};

} // namespace Hirari::DSP::Spatial
