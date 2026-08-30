#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <iostream>
#include "spectrum_analyzer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class NeuralMaskingKernel
 * @brief Cross-track Spectral Collision & Masking Detection.
 */
class NeuralMaskingKernel {
public:
    struct Collision {
        uint32_t binIndex;
        float intensity; // 0.0 to 1.0
        uint32_t trackA;
        uint32_t trackB;
    };

    /**
     * @brief Analyses spectral masking between two tracks.
     * Masking Threshold: -18dB relative overlap.
     */
    static std::vector<Collision> DetectCollisions(
        uint32_t idA, const std::vector<float>& bandsA,
        uint32_t idB, const std::vector<float>& bandsB) 
    {
        std::vector<Collision> collisions;
        size_t bins = std::min(bandsA.size(), bandsB.size());
        
        for (size_t i = 0; i < bins; ++i) {
            float a = bandsA[i];
            float b = bandsB[i];
            
            // Industrial Masking Logic:
            // If both are strong and within 18dB of each other, its a collision.
            float diff = std::abs(20.0f * std::log10(a + 1e-9f) - 20.0f * std::log10(b + 1e-9f));
            
            if (a > 0.05f && b > 0.05f && diff < 18.0f) {
                collisions.push_back({
                    (uint32_t)i,
                    (1.0f - (diff / 18.0f)) * std::max(a, b),
                    idA, idB
                });
            }
        }
        return collisions;
    }
};

} // namespace Aura::DSP::Analysis
