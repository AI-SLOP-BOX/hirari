#pragma once

#include "audio_note_segment.hpp"
#include "../rust_ffi.hpp"

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <memory>
#include <vector>

namespace hirari::editing {

// Bounded, offline monophonic F0 analysis. The FFT, autocorrelation, note
// boundary tracking, and pitch-anchor generation run in the Rust core.
class AudioPitchAnalyzer {
public:
    static constexpr std::size_t kMaxSegments = 4096;

    struct Config {
        double minHz = 65.0;
        double maxHz = 1200.0;
        std::size_t window = 2048;
        std::size_t hop = 512;
        double threshold = 0.82;
        double noteChangeThresholdCents = 90.0;
        std::size_t noteChangeConfirmationFrames = 2;
    };

    static std::vector<AudioNoteSegment> analyze(
        const float* samples, std::size_t count, double sample_rate, Config config) {
        std::vector<AudioNoteSegment> output;
        if (!samples || count == 0 || count > 16'000'000
            || !std::isfinite(sample_rate) || sample_rate <= 0.0) {
            return output;
        }

        void* raw = hirari_audio_pitch_analyze(
            samples, count, sample_rate, config.minHz, config.maxHz,
            config.window, config.hop, config.threshold,
            config.noteChangeThresholdCents, config.noteChangeConfirmationFrames);
        if (!raw) return output;
        const auto release = [](void* state) { hirari_audio_pitch_result_destroy(state); };
        std::unique_ptr<void, decltype(release)> state(raw, release);

        const std::size_t segment_count = std::min(
            hirari_audio_pitch_segment_count(state.get()), kMaxSegments);
        output.reserve(segment_count);
        for (std::size_t index = 0; index < segment_count; ++index) {
            AudioNoteSegment segment{};
            if (!hirari_audio_pitch_get_segment(
                    state.get(), index, &segment.startSeconds,
                    &segment.endSeconds, &segment.detectedPitchCents)) {
                continue;
            }
            const std::size_t anchor_count = std::min<std::size_t>(
                hirari_audio_pitch_anchor_count(state.get(), index), 4096);
            segment.anchors.reserve(anchor_count);
            for (std::size_t anchor_index = 0; anchor_index < anchor_count; ++anchor_index) {
                AudioNoteAnchor anchor{};
                if (hirari_audio_pitch_get_anchor(
                        state.get(), index, anchor_index, &anchor.positionSeconds,
                        &anchor.pitchCents, &anchor.formantCents)) {
                    segment.anchors.push_back(anchor);
                }
            }
            if (segment.valid()) {
                segment.rebuildPitchRatioIntegral();
                output.push_back(std::move(segment));
            }
        }
        return output;
    }

    static std::vector<AudioNoteSegment> analyze(
        const float* samples, std::size_t count, double sample_rate) {
        return analyze(samples, count, sample_rate, Config{});
    }
};

} // namespace hirari::editing
