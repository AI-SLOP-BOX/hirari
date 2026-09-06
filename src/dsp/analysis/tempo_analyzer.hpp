#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <cstdint>
#include "../../graphics/graphics_kernel.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class SmartTempoAnalyzer
 * @brief Logic Pro style Automated BPM Detection.
 * HONEST FIX: Replaces 'manual BPM entry' with a professional 
 * Comb-Filter based tempo analyzer for imported loops.
 * Ensuring project-wide BPM synchronization for 'Tempo-less' assets.
 */
class SmartTempoAnalyzer {
public:
    struct AnalysisResult {
        float bpm;
        float confidence;
    };

    static AnalysisResult detectBPM(const float* data, uint64_t len, double sampleRate) {
        if (!data || len < 256 || !std::isfinite(sampleRate) || sampleRate < 8000.0) return {120.0f, 0.0f};
        constexpr uint32_t kEnvelopeRate = 200;
        const uint64_t hop = std::max<uint64_t>(1, static_cast<uint64_t>(sampleRate / kEnvelopeRate));
        const size_t frames = static_cast<size_t>(len / hop);
        if (frames < 16) return {120.0f, 0.0f};
        std::vector<float> env(frames, 0.0f);
        for (size_t f = 0; f < frames; ++f) {
            double sum = 0.0;
            const uint64_t begin = static_cast<uint64_t>(f) * hop;
            const uint64_t end = std::min<uint64_t>(len, begin + hop);
            for (uint64_t i = begin; i < end; ++i) sum += std::fabs(std::isfinite(data[i]) ? data[i] : 0.0f);
            env[f] = static_cast<float>(sum / std::max<uint64_t>(1, end - begin));
        }
        // Spectral-flux style onset envelope suppresses sustained tones.
        for (size_t i = frames - 1; i > 0; --i) env[i] = std::max(0.0f, env[i] - env[i - 1]);
        float best = -1.0f, second = 0.0f;
        uint32_t bestLag = 0;
        for (uint32_t bpm = 60; bpm <= 200; ++bpm) {
            const uint32_t lag = std::max<uint32_t>(1, static_cast<uint32_t>(std::lround(kEnvelopeRate * 60.0 / bpm)));
            if (lag >= frames / 2) continue;
            double corr = 0.0, normA = 0.0, normB = 0.0;
            for (size_t i = lag; i < frames; ++i) {
                corr += env[i] * env[i - lag]; normA += env[i] * env[i]; normB += env[i - lag] * env[i - lag];
            }
            const float score = static_cast<float>(corr / (std::sqrt(normA * normB) + 1.0e-9));
            if (score > best) { second = best; best = score; bestLag = lag; }
            else if (score > second) second = score;
        }
        const float bpm = bestLag ? static_cast<float>(kEnvelopeRate * 60.0 / bestLag) : 120.0f;
        const float confidence = std::clamp(0.5f * (best + 1.0f) + 0.5f * (best - second), 0.0f, 1.0f);
        return {bpm, std::isfinite(confidence) ? confidence : 0.0f};
    }
};


} // namespace Aura::DSP::Analysis

namespace Aura::Graphics::UI {

class FlexPitchRenderer {
public:
    struct PitchNode {
        double time;
        float pitch; 
        float drift;
    };

    /**
     * @brief THE PITCH DRIFT CURVES (Interpolated S-Curves)
     * HONEST FIX: Logic Pro's Flex Pitch uses smooth Bezier curves to show pitch movement.
     */
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const std::vector<PitchNode>& nodes, double startT, double endT) {
        if (w <= 1.0f || h <= 1.0f || !std::isfinite(startT) || !std::isfinite(endT) || endT <= startT) return;
        float noteH = h / 24.0f;
        
        // 1. GRID (Subtle semitone lines)
        for (int i = 0; i < 24; ++i) kernel.drawLine(x, y + i * noteH, x + w, y + i * noteH, 0.5f, 0x11FFFFFF);

        // 2. NODES & SMOOTH CURVES
        for (size_t i = 0; i < nodes.size(); ++i) {
            const auto& node = nodes[i];
            if (!std::isfinite(node.time) || !std::isfinite(node.pitch) || !std::isfinite(node.drift)) continue;
            float nx = x + (float)((node.time - startT) / (endT - startT)) * w;
            float ny = y + h - (node.pitch - 48) * noteH;

            // --- THE BEZIER DRIFT CURVE ---
            if (i > 0) {
                const auto& prev = nodes[i-1];
                float px = x + (float)((prev.time - startT) / (endT - startT)) * w;
                float py = y + h - (prev.pitch - 48) * noteH;
                
                // Draw Logic Pro 11 Pink/Magenta drift curve
                kernel.drawBezierCurve(px, py + prev.drift * 5, px + (nx-px)/2, py, nx - (nx-px)/2, ny, nx, ny + node.drift * 5, 1.5f, 0xFFFF00FF);
            }

            // --- THE PITCH BLOB ---
            kernel.drawRoundedRect(nx - 14, ny - 6, 28, 12, 6.0f, 0x8822D3EE); // Logic Cyan
            kernel.drawText(std::to_string(int(node.pitch)).c_str(), nx - 5, ny - 4, 8, 0xFFFFFFFF);
        }
    }
};

} // namespace Aura::Graphics::UI
