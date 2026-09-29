#pragma once
#include <vector>
#include <array>
#include <string>
#include <algorithm>
#include <filesystem>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <memory>
#include <utility>
#include <mutex>
#include <thread>
#include <chrono>
#include "../../dsp/mixing/channel_strip.hpp"
#include "region_processor.hpp"
#include "../rust_ffi.hpp"
#include "../audio_buffer.hpp"
#include "../project_serializer.hpp"
#include "../io/audio_decoder.hpp"
#include "automation_recorder.hpp"
#include "../effect_chain.hpp"
#include "../midi_region.hpp"
#include "../plugin_host.hpp"
#include "../plugins/process_sandbox_processor.hpp"
#include "../plugins/plugin_cache_manager.hpp"
#include "../../core/dsp/spatial/holographic_panner.hpp"
#include "vca_manager.hpp"
#include "../editing/audio_note_segment.hpp"
#include "../editing/audio_pitch_analysis.hpp"
#include "../editing/event_processing_history.hpp"
#include "../../dsp/utils/dsp_utils.hpp"

namespace Hirari::Core::Engine {

// Immutable OLA window constructed before audio callbacks start. Keeping the
// cosine out of the sample loop makes the bounded WSOLA path cheaper per frame.
inline const std::array<float, 1024> kTrackWsolaWindow = [] {
    std::array<float, 1024> window{};
    for (size_t i = 0; i < window.size(); ++i) {
        window[i] = 0.5f * (1.0f - std::cos(
            6.28318530718f * static_cast<float>(i) /
            static_cast<float>(window.size() - 1)));
    }
    return window;
}();

// Shared invalidation for cheap project-layout polling. The realtime thread
// only stores a dirty bit when it queues write-automation points; the
// control-side reader folds that bit into the monotonic revision.
struct ProjectLayoutRevision {
    std::atomic<uint64_t> value{1};
    std::atomic<bool> realtimeAutomationDirty{false};

    void markChanged() noexcept {
        value.fetch_add(1, std::memory_order_release);
    }

    uint64_t current() noexcept {
        if (realtimeAutomationDirty.exchange(false, std::memory_order_acq_rel)) {
            markChanged();
        }
        return value.load(std::memory_order_acquire);
    }
};

struct Region {
    struct RangeEdit { uint64_t start = 0; uint64_t end = 0; float gain = 1.0f; uint64_t fadeIn = 0; uint64_t fadeOut = 0; };
    struct CompRange { uint64_t start = 0; uint64_t end = 0; uint64_t fadeIn = 0; uint64_t fadeOut = 0; };
    using WarpMarker = ::HirariWarpMarker;
    uint32_t id;
    std::string path;
    uint64_t start;
    uint64_t len;
    // Source-domain length; len is the timeline-domain duration.
    uint64_t sourceLength = 0;
    uint64_t sourceOffset = 0;
    uint64_t baseStart = 0;
    uint64_t baseSourceOffset = 0;
    uint64_t baseLength = 0;
    bool muted;
    std::string name;
    std::shared_ptr<AudioBuffer> audio;
    float clipGain = 1.0f;
    uint64_t fadeInSamples = 64;
    uint64_t fadeOutSamples = 64;
    bool reverse = false;
    double warpRatio = 1.0;
    // Decoded audio stays in its native frame domain. These rates map each
    // project-timeline frame to a source frame without changing the user's
    // independent varispeed/warp setting.
    double sourceSampleRate = 0.0;
    double timelineSampleRate = 0.0;
    double sourceFramesPerTimelineFrame() const noexcept {
        const double sourceRate = std::isfinite(sourceSampleRate) && sourceSampleRate > 0.0
            ? sourceSampleRate : timelineSampleRate;
        const double timelineRate = std::isfinite(timelineSampleRate) && timelineSampleRate > 0.0
            ? timelineSampleRate : sourceRate;
        const double conversion = sourceRate / timelineRate;
        const double effective = warpRatio * conversion;
        return std::isfinite(effective) && effective > 0.0 ? effective : warpRatio;
    }
    // When enabled, the spectral stretcher changes duration while preserving
    // pitch and applying the region's non-destructive pitch/formant curves.
    // Off preserves legacy varispeed behavior.
    bool pitchPreserveWarp = false;
    // Derived by Rust when a region snapshot is published, off the audio thread.
    bool needsSpectralStretch = false;
    std::shared_ptr<RegionTimeStretch> timeStretch;
    float pitchSemitones = 0.0f;
    // Non-destructive VariAudio-style note edits for this audio region.
    std::vector<::hirari::editing::AudioNoteSegment> audioNoteSegments;
    // Control-thread projection consumed by the real-time Rust segment lookup.
    std::vector<HirariAudioNoteSegmentRange> audioNoteSegmentRanges;
    // Immutable C ABI views consumed by the Rust WSOLA grain-pitch scheduler.
    std::vector<HirariAudioNoteCurveView> audioNoteCurveViews;
    uint32_t loopCount = 1;
    std::vector<RangeEdit> rangeEdits;
    bool compManaged = false;
    std::vector<CompRange> compRanges;
    bool locked = false;
    uint32_t syncGroup = 0;
    // Region-local, source-to-timeline anchors for non-destructive timing edits.
    std::vector<WarpMarker> warpMarkers;
    std::vector<::hirari::editing::EventProcessingStep> processingHistory;
    // Derived on the control thread when a region snapshot is published.
    uint64_t crossfadeInSamples = 0;
    uint64_t crossfadeOutSamples = 0;
};

/**
 * @class Track
 * @brief High-performance track orchestration engine.
 * HONEST FIX: Optimized region lookup using sorted-list binary search.
 */
class Track {
    #include "track_part_1a.inc"
    #include "track_part_1b.inc"
    #include "track_part_2.inc"
    #include "track_part_3a.inc"
    #include "track_part_3b.inc"
    #include "track_part_4.inc"

} // namespace Hirari::Core::Engine
