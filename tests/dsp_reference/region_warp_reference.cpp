#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

struct HirariWarpMarkerReference {
    uint64_t source_sample;
    uint64_t timeline_sample;
    uint8_t transient;
};

struct HirariAudioNoteAnchorReference {
    double position_seconds;
    double pitch_cents;
    double formant_cents;
};

struct HirariAudioNoteCurveViewReference {
    const HirariAudioNoteAnchorReference* anchors;
    size_t anchor_count;
    const double* integral_prefix;
    double start_seconds;
    double end_seconds;
    double pitch_offset_cents;
    double correction_before_seconds;
    double formant_offset_cents;
};

extern "C" bool hirari_region_needs_spectral_stretch_reference(
    double source_rate, size_t warp_marker_count, float pitch_semitones,
    const HirariAudioNoteCurveViewReference* note_curves, size_t note_curve_count) {
    bool needed = std::abs(source_rate - 1.0) > 1.0e-5 ||
        warp_marker_count != 0 || std::abs(pitch_semitones) > 1.0e-4f;
    for (size_t segment = 0; segment < note_curve_count; ++segment) {
        const auto& curve = note_curves[segment];
        if (std::abs(curve.pitch_offset_cents) > 1.0e-3 ||
            std::abs(curve.formant_offset_cents) > 1.0e-3) {
            return true;
        }
        for (size_t anchor = 0; anchor < curve.anchor_count; ++anchor) {
            if (std::abs(curve.anchors[anchor].pitch_cents) > 1.0e-3 ||
                std::abs(curve.anchors[anchor].formant_cents) > 1.0e-3) {
                return true;
            }
        }
    }
    return needed;
}

extern "C" double hirari_region_source_position_reference(
    const HirariWarpMarkerReference* markers, size_t marker_count,
    uint64_t timeline_sample, double source_rate, uint64_t source_span) {
    if (marker_count < 2) {
        return std::clamp(static_cast<double>(timeline_sample) * source_rate,
                          0.0, static_cast<double>(source_span));
    }
    const auto* upper = std::upper_bound(
        markers, markers + marker_count, timeline_sample,
        [](uint64_t sample, const HirariWarpMarkerReference& marker) {
            return sample < marker.timeline_sample;
        });
    const HirariWarpMarkerReference* left = nullptr;
    const HirariWarpMarkerReference* right = nullptr;
    if (upper == markers) {
        left = &markers[0];
        right = &markers[1];
    } else if (upper == markers + marker_count) {
        left = &markers[marker_count - 2];
        right = &markers[marker_count - 1];
    } else {
        left = upper - 1;
        right = upper;
    }
    const uint64_t timeline_delta = right->timeline_sample - left->timeline_sample;
    if (timeline_delta == 0) return static_cast<double>(left->source_sample);
    const double source_per_timeline =
        static_cast<double>(right->source_sample - left->source_sample) /
        static_cast<double>(timeline_delta);
    const double mapped = static_cast<double>(left->source_sample) +
        (static_cast<double>(timeline_sample) -
         static_cast<double>(left->timeline_sample)) * source_per_timeline;
    return std::clamp(mapped, 0.0, static_cast<double>(source_span));
}
