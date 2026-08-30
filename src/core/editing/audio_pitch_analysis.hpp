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
        if (count < config.window) return result;

        const std::size_t minLag = std::max<std::size_t>(1, static_cast<std::size_t>(sampleRate / config.maxHz));
        const std::size_t maxLag = std::min<std::size_t>(config.window - 1,
            static_cast<std::size_t>(sampleRate / config.minHz));
        bool active = false;
        AudioNoteSegment current{};
        for (std::size_t offset = 0; offset + config.window <= count; offset += config.hop) {
            double energy = 0.0;
            for (std::size_t i = 0; i < config.window; ++i) energy += samples[offset + i] * samples[offset + i];
            if (energy < 1e-8) { if (active) { result.push_back(current); active = false; } continue; }
            std::size_t bestLag = 0; double best = -1.0;
            for (std::size_t lag = minLag; lag <= maxLag; ++lag) {
                double corr = 0.0;
                for (std::size_t i = lag; i < config.window; ++i) corr += samples[offset + i] * samples[offset + i - lag];
                corr /= energy;
                if (corr > best) { best = corr; bestLag = lag; }
            }
            const bool voiced = bestLag > 0 && best >= config.threshold;
            if (!voiced) { if (active) { result.push_back(current); active = false; } continue; }
            const double time = static_cast<double>(offset) / sampleRate;
            const double cents = 1200.0 * std::log2((sampleRate / static_cast<double>(bestLag)) / 440.0) + 6900.0;
            if (!active) {
                current = AudioNoteSegment{}; current.startSeconds = time;
                current.endSeconds = time + static_cast<double>(config.window) / sampleRate;
                current.detectedPitchCents = cents; active = true;
            } else {
                current.endSeconds = time + static_cast<double>(config.window) / sampleRate;
                current.detectedPitchCents = 0.5 * (current.detectedPitchCents + cents);
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
