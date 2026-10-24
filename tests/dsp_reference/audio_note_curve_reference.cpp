#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include "editing/audio_note_segment.hpp"

struct AudioNoteAnchorReference {
    double position_seconds;
    double pitch_cents;
    double formant_cents;
};
struct AudioNoteSegmentRangeReference {
    double start_seconds;
    double end_seconds;
};
struct AudioNoteCurveViewReference {
    const AudioNoteAnchorReference* anchors;
    size_t anchor_count;
    const double* prefix;
    double start_seconds;
    double end_seconds;
    double pitch_offset_cents;
    double correction_before_seconds;
};

namespace {
double ratio_for_cents(double cents) {
    return std::exp2(std::clamp(cents, -4800.0, 4800.0) / 1200.0);
}

double integrate_linear_pitch(double left, double right, double seconds, double offset) {
    if (!std::isfinite(seconds) || seconds <= 0.0) return 0.0;
    left = std::clamp(offset + left, -4800.0, 4800.0);
    right = std::clamp(offset + right, -4800.0, 4800.0);
    const double delta = right - left;
    constexpr double cents_to_exponent = 0.0005776226504666211;
    if (std::abs(delta) < 1.0e-8) return seconds * ratio_for_cents(left);
    const double exponent = cents_to_exponent * delta;
    const double integral = seconds * ratio_for_cents(left) * std::expm1(exponent) / exponent;
    return std::isfinite(integral) ? integral : seconds * ratio_for_cents(left);
}

double interpolate(const AudioNoteAnchorReference* anchors, size_t count,
                   double seconds, double base, bool formant) {
    if (!std::isfinite(seconds)) return 0.0;
    if (count == 0) return base;
    const auto value = [formant](const AudioNoteAnchorReference& anchor) {
        return formant ? anchor.formant_cents : anchor.pitch_cents;
    };
    if (seconds <= anchors[0].position_seconds) return base + value(anchors[0]);
    if (seconds >= anchors[count - 1].position_seconds) return base + value(anchors[count - 1]);
    size_t upper = 0;
    while (upper < count && anchors[upper].position_seconds <= seconds) ++upper;
    const auto& left = anchors[upper - 1];
    const auto& right = anchors[upper];
    const double span = right.position_seconds - left.position_seconds;
    const double t = span > 0.0
        ? std::clamp((seconds - left.position_seconds) / span, 0.0, 1.0) : 0.0;
    const double l = value(left), r = value(right);
    return base + l + (r - l) * t;
}

double integral_at(const AudioNoteAnchorReference* anchors, size_t count,
                   const double* prefix, double start, double finish,
                   double pitch_offset, double seconds) {
    if (!std::isfinite(seconds) || seconds <= start) return 0.0;
    const double end = std::min(seconds, finish);
    if (end <= start) return 0.0;
    if (count == 0) return (end - start) * ratio_for_cents(pitch_offset);
    const auto& first = anchors[0];
    double total = (std::min(end, first.position_seconds) - start) *
        ratio_for_cents(pitch_offset + first.pitch_cents);
    if (end <= first.position_seconds) return total;
    size_t upper = 0;
    while (upper < count && anchors[upper].position_seconds <= end) ++upper;
    const size_t left_index = upper - 1;
    total += prefix[left_index];
    if (upper == count) {
        total += (end - anchors[count - 1].position_seconds) *
            ratio_for_cents(pitch_offset + anchors[count - 1].pitch_cents);
    } else {
        const auto& left = anchors[left_index];
        const auto& right = anchors[upper];
        const double span = right.position_seconds - left.position_seconds;
        const double t = span > 0.0
            ? std::clamp((end - left.position_seconds) / span, 0.0, 1.0) : 0.0;
        const double end_cents = left.pitch_cents + (right.pitch_cents - left.pitch_cents) * t;
        total += integrate_linear_pitch(left.pitch_cents, end_cents,
                                        end - left.position_seconds, pitch_offset);
    }
    return std::isfinite(total) ? total : 0.0;
}
}

extern "C" void audio_note_curve_reference_build_prefix(
    const AudioNoteAnchorReference* anchors, size_t count, double pitch_offset,
    double* output) {
    if (count == 0 || !anchors || !output) return;
    output[0] = 0.0;
    for (size_t i = 1; i < count; ++i) {
        output[i] = output[i - 1] + integrate_linear_pitch(
            anchors[i - 1].pitch_cents, anchors[i].pitch_cents,
            anchors[i].position_seconds - anchors[i - 1].position_seconds,
            pitch_offset);
    }
}

extern "C" void audio_note_curve_reference_evaluate(
    const AudioNoteAnchorReference* anchors, size_t count, const double* prefix,
    double start, double finish, double pitch_offset, double formant_offset,
    double seconds, double* output) {
    if (!output) return;
    output[0] = interpolate(anchors, count, seconds, pitch_offset, false);
    output[1] = interpolate(anchors, count, seconds, formant_offset, true);
    output[2] = integral_at(anchors, count, prefix, start, finish, pitch_offset, seconds);
}

extern "C" void audio_note_curve_cpp_wrapper_probe(double* output) {
    if (!output) return;
    hirari::editing::AudioNoteSegment segment;
    segment.setTiming(0.0, 1.0);
    segment.setPitchOffset(100.0);
    segment.setFormantOffset(-50.0);
    segment.upsertAnchor({0.2, -300.0, 100.0});
    segment.upsertAnchor({0.8, 600.0, -250.0});
    segment.evaluateAt(0.45, output[0], output[1]);
    output[2] = segment.pitchRatioIntegralAt(0.45);
}

extern "C" void audio_note_segment_lookup_reference(
    const AudioNoteSegmentRangeReference* ranges, size_t count,
    double seconds, int64_t* output) {
    if (!output) return;
    output[0] = output[1] = output[2] = -1;
    if (!ranges || count == 0 || !std::isfinite(seconds)) return;
    size_t upper = 0;
    while (upper < count && ranges[upper].start_seconds <= seconds) ++upper;
    if (upper == 0) return;
    const size_t latest = upper - 1;
    if (ranges[latest].end_seconds < seconds) output[1] = static_cast<int64_t>(latest);
    if (seconds <= ranges[latest].end_seconds) output[2] = static_cast<int64_t>(latest);
    size_t cursor = upper;
    while (cursor > 0) {
        const size_t index = cursor - 1;
        const auto& range = ranges[index];
        if (range.end_seconds < seconds) break;
        if (seconds >= range.start_seconds && seconds <= range.end_seconds)
            output[0] = static_cast<int64_t>(index);
        cursor = index;
    }
}

extern "C" double audio_note_phase_correction_reference(
    double seconds, const AudioNoteCurveViewReference* matched,
    const AudioNoteCurveViewReference* previous) {
    if (matched) {
        const double segment_offset = std::max(0.0, seconds - matched->start_seconds);
        const double integral = integral_at(
            matched->anchors, matched->anchor_count, matched->prefix,
            matched->start_seconds, matched->end_seconds,
            matched->pitch_offset_cents, seconds);
        return matched->correction_before_seconds + integral - segment_offset;
    }
    if (previous) {
        const double integral = integral_at(
            previous->anchors, previous->anchor_count, previous->prefix,
            previous->start_seconds, previous->end_seconds,
            previous->pitch_offset_cents, previous->end_seconds);
        return previous->correction_before_seconds + integral -
            (previous->end_seconds - previous->start_seconds);
    }
    return 0.0;
}
