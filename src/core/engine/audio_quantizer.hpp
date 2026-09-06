#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <cmath>
#include <cstdint>

namespace Aura::Core::Engine {

/**
 * @brief AudioQuantizer: Snaps detected peaks to the musical grid.
 * Warps audio segments using linear interpolation.
 */
class AudioQuantizer {
public:
    static constexpr uint64_t kMaxWarpSamples = 16'384u;
    struct Options {
        float strength = 1.0f; // [0, 1] 1.0 = perfect grid
        float swing = 0.0f;    // [0, 1]
    };

    /**
     * @brief Detects transient peaks and warps audio to snap to the grid.
     */
    static void quantize(const float* in, float* out, uint64_t len, float bpm, double sampleRate, Options opt) {
        if (len == 0 || len > kMaxWarpSamples || !in || !out ||
            !std::isfinite(bpm) || bpm <= 0.0f || !std::isfinite(sampleRate) ||
            sampleRate <= 0.0 || !std::isfinite(opt.strength) ||
            !std::isfinite(opt.swing)) return;

        const float strength = std::clamp(opt.strength, 0.0f, 1.0f);
        const float swing = std::clamp(opt.swing, -1.0f, 1.0f);
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
                gridPos += static_cast<double>(swing) * 0.3 * stepSamples;
            }
            
            // Apply strength factor
            double targetPos = peakPos + static_cast<double>(strength) * (gridPos - peakPos);
            
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

    /**
     * Quantize a multi-microphone recording with one shared time map.  The
     * transient map is detected from the first channel and then applied to
     * every channel, which keeps inter-microphone phase relationships intact.
     * Inputs and outputs may alias on a per-channel basis, but channel buffers
     * themselves must not overlap.
     */
    static bool quantizeGroup(const float* const* inputs, float* const* outputs,
                              uint32_t channels, uint64_t len, float bpm,
                              double sampleRate, Options opt) {
        if (!inputs || !outputs || channels == 0 || channels > 32 || len == 0 ||
            len > kMaxWarpSamples ||
            bpm <= 0.0f || !std::isfinite(bpm) || sampleRate <= 0.0 ||
            !std::isfinite(sampleRate) || !std::isfinite(opt.strength) ||
            !std::isfinite(opt.swing)) return false;
        for (uint32_t c = 0; c < channels; ++c) {
            if (!inputs[c] || !outputs[c]) return false;
        }
        const double stepSamples = (60.0 / bpm / 4.0) * sampleRate;
        if (stepSamples <= 10.0) return false;

        std::vector<float> reference(inputs[0], inputs[0] + len);
        std::vector<double> transients{0.0};
        std::vector<double> targets{0.0};
        const uint64_t numSteps = static_cast<uint64_t>(static_cast<double>(len) / stepSamples);
        const float strength = std::clamp(opt.strength, 0.0f, 1.0f);
        const float swing = std::clamp(opt.swing, -1.0f, 1.0f);
        for (uint64_t k = 1; k < numSteps; ++k) {
            const double start = (k - 0.5) * stepSamples;
            const double end = (k + 0.5) * stepSamples;
            const uint64_t first = std::min(len, static_cast<uint64_t>(std::max(0.0, start)));
            const uint64_t last = std::min(len, static_cast<uint64_t>(std::max(0.0, end)));
            double peak = k * stepSamples;
            float amplitude = 0.0f;
            for (uint64_t i = first; i < last; ++i) {
                const float value = std::isfinite(reference[i]) ? std::abs(reference[i]) : 0.0f;
                if (value > amplitude) { amplitude = value; peak = static_cast<double>(i); }
            }
            double grid = k * stepSamples;
            if (k & 1u) grid += static_cast<double>(swing) * 0.3 * stepSamples;
            transients.push_back(peak);
            targets.push_back(peak + static_cast<double>(strength) * (grid - peak));
        }
        transients.push_back(static_cast<double>(len));
        targets.push_back(static_cast<double>(len));

        for (uint32_t c = 0; c < channels; ++c) {
            std::vector<float> copy(inputs[c], inputs[c] + len);
            std::fill(outputs[c], outputs[c] + len, 0.0f);
            for (size_t segment = 0; segment + 1 < transients.size(); ++segment) {
                const double t0 = transients[segment], t1 = transients[segment + 1];
                const double g0 = targets[segment], g1 = targets[segment + 1];
                const uint64_t outStart = std::min(len, static_cast<uint64_t>(std::max(0.0, g0)));
                const uint64_t outEnd = std::min(len, static_cast<uint64_t>(std::max(0.0, g1)));
                const double outDuration = g1 - g0, inDuration = t1 - t0;
                for (uint64_t x = outStart; x < outEnd; ++x) {
                    const double u = outDuration > 0.0 ? (static_cast<double>(x) - g0) / outDuration : 0.0;
                    const double source = std::clamp(t0 + u * inDuration, 0.0, static_cast<double>(len - 1));
                    const auto i0 = static_cast<uint64_t>(source);
                    const auto i1 = std::min<uint64_t>(len - 1, i0 + 1);
                    const float frac = static_cast<float>(source - static_cast<double>(i0));
                    const float a = std::isfinite(copy[i0]) ? copy[i0] : 0.0f;
                    const float b = std::isfinite(copy[i1]) ? copy[i1] : 0.0f;
                    outputs[c][x] = a + (b - a) * frac;
                }
            }
        }
        return true;
    }
};

} // namespace Aura::Core::Engine
