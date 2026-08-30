#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @brief AudioQuantizer: Snaps detected peaks to the musical grid.
 * Warps audio segments using linear interpolation.
 */
class AudioQuantizer {
public:
    struct Options {
        float strength = 1.0f; // [0, 1] 1.0 = perfect grid
        float swing = 0.0f;    // [0, 1]
    };

    /**
     * @brief Detects transient peaks and warps audio to snap to the grid.
     */
    static void quantize(const float* in, float* out, uint64_t len, float bpm, double sampleRate, Options opt) {
        if (len == 0 || bpm <= 0.0f || sampleRate <= 0.0) return;

        double stepSamples = (60.0 / bpm / 4.0) * sampleRate;
        if (stepSamples <= 10.0) return;

        // Copy input to handle in-place (in == out) safety
        std::vector<float> inCopy(in, in + len);

        // 1. Detect transient peaks in each grid step interval
        std::vector<double> transients;
        std::vector<double> targets;

        transients.push_back(0.0);
        targets.push_back(0.0);

        uint64_t numSteps = static_cast<uint64_t>(static_cast<double>(len) / stepSamples);
        for (uint64_t k = 1; k < numSteps; ++k) {
            double start = (k - 0.5) * stepSamples;
            double end = (k + 0.5) * stepSamples;
            
            // Find peak amplitude location in this range
            double peakPos = k * stepSamples;
            float maxAmp = 0.0f;
            uint64_t limitStart = std::min(len, static_cast<uint64_t>(std::max(0.0, start)));
            uint64_t limitEnd = std::min(len, static_cast<uint64_t>(end));
            
            for (uint64_t i = limitStart; i < limitEnd; ++i) {
                float absVal = std::abs(inCopy[i]);
                if (absVal > maxAmp) {
                    maxAmp = absVal;
                    peakPos = static_cast<double>(i);
                }
            }
            
            // Grid target position (with optional swing)
            double gridPos = k * stepSamples;
            if (k % 2 == 1) { // Apply swing to offbeats
                gridPos += opt.swing * 0.3 * stepSamples;
            }
            
            // Apply strength factor
            double targetPos = peakPos + opt.strength * (gridPos - peakPos);
            
            transients.push_back(peakPos);
            targets.push_back(targetPos);
        }

        transients.push_back(static_cast<double>(len));
        targets.push_back(static_cast<double>(len));

        // 2. Warp the segments using linear interpolation
        for (size_t k = 0; k < transients.size() - 1; ++k) {
            double t0 = transients[k];
            double t1 = transients[k+1];
            double g0 = targets[k];
            double g1 = targets[k+1];
            
            uint64_t outStart = std::min(len, static_cast<uint64_t>(std::max(0.0, g0)));
            uint64_t outEnd = std::min(len, static_cast<uint64_t>(std::max(0.0, g1)));
            
            double outDur = g1 - g0;
            double inDur = t1 - t0;
            
            for (uint64_t x = outStart; x < outEnd; ++x) {
                double u = 0.0;
                if (outDur > 0.0) {
                    u = (static_cast<double>(x) - g0) / outDur;
                }
                
                double inIdx = t0 + u * inDur;
                uint64_t idx0 = static_cast<uint64_t>(std::floor(inIdx));
                uint64_t idx1 = std::min(len - 1, idx0 + 1);
                float frac = static_cast<float>(inIdx - idx0);
                
                if (idx0 < len) {
                    out[x] = (1.0f - frac) * inCopy[idx0] + frac * inCopy[idx1];
                } else {
                    out[x] = 0.0f;
                }
            }
        }
    }
};

} // namespace Aura::Core::Engine
