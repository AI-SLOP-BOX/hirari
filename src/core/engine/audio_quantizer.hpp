#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <numbers>
#include <utility>
#include <functional>
#include <limits>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct AudioQuantizerTimeConverters {
    const std::function<double(uint64_t)>* samplesToBeats;
    const std::function<uint64_t(double)>* beatsToSamples;
    bool failed = false;
};

extern "C" inline double audioQuantizerSamplesToBeats(void* context, uint64_t samples) {
    auto& converters = *static_cast<AudioQuantizerTimeConverters*>(context);
    try { return (*converters.samplesToBeats)(samples); }
    catch (...) {
        converters.failed = true;
        return std::numeric_limits<double>::quiet_NaN();
    }
}

extern "C" inline uint64_t audioQuantizerBeatsToSamples(void* context, double beats) {
    auto& converters = *static_cast<AudioQuantizerTimeConverters*>(context);
    try { return (*converters.beatsToSamples)(beats); }
    catch (...) {
        converters.failed = true;
        return 0;
    }
}

/**
 * @brief AudioQuantizer: Snaps detected peaks to the musical grid.
 * Warps audio segments using linear interpolation.
 */
class AudioQuantizer {
public:
    // Offline buffer quantization is bounded to keep its per-channel source
    // copy finite. This supports roughly 5.8 minutes at 48 kHz, instead of
    // only a fraction of a second, while leaving long-session warp maps to the
    // region-level non-destructive path.
    static constexpr uint64_t kMaxWarpSamples = 16'777'216u;
    struct Options {
        float strength = 1.0f; // [0, 1] 1.0 = perfect grid
        float swing = 0.0f;    // [-1, 1], applied to alternating grid points
    };

    // Build source->timeline anchors without touching the source buffers. The
    // same map can then be attached non-destructively to every region in a
    // phase-aligned edit group.
    static bool buildGroupWarpMap(
        const float* const* inputs, uint32_t channels, uint64_t len,
        uint64_t timelineStart, double gridBeats, Options opt,
        const std::function<double(uint64_t)>& samplesToBeats,
        const std::function<uint64_t(double)>& beatsToSamples,
        std::vector<std::pair<uint64_t, uint64_t>>& markers) {
        markers.clear();
        if (!inputs || !samplesToBeats || !beatsToSamples) return false;
        AudioQuantizerTimeConverters converters{&samplesToBeats, &beatsToSamples};
        void* mapState = hirari_audio_quantizer_build_group_map(
            inputs, channels, len, timelineStart, gridBeats,
            opt.strength, opt.swing, &converters,
            &audioQuantizerSamplesToBeats, &audioQuantizerBeatsToSamples);
        if (!mapState) return false;
        std::unique_ptr<void, decltype(&hirari_audio_quantizer_map_destroy)>
            mapGuard(mapState, &hirari_audio_quantizer_map_destroy);
        if (converters.failed) return false;
        const size_t count = hirari_audio_quantizer_map_count(mapState);
        markers.reserve(count);
        for (size_t i = 0; i < count; ++i) {
            uint64_t source = 0, timeline = 0;
            if (!hirari_audio_quantizer_map_get(mapState, i, &source, &timeline)) {
                markers.clear();
                return false;
            }
            markers.emplace_back(source, timeline);
        }
        if (converters.failed) {
            markers.clear();
            return false;
        }
        return !markers.empty();
    }

    /** Detect transients and warp one offline audio buffer in Rust. */
    static void quantize(const float* in, float* out, uint64_t len,
                         float bpm, double sampleRate, Options opt) {
        if (!in || !out || len == 0 || len > kMaxWarpSamples) return;
        (void)hirari_audio_quantize(in, out, len, bpm, sampleRate,
                                    opt.strength, opt.swing);
    }

    /**
     * Quantize a multi-microphone recording with one shared time map. The
     * transient detector combines per-channel RMS-normalized energy, then
     * applies the same map to every channel to retain their phase relationship.
     * Inputs and outputs may alias on a per-channel basis, but channel buffers
     * themselves must not overlap.
     */
    static bool quantizeGroup(const float* const* inputs, float* const* outputs,
                              uint32_t channels, uint64_t len, float bpm,
                              double sampleRate, Options opt) {
        return hirari_audio_quantize_group(inputs, outputs, channels, len, bpm,
                                           sampleRate, opt.strength, opt.swing);
    }
};

} // namespace Hirari::Core::Engine
