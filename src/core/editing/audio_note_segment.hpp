#pragma once

#include <algorithm>
#include <cstddef>
#include <cmath>
#include <cstdint>
#include <limits>
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
    // Rebuilt on the control side; deliberately omitted from project files.
    std::vector<double> pitchRatioIntegralPrefix;
    double pitchRatioCorrectionBeforeSeconds = 0.0;

    void evaluateAt(double seconds, double& pitch, double& formant) const noexcept {
        double output[2]{};
        hirari_audio_note_curve_at(anchors.data(), anchors.size(), seconds,
                                   pitchOffsetCents, formantOffsetCents, output);
        pitch = output[0];
        formant = output[1];
    }

    double pitchAt(double seconds) const noexcept {
        double pitch = 0.0, formant = 0.0;
        evaluateAt(seconds, pitch, formant);
        return pitch;
    }

    double formantAt(double seconds) const noexcept {
        double pitch = 0.0, formant = 0.0;
        evaluateAt(seconds, pitch, formant);
        return formant;
    }

    bool valid() const noexcept {
        return std::isfinite(startSeconds) && std::isfinite(endSeconds) &&
               endSeconds > startSeconds && std::isfinite(detectedPitchCents) &&
               std::isfinite(pitchOffsetCents) && std::isfinite(formantOffsetCents);
    }

    void setTiming(double start, double end) {
        if (std::isfinite(start) && std::isfinite(end) && end > start) {
            startSeconds = start;
            endSeconds = end;
            rebuildPitchRatioIntegral();
        }
    }

    // Move and scale the segment without touching source audio. Anchor
    // positions follow the same affine transform, preserving the edited curve.
    bool warpTiming(double newStart, double newEnd) {
        if (!std::isfinite(newStart) || !std::isfinite(newEnd) || newEnd <= newStart || !valid()) return false;
        if (!hirari_audio_note_curve_warp_anchors(
                anchors.data(), anchors.size(), startSeconds, endSeconds, newStart, newEnd)) return false;
        startSeconds = newStart;
        endSeconds = newEnd;
        rebuildPitchRatioIntegral();
        return true;
    }

    void setPitchOffset(double cents) {
        if (std::isfinite(cents)) {
            pitchOffsetCents = std::clamp(cents, -4800.0, 4800.0);
            rebuildPitchRatioIntegral();
        }
    }

    void setFormantOffset(double cents) noexcept {
        if (std::isfinite(cents)) formantOffsetCents = std::clamp(cents, -2400.0, 2400.0);
    }

    void upsertAnchor(AudioNoteAnchor anchor) {
        if (!std::isfinite(anchor.positionSeconds) ||
            !std::isfinite(anchor.pitchCents) || !std::isfinite(anchor.formantCents) ||
            anchor.positionSeconds < startSeconds || anchor.positionSeconds > endSeconds) return;
        std::vector<AudioNoteAnchor> normalized(anchors.size() + 1);
        std::copy(anchors.begin(), anchors.end(), normalized.begin());
        const size_t count = hirari_audio_note_curve_upsert_anchor(
            normalized.data(), anchors.size(), normalized.size(), startSeconds, endSeconds,
            anchor.positionSeconds, anchor.pitchCents, anchor.formantCents);
        if (count == std::numeric_limits<size_t>::max()) return;
        normalized.resize(count);
        anchors = std::move(normalized);
        rebuildPitchRatioIntegral();
    }

    void rebuildPitchRatioIntegral() {
        pitchRatioIntegralPrefix.assign(anchors.size(), 0.0);
        hirari_audio_note_curve_build_integral_prefix(
            anchors.data(), anchors.size(), pitchOffsetCents,
            pitchRatioIntegralPrefix.data());
    }

    // Integrates the pitch ratio in seconds from segment start to `seconds`.
    // The exact exponential integral over linear-in-cents anchors keeps the
    // pitch shifter's phase trajectory continuous through edited glides.
    double pitchRatioIntegralAt(double seconds) const noexcept {
        if (pitchRatioIntegralPrefix.size() != anchors.size()) return 0.0;
        return hirari_audio_note_curve_integral_at(
            anchors.data(), anchors.size(), pitchRatioIntegralPrefix.data(),
            startSeconds, endSeconds, pitchOffsetCents, seconds);
    }

private:
};

inline std::vector<::HirariAudioNoteSegmentRange> audioNoteSegmentRanges(
    const std::vector<AudioNoteSegment>& segments) {
    std::vector<::HirariAudioNoteSegmentRange> ranges;
    ranges.reserve(segments.size());
    for (const auto& segment : segments) {
        ranges.push_back({segment.startSeconds, segment.endSeconds,
                          segment.detectedPitchCents});
    }
    return ranges;
}

// Compute the accumulated pitch-ratio phase offset before each non-overlapping
// note segment. This lets the random-access audio callback restart in a gap or
// at a later segment without replaying every earlier sample.
inline void rebuildAudioNotePhasePrefixes(std::vector<AudioNoteSegment>& segments) {
    std::vector<::HirariAudioNotePhaseView> views;
    views.reserve(segments.size());
    for (const auto& segment : segments) {
        views.push_back({segment.startSeconds, segment.endSeconds,
            segment.pitchOffsetCents, segment.anchors.data(), segment.anchors.size(),
            segment.pitchRatioIntegralPrefix.data()});
    }
    std::vector<uint32_t> orderedIndices(segments.size());
    std::vector<double> corrections(segments.size());
    if (!hirari_audio_note_rebuild_phase_prefixes(
            views.data(), views.size(), orderedIndices.data(), corrections.data())) return;
    std::vector<AudioNoteSegment> ordered;
    ordered.reserve(segments.size());
    for (uint32_t index : orderedIndices) ordered.push_back(std::move(segments[index]));
    for (size_t index = 0; index < ordered.size(); ++index) {
        ordered[index].pitchRatioCorrectionBeforeSeconds = corrections[index];
    }
    segments = std::move(ordered);
}

} // namespace hirari::editing
