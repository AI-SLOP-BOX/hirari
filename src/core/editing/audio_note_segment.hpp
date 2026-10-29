#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <vector>
#include <type_traits>
#include "../rust_ffi.hpp"

namespace hirari::editing {

// Non-destructive edit data for a detected monophonic audio note.  The audio
// renderer can consume these anchors later; keeping them in the project model
// means pitch, formant and timing edits do not require rewriting source audio.
using AudioNoteAnchor = ::HirariAudioNoteAnchor;
static_assert(std::is_standard_layout_v<AudioNoteAnchor>);
static_assert(sizeof(AudioNoteAnchor) == 24);
static_assert(offsetof(AudioNoteAnchor, positionSeconds) == 0);
static_assert(offsetof(AudioNoteAnchor, pitchCents) == 8);
static_assert(offsetof(AudioNoteAnchor, formantCents) == 16);

struct AudioNoteSegment {
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double detectedPitchCents = 0.0;
    double pitchOffsetCents = 0.0;
    double formantOffsetCents = 0.0;
    std::vector<AudioNoteAnchor> anchors;
};

// ============================================================
// Rust-owned note-edit table.
//
// `AudioNoteSegment` above stays the C++ *transfer* type used by the engine
// API, serialization and split. The authoritative table lives in Rust
// (hirari-core-bridge/src/vari_audio.rs): C++ holds only the opaque store
// handle plus the immutable snapshot it publishes for the audio thread.
// Every mutator is an FFI call, so the edit policy (validation, overlap and
// dedup rules, anchor window surgery, sort order, phase prefixes) has exactly
// one implementation.
// ============================================================
class RegionNoteEdits {
public:
    RegionNoteEdits()
        : store_(hirari_region_vari_audio_store_new(),
                 [](HirariRegionVariAudioStore* s) {
                     hirari_region_vari_audio_store_clear(s);
                 }) {}

    RegionNoteEdits(const RegionNoteEdits&) = default;
    RegionNoteEdits& operator=(const RegionNoteEdits&) = default;
    RegionNoteEdits(RegionNoteEdits&&) noexcept = default;
    RegionNoteEdits& operator=(RegionNoteEdits&&) noexcept = default;

    bool valid() const noexcept { return static_cast<bool>(store_); }

    /// Borrowed handle for the Rust edit bridge; the table stays owned here.
    HirariRegionVariAudioStore* storeHandle() const noexcept { return store_.get(); }

    /// Rebuilds the immutable projection from the store. Control thread only;
    /// the audio thread reads `ranges()` / `curves()` without a lock.
    void publish() noexcept {
        snapshot_ = std::shared_ptr<HirariVariAudioFrozen>(
            store_ ? hirari_region_vari_audio_freeze(store_.get()) : nullptr,
            [](HirariVariAudioFrozen* f) { hirari_region_vari_audio_frozen_free(f); });
        ranges_ = snapshot_ ? hirari_region_vari_audio_frozen_ranges(snapshot_.get(), &rangeCount_)
                           : nullptr;
        curves_ = snapshot_ ? hirari_region_vari_audio_frozen_curves(snapshot_.get(), &curveCount_)
                            : nullptr;
        if (!snapshot_) {
            rangeCount_ = 0;
            curveCount_ = 0;
        }
    }

    /// Gives this region its own table. Required before mutating notes on a
    /// region that was copied out of the track list, because copies share the
    /// store handle until they detach.
    void detach() noexcept {
        auto owned = std::make_shared<RegionNoteEdits>();
        std::vector<AudioNoteSegment> copy;
        this->copyFromStore(copy);
        (void)owned->assignRepaired(copy);
        *this = std::move(*owned);
    }

    /// Copies the table into the C++ transfer type (engine API, serialization,
    /// split). Prefers the published snapshot, which is immutable, so the
    /// caller needs no lock and cannot observe a concurrent edit halfway
    /// through the copy.
    bool copy(std::vector<AudioNoteSegment>& out) const {
        if (!store_) return false;
        if (snapshot_ && curves_ && ranges_ && rangeCount_ == curveCount_) {
            out.clear();
            out.reserve(curveCount_);
            for (size_t index = 0; index < curveCount_; ++index) {
                const auto& curve = curves_[index];
                AudioNoteSegment segment{};
                segment.startSeconds = curve.startSeconds;
                segment.endSeconds = curve.endSeconds;
                segment.detectedPitchCents = ranges_[index].detectedPitchCents;
                segment.pitchOffsetCents = curve.pitchOffsetCents;
                segment.formantOffsetCents = curve.formantOffsetCents;
                segment.anchors.assign(curve.anchors, curve.anchors + curve.anchorCount);
                out.push_back(std::move(segment));
            }
            return true;
        }
        return copyFromStore(out);
    }

    /// Validated replace: one malformed entry or overlapping neighbour
    /// rejects the whole batch (editor API / undo restore).
    bool assignValidated(const std::vector<AudioNoteSegment>& segments) const {
        return replace(segments, hirari_region_vari_audio_store_set);
    }

    /// Repairing replace: malformed entries are dropped and the survivors are
    /// ordered (project load, region split, pitch analysis).
    bool assignRepaired(const std::vector<AudioNoteSegment>& segments) const {
        return replace(segments, hirari_region_vari_audio_store_load);
    }

    void clear() const {
        if (store_) (void)hirari_region_vari_audio_store_clear_segments(store_.get());
    }

    /// Region-add policy in place.
    bool normalize() const {
        return store_ && hirari_region_vari_audio_store_normalize(store_.get());
    }

    // ---- published projection (audio thread reads these) ----
    const HirariAudioNoteSegmentRange* ranges() const noexcept { return ranges_; }
    size_t rangeCount() const noexcept { return rangeCount_; }
    const HirariAudioNoteCurveView* curves() const noexcept { return curves_; }
    size_t curveCount() const noexcept { return curveCount_; }
    /// Stable identity of the published table, for render-cache invalidation.
    std::uintptr_t cacheKey() const noexcept {
        return reinterpret_cast<std::uintptr_t>(snapshot_.get());
    }
    /// Immutable published projection, borrowed for the duration of a call.
    /// Null until the region has been published at least once.
    const HirariVariAudioFrozen* frozenHandle() const noexcept { return snapshot_.get(); }

private:
    using ReplaceFn = bool (*)(HirariRegionVariAudioStore*,
                               const HirariAudioNoteSegment*, size_t,
                               const HirariAudioNoteAnchor*, const size_t*,
                               const size_t*);

    static HirariAudioNoteSegment toAbi(const AudioNoteSegment& segment) {
        return HirariAudioNoteSegment{segment.startSeconds, segment.endSeconds,
                                      segment.detectedPitchCents,
                                      segment.pitchOffsetCents,
                                      segment.formantOffsetCents};
    }

    /// Materializes straight from the authoritative table. Used when no
    /// snapshot has been published yet, and by `detach` where the live table is
    /// exactly what has to be duplicated.
    bool copyFromStore(std::vector<AudioNoteSegment>& out) const {
        out.clear();
        if (!store_) return false;
        size_t segmentCount = 0;
        size_t anchorTotal = 0;
        if (!hirari_region_vari_audio_store_sizes(store_.get(), &segmentCount,
                                                  &anchorTotal)) {
            return false;
        }
        if (segmentCount == 0) return true;
        std::vector<HirariAudioNoteSegment> values(segmentCount);
        std::vector<HirariAudioNoteAnchor> anchors(anchorTotal);
        std::vector<size_t> offsets(segmentCount);
        std::vector<size_t> counts(segmentCount);
        size_t writtenSegments = 0;
        size_t writtenAnchors = 0;
        if (!hirari_region_vari_audio_store_materialize(
                store_.get(), values.data(), values.size(), anchors.data(),
                anchors.size(), offsets.data(), counts.data(), &writtenSegments,
                &writtenAnchors)) {
            return false;
        }
        out.reserve(writtenSegments);
        for (size_t index = 0; index < writtenSegments; ++index) {
            AudioNoteSegment segment{};
            const auto& value = values[index];
            segment.startSeconds = value.start_seconds;
            segment.endSeconds = value.end_seconds;
            segment.detectedPitchCents = value.detected_pitch_cents;
            segment.pitchOffsetCents = value.pitch_offset_cents;
            segment.formantOffsetCents = value.formant_offset_cents;
            segment.anchors.assign(
                anchors.begin() + static_cast<std::ptrdiff_t>(offsets[index]),
                anchors.begin() + static_cast<std::ptrdiff_t>(offsets[index] +
                                                              counts[index]));
            out.push_back(std::move(segment));
        }
        return true;
    }

    bool replace(const std::vector<AudioNoteSegment>& segments, ReplaceFn apply) const {
        if (!store_) return false;
        if (segments.empty()) {
            clear();
            return true;
        }
        std::vector<HirariAudioNoteSegment> values;
        std::vector<HirariAudioNoteAnchor> anchors;
        std::vector<size_t> offsets;
        std::vector<size_t> counts;
        values.reserve(segments.size());
        offsets.reserve(segments.size());
        counts.reserve(segments.size());
        size_t anchorTotal = 0;
        for (const auto& segment : segments) {
            offsets.push_back(anchorTotal);
            counts.push_back(segment.anchors.size());
            anchorTotal += segment.anchors.size();
            values.push_back(toAbi(segment));
            anchors.insert(anchors.end(), segment.anchors.begin(),
                           segment.anchors.end());
        }
        return apply(store_.get(), values.data(), values.size(),
                     anchors.empty() ? nullptr : anchors.data(), offsets.data(),
                     counts.data());
    }

    std::shared_ptr<HirariRegionVariAudioStore> store_;
    std::shared_ptr<HirariVariAudioFrozen> snapshot_;
    const HirariAudioNoteSegmentRange* ranges_ = nullptr;
    const HirariAudioNoteCurveView* curves_ = nullptr;
    size_t rangeCount_ = 0;
    size_t curveCount_ = 0;
};

} // namespace hirari::editing
