#pragma once

#include "audio_note_segment.hpp"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <vector>

namespace aura::editing {

// Bounded, offline monophonic F0 analysis. This intentionally does not run in
// the audio callback; callers can schedule it on the analysis worker and then
// store the resulting non-destructive segments in an AudioRegion.
class AudioPitchAnalyzer {
public:
    struct Config {
        double minHz = 65.0;
        double maxHz = 1200.0;
        std::size_t window = 2048;
        std::size_t hop = 512;
        double threshold = 0.82;
    };

    static std::vector<AudioNoteSegment> analyze(const float* samples, std::size_t count,
                                                  double sampleRate, Config config) {
        std::vector<AudioNoteSegment> result;
        if (!samples || count == 0 || !std::isfinite(sampleRate) || sampleRate <= 0.0) return result;
        config.minHz = std::clamp(config.minHz, 20.0, 2000.0);
        config.maxHz = std::clamp(config.maxHz, config.minHz + 1.0, 4000.0);
        config.window = std::clamp<std::size_t>(config.window, 256, 8192);
        config.hop = std::clamp<std::size_t>(config.hop, 64, config.window);
        config.threshold = std::clamp(config.threshold, 0.5, 0.99);
        const std::size_t minLag = std::max<std::size_t>(1, static_cast<std::size_t>(sampleRate / config.maxHz));
        const std::size_t maxLag = std::min<std::size_t>(config.window - 1,
            static_cast<std::size_t>(sampleRate / config.minHz));
        bool active = false;
        AudioNoteSegment current{};
        // Analyze a final partial frame with zero padding. Short recordings
        // (including one-shot vocal takes) should still produce an editable
        // segment instead of being silently discarded.
        for (std::size_t offset = 0; offset < count; offset += config.hop) {
            const auto sampleAt = [&](std::size_t index) noexcept -> double {
                if (index >= count) return 0.0;
                const float value = samples[index];
                return std::isfinite(value) ? static_cast<double>(value) : 0.0;
            };
            double energy = 0.0;
            for (std::size_t i = 0; i < config.window; ++i) {
                const double s = sampleAt(offset + i);
                energy += s * s;
            }
            if (energy < 1e-8) { if (active) { result.push_back(current); active = false; } continue; }
            std::size_t bestLag = 0; double best = -1.0;
            for (std::size_t lag = minLag; lag <= maxLag; ++lag) {
                double corr = 0.0;
                for (std::size_t i = lag; i < config.window; ++i) {
                    const double a = sampleAt(offset + i);
                    const double b = sampleAt(offset + i - lag);
                    corr += a * b;
                }
                corr /= energy;
                if (corr > best) { best = corr; bestLag = lag; }
            }
            const bool voiced = bestLag > 0 && best >= config.threshold;
            if (!voiced) { if (active) { result.push_back(current); active = false; } continue; }
            // Parabolic interpolation around the correlation peak improves F0
            // resolution without requiring a larger analysis window.
            double refinedLag = static_cast<double>(bestLag);
            if (bestLag > minLag && bestLag < maxLag) {
                auto correlationAt = [&](std::size_t lag) {
                    double value = 0.0;
                    for (std::size_t i = lag; i < config.window; ++i) {
                        const double a = sampleAt(offset + i);
                        const double b = sampleAt(offset + i - lag);
                        value += a * b;
                    }
                    return value / energy;
                };
                const double ym = correlationAt(bestLag - 1);
                const double y0 = best;
                const double yp = correlationAt(bestLag + 1);
                const double denom = ym - 2.0 * y0 + yp;
                if (std::isfinite(denom) && std::abs(denom) > 1e-12) {
                    refinedLag += 0.5 * (ym - yp) / denom;
                }
            }
            refinedLag = std::clamp(refinedLag, static_cast<double>(minLag), static_cast<double>(maxLag));
            const double time = static_cast<double>(offset) / sampleRate;
            const double hz = sampleRate / refinedLag;
            const double cents = std::isfinite(hz) && hz > 0.0
                ? 1200.0 * std::log2(hz / 440.0) + 6900.0 : 0.0;
            if (!active) {
                current = AudioNoteSegment{}; current.startSeconds = time;
                current.endSeconds = std::min(static_cast<double>(count) / sampleRate,
                                              time + static_cast<double>(config.window) / sampleRate);
                current.detectedPitchCents = cents; active = true;
                current.anchors.push_back({time, 0.0, 0.0});
            } else {
                current.endSeconds = std::min(static_cast<double>(count) / sampleRate,
                                              time + static_cast<double>(config.window) / sampleRate);
                current.detectedPitchCents = 0.5 * (current.detectedPitchCents + cents);
                const double delta = cents - current.detectedPitchCents;
                current.anchors.push_back({time, std::clamp(delta, -2400.0, 2400.0), 0.0});
            }
        }
        if (active) result.push_back(current);
        result.erase(std::remove_if(result.begin(), result.end(), [](const auto& s) { return !s.valid(); }), result.end());
        return result;
    }

    static std::vector<AudioNoteSegment> analyze(const float* samples, std::size_t count,
                                                  double sampleRate) {
        return analyze(samples, count, sampleRate, Config{});
    }
};

} // namespace aura::editing
