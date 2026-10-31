#pragma once
#include <vector>
#include <array>
#include <string>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <memory>
#include <utility>
#include <mutex>
#include <thread>
#include <chrono>
#include "../rust_ffi.hpp"
#include "../audio_buffer.hpp"
#include "../project_automation.hpp"
#include "../effect_chain.hpp"
#include "../midi_region.hpp"
#include "../plugin_host.hpp"
#include "../plugins/process_sandbox_processor.hpp"
#include "../editing/audio_pitch_analysis.hpp"

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
    // The single mapping between Region storage and the Rust-owned edit state.
    // Split, duplicate, the host bridge and the mirror all go through these so
    // a new field cannot be added on one side only.
    ::HirariRegionState editableState() const noexcept {
        ::HirariRegionState state;
        state.start = start;
        state.len = len;
        state.baseStart = baseStart;
        state.baseLength = baseLength;
        state.sourceOffset = sourceOffset;
        state.baseSourceOffset = baseSourceOffset;
        state.sourceLength = sourceLength;
        state.fadeInSamples = fadeInSamples;
        state.fadeOutSamples = fadeOutSamples;
        state.warpRatio = warpRatio;
        state.sourceSampleRate = sourceSampleRate;
        state.timelineSampleRate = timelineSampleRate;
        state.clipGain = clipGain;
        state.pitchSemitones = pitchSemitones;
        state.loopCount = loopCount;
        state.syncGroup = syncGroup;
        state.muted = muted;
        state.reverse = reverse;
        state.locked = locked;
        state.pitchPreserveWarp = pitchPreserveWarp;
        return state;
    }
    void applyEditableState(const ::HirariRegionState& state) noexcept {
        start = state.start;
        len = state.len;
        baseStart = state.baseStart;
        baseLength = state.baseLength;
        sourceOffset = state.sourceOffset;
        baseSourceOffset = state.baseSourceOffset;
        sourceLength = state.sourceLength;
        fadeInSamples = state.fadeInSamples;
        fadeOutSamples = state.fadeOutSamples;
        warpRatio = state.warpRatio;
        sourceSampleRate = state.sourceSampleRate;
        timelineSampleRate = state.timelineSampleRate;
        clipGain = state.clipGain;
        pitchSemitones = state.pitchSemitones;
        loopCount = state.loopCount;
        syncGroup = state.syncGroup;
        muted = state.muted;
        reverse = state.reverse;
        locked = state.locked;
        pitchPreserveWarp = state.pitchPreserveWarp;
    }
    // When enabled, the spectral stretcher changes duration while preserving
    // pitch and applying the region's non-destructive pitch/formant curves.
    // Off preserves legacy varispeed behavior.
    bool pitchPreserveWarp = false;
    // Derived by Rust when a region snapshot is published, off the audio thread.
    bool needsSpectralStretch = false;
    // Rust owns the streaming stretcher; this shared handle is created with a
    // destroy deleter so copies of the Region keep one stretcher instance.
    std::shared_ptr<void> timeStretch;
    float pitchSemitones = 0.0f;
    // Non-destructive VariAudio-style note edits. Rust owns the authoritative
    // table; the region keeps the store handle plus the immutable projection
    // the audio callback reads without a lock.
    std::shared_ptr<HirariRegionVariAudioStore> noteStore{
        hirari_region_vari_audio_store_new(),
        [](HirariRegionVariAudioStore* store) { hirari_region_vari_audio_store_clear(store); }};
    std::shared_ptr<HirariVariAudioFrozen> noteSnapshot;
    const HirariAudioNoteSegmentRange* noteRanges = nullptr;
    const HirariAudioNoteCurveView* noteCurves = nullptr;
    std::size_t noteRangeCount = 0;
    std::size_t noteCurveCount = 0;
    // Cache identity of the published note projection.
    std::uintptr_t noteCacheKey = 0;
    void publishNoteEdits() noexcept {
        noteSnapshot.reset(
            noteStore ? hirari_region_vari_audio_freeze(noteStore.get()) : nullptr,
            [](HirariVariAudioFrozen* frozen) { hirari_region_vari_audio_frozen_free(frozen); });
        noteRanges = noteSnapshot
            ? hirari_region_vari_audio_frozen_ranges(noteSnapshot.get(), &noteRangeCount) : nullptr;
        noteCurves = noteSnapshot
            ? hirari_region_vari_audio_frozen_curves(noteSnapshot.get(), &noteCurveCount) : nullptr;
        noteCacheKey = reinterpret_cast<std::uintptr_t>(noteSnapshot.get());
        if (!noteSnapshot) {
            noteRangeCount = 0;
            noteCurveCount = 0;
        }
    }
    // Region-add gives the region its own table; the copy shares the caller's
    // store handle until this detach.
    void detachNoteEdits() {
        auto* copy = noteStore ? hirari_region_vari_audio_store_clone(noteStore.get()) : nullptr;
        if (!copy) return;
        noteStore.reset(copy, [](HirariRegionVariAudioStore* store) {
            hirari_region_vari_audio_store_clear(store);
        });
        noteSnapshot.reset();
        noteRanges = nullptr;
        noteCurves = nullptr;
        noteRangeCount = 0;
        noteCurveCount = 0;
        noteCacheKey = 0;
    }
    // Repairing replace (project load): the Rust store drops malformed entries
    // and orders the survivors.
    bool loadNoteSegments(const HirariAudioNoteSegment* segments, std::size_t segmentCount,
                          const HirariAudioNoteAnchor* anchors,
                          const std::size_t* anchorOffsets, const std::size_t* anchorCounts) {
        if (!noteStore || (segmentCount != 0 && !segments)) return false;
        if (segmentCount == 0) {
            (void)hirari_region_vari_audio_store_clear_segments(noteStore.get());
            return true;
        }
        return hirari_region_vari_audio_store_load(noteStore.get(), segments, segmentCount,
                                                   anchors, anchorOffsets, anchorCounts);
    }
    uint32_t loopCount = 1;
    std::vector<RangeEdit> rangeEdits;
    bool compManaged = false;
    std::vector<CompRange> compRanges;
    bool locked = false;
    uint32_t syncGroup = 0;
    // Region-local, source-to-timeline anchors for non-destructive timing edits.
    std::vector<WarpMarker> warpMarkers;
    // Non-destructive processing-step history. Rust owns the authoritative
    // table; C++ hands steps in and the layout serializer reads it directly.
    std::shared_ptr<HirariRegionProcessingHistoryStore> processingStore{
        hirari_region_processing_history_new(),
        [](HirariRegionProcessingHistoryStore* store) {
            hirari_region_processing_history_clear(store);
        }};
    // The store owns the admissibility policy (id, operation size, parameter
    // finiteness, 4096-step cap).
    bool appendProcessingStep(uint32_t stepId, const std::string& operation, float parameter,
                              bool enabled) {
        if (!processingStore || stepId == 0 || operation.empty()) return false;
        HirariEventStep step{};
        step.id = stepId;
        step.operation = reinterpret_cast<const uint8_t*>(operation.data());
        step.operation_len = operation.size();
        step.parameter = parameter;
        step.enabled = enabled;
        return hirari_region_processing_history_append(processingStore.get(), step);
    }
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
