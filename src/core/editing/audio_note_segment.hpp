#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>

namespace aura::editing {

// Non-destructive edit data for a detected monophonic audio note.  The audio
// renderer can consume these anchors later; keeping them in the project model
// means pitch, formant and timing edits do not require rewriting source audio.
struct AudioNoteAnchor {
    double positionSeconds = 0.0;
    double pitchCents = 0.0;
    double formantCents = 0.0;
};

struct AudioNoteSegment {
    double startSeconds = 0.0;
    double endSeconds = 0.0;
    double detectedPitchCents = 0.0;
    double pitchOffsetCents = 0.0;
    double formantOffsetCents = 0.0;
    std::vector<AudioNoteAnchor> anchors;

    double pitchAt(double seconds) const noexcept {
        return interpolateAt(seconds, false);
    }

    double formantAt(double seconds) const noexcept {
        return interpolateAt(seconds, true);
    }

    bool valid() const noexcept {
        return std::isfinite(startSeconds) && std::isfinite(endSeconds) &&
               endSeconds > startSeconds && std::isfinite(detectedPitchCents) &&
               std::isfinite(pitchOffsetCents) && std::isfinite(formantOffsetCents);
    }

    void setTiming(double start, double end) noexcept {
        if (std::isfinite(start) && std::isfinite(end) && end > start) {
            startSeconds = start;
            endSeconds = end;
        }
    }

    // Move and scale the segment without touching source audio. Anchor
    // positions follow the same affine transform, preserving the edited curve.
    bool warpTiming(double newStart, double newEnd) noexcept {
        if (!std::isfinite(newStart) || !std::isfinite(newEnd) || newEnd <= newStart || !valid()) return false;
        const double oldSpan = endSeconds - startSeconds;
        const double scale = (newEnd - newStart) / oldSpan;
        for (auto& anchor : anchors)
            anchor.positionSeconds = newStart + (anchor.positionSeconds - startSeconds) * scale;
        startSeconds = newStart;
        endSeconds = newEnd;
        return true;
    }

    void setPitchOffset(double cents) noexcept {
        if (std::isfinite(cents)) pitchOffsetCents = std::clamp(cents, -4800.0, 4800.0);
    }

    void setFormantOffset(double cents) noexcept {
        if (std::isfinite(cents)) formantOffsetCents = std::clamp(cents, -2400.0, 2400.0);
    }

    void upsertAnchor(AudioNoteAnchor anchor) {
        if (!std::isfinite(anchor.positionSeconds) ||
            !std::isfinite(anchor.pitchCents) || !std::isfinite(anchor.formantCents) ||
            anchor.positionSeconds < startSeconds || anchor.positionSeconds > endSeconds) return;
        auto it = std::lower_bound(anchors.begin(), anchors.end(), anchor.positionSeconds,
            [](const AudioNoteAnchor& a, double p) { return a.positionSeconds < p; });
        if (it != anchors.end() && std::abs(it->positionSeconds - anchor.positionSeconds) < 1e-9)
            *it = anchor;
        else
            anchors.insert(it, anchor);
    }

private:
    double interpolateAt(double seconds, bool formant) const noexcept {
        if (!std::isfinite(seconds)) return 0.0;
        const double base = formant ? formantOffsetCents : pitchOffsetCents;
        if (anchors.empty()) return base;
        if (seconds <= anchors.front().positionSeconds)
            return base + (formant ? anchors.front().formantCents : anchors.front().pitchCents);
        if (seconds >= anchors.back().positionSeconds)
            return base + (formant ? anchors.back().formantCents : anchors.back().pitchCents);
        auto upper = std::upper_bound(anchors.begin(), anchors.end(), seconds,
            [](double p, const AudioNoteAnchor& a) { return p < a.positionSeconds; });
        const auto& right = *upper;
        const auto& left = *(upper - 1);
        const double span = right.positionSeconds - left.positionSeconds;
        const double t = span > 0.0 ? (seconds - left.positionSeconds) / span : 0.0;
        const double l = formant ? left.formantCents : left.pitchCents;
        const double r = formant ? right.formantCents : right.pitchCents;
        return base + l + (r - l) * std::clamp(t, 0.0, 1.0);
    }
};

} // namespace aura::editing
