#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <utility>
#include "../rust_ffi.hpp"

namespace hirari::editing {

// ============================================================
// Rust-owned note-edit table.
//
// The authoritative table lives in Rust (hirari-core-bridge/src/vari_audio.rs):
// C++ holds only the opaque store handle plus the immutable projection it
// publishes for the audio thread. Every mutator is an FFI call, so the edit
// policy (validation, overlap and dedup rules, anchor window surgery, sort
// order, phase prefixes) has exactly one implementation.
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

    /// Fresh deep copy for analysis workers; the caller owns and releases it
    /// with `hirari_region_vari_audio_store_clear`.
    HirariRegionVariAudioStore* cloneHandle() const noexcept {
        return store_ ? hirari_region_vari_audio_store_clone(store_.get()) : nullptr;
    }

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

    /// Gives this region its own table. Region-add normalizes a copy of the
    /// caller's region, whose store handle is still shared with them.
    void detach() noexcept {
        if (!store_) return;
        auto* copy = hirari_region_vari_audio_store_clone(store_.get());
        if (!copy) return;
        *this = RegionNoteEdits(std::shared_ptr<HirariRegionVariAudioStore>(
            copy, [](HirariRegionVariAudioStore* s) {
                hirari_region_vari_audio_store_clear(s);
            }));
    }

    /// Region-add policy in place.
    bool normalize() const {
        return store_ && hirari_region_vari_audio_store_normalize(store_.get());
    }

    /// Repairing replace (project load): malformed entries are dropped and the
    /// survivors are ordered. Callers pass the flat ABI layout.
    bool assignRepaired(const HirariAudioNoteSegment* segments, size_t segmentCount,
                        const HirariAudioNoteAnchor* anchors,
                        const size_t* anchorOffsets, const size_t* anchorCounts) const {
        if (!store_ || (segmentCount != 0 && !segments)) return false;
        if (segmentCount == 0) {
            (void)hirari_region_vari_audio_store_clear_segments(store_.get());
            return true;
        }
        return hirari_region_vari_audio_store_load(store_.get(), segments, segmentCount,
                                                   anchors, anchorOffsets, anchorCounts);
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
    explicit RegionNoteEdits(std::shared_ptr<HirariRegionVariAudioStore> store)
        : store_(std::move(store)) {}

    std::shared_ptr<HirariRegionVariAudioStore> store_;
    std::shared_ptr<HirariVariAudioFrozen> snapshot_;
    const HirariAudioNoteSegmentRange* ranges_ = nullptr;
    const HirariAudioNoteCurveView* curves_ = nullptr;
    size_t rangeCount_ = 0;
    size_t curveCount_ = 0;
};

} // namespace hirari::editing
