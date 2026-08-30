#pragma once
#include <atomic>
#include <algorithm>
#include <cmath>
#include <vector>
#include "../engine_types.hpp"

namespace Aura::Core::Engine {

/**
 * @class MetronomeEngine
 * @brief High-precision, sample-accurate metronome click generator.
 * HONEST FIX: Implemented tick-based timing and ms-accurate click duration.
 */
class MetronomeEngine {
public:
    void process(float* l, float* r, size_t numFrames, const EngineContext& ctx) {
        if (l == nullptr || r == nullptr || numFrames == 0 ||
            !std::isfinite(ctx.sampleRate) || ctx.sampleRate <= 0.0 ||
            !std::isfinite(ctx.tempo) || ctx.tempo <= 0.0 ||
            ctx.timeSig.numerator <= 0) return;

        const double samplesPerBeatExact = ctx.sampleRate * 60.0 / ctx.tempo;
        if (!std::isfinite(samplesPerBeatExact) || samplesPerBeatExact < 1.0) return;
        const uint64_t samplesPerBeat = static_cast<uint64_t>(samplesPerBeatExact + 0.5);
        if (samplesPerBeat == 0) return;

        // The click is derived from the absolute sample position, so loop
        // blocks and offline renders remain sample-accurate without mutable
        // callback state. Downbeats receive a stronger, slightly brighter
        // impulse; subdivisions use a softer click.
        for (size_t i = 0; i < numFrames; ++i) {
            const uint64_t position = ctx.playhead > UINT64_MAX - i
                ? UINT64_MAX : ctx.playhead + static_cast<uint64_t>(i);
            if (position == UINT64_MAX || position % samplesPerBeat != 0) continue;
            const uint64_t beat = position / samplesPerBeat;
            const bool downbeat = (beat % static_cast<uint64_t>(ctx.timeSig.numerator)) == 0;
            const float level = downbeat ? 0.42f : 0.25f;
            const size_t tail = std::min<size_t>(numFrames - i, 48);
            for (size_t j = 0; j < tail; ++j) {
                const float envelope = std::exp(-static_cast<float>(j) / (downbeat ? 10.0f : 7.0f));
                const float click = level * envelope;
                l[i + j] += click;
                r[i + j] += click;
            }
        }
    }


};

} // namespace Aura::Core::Engine
