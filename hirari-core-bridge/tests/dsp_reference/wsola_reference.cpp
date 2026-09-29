// Frozen correlation search extracted from Track's pre-Rust WSOLA renderer.
#include <algorithm>
#include <cmath>
#include <cstdint>

extern "C" void hirari_region_read_warped_reference(
    const float*, uint64_t, uint64_t, uint64_t, double, uint8_t, uint8_t,
    double, const float*, float*);

extern "C" double hirari_wsola_select_grain_reference(
    const float* left, const float* right, uint64_t source_samples,
    uint64_t source_offset, uint64_t source_span, double expected,
    double reference, double overlap_span, double reference_span,
    int64_t center, int64_t last, int64_t search_radius, uint8_t reverse,
    const double* current_offsets, const double* previous_offsets,
    uint32_t point_count) {
    if (!left || !right || !current_offsets || !previous_offsets ||
        source_span == 0 || source_offset > source_samples ||
        source_span > source_samples - source_offset ||
        !std::isfinite(expected) || !std::isfinite(reference) ||
        !std::isfinite(overlap_span) || !std::isfinite(reference_span) ||
        search_radius < 0 || point_count == 0 || point_count > 32)
        return expected;

    last = std::min<int64_t>(last, static_cast<int64_t>(source_span - 1));
    const auto score_candidate = [&](int64_t candidate) {
        if (candidate < 0 || static_cast<double>(candidate) + overlap_span >= last ||
            reference < 0.0 || reference + reference_span >= last)
            return -2.0;
        double dot_l = 0.0, energy_al = 0.0, energy_bl = 0.0;
        double dot_r = 0.0, energy_ar = 0.0, energy_br = 0.0;
        for (uint32_t i = 0; i < point_count; ++i) {
            const double a_rel = std::round(static_cast<double>(candidate) + current_offsets[i]);
            const double b_rel = std::round(reference + previous_offsets[i]);
            if (a_rel < 0.0 || b_rel < 0.0 || a_rel >= source_span || b_rel >= source_span)
                continue;
            const auto a_relative = static_cast<uint64_t>(a_rel);
            const auto b_relative = static_cast<uint64_t>(b_rel);
            const uint64_t a_index = reverse
                ? source_offset + source_span - 1 - a_relative
                : source_offset + a_relative;
            const uint64_t b_index = reverse
                ? source_offset + source_span - 1 - b_relative
                : source_offset + b_relative;
            if (a_index >= source_samples || b_index >= source_samples) continue;
            const double a_l = left[a_index], a_r = right[a_index];
            const double b_l = left[b_index], b_r = right[b_index];
            if (!std::isfinite(a_l) || !std::isfinite(a_r) ||
                !std::isfinite(b_l) || !std::isfinite(b_r)) continue;
            dot_l += a_l * b_l;
            energy_al += a_l * a_l;
            energy_bl += b_l * b_l;
            dot_r += a_r * b_r;
            energy_ar += a_r * a_r;
            energy_br += b_r * b_r;
        }
        double correlation = 0.0;
        uint32_t active_channels = 0;
        if (energy_al > 1.0e-20 && energy_bl > 1.0e-20) {
            correlation += std::clamp(dot_l / std::sqrt(energy_al * energy_bl), -1.0, 1.0);
            ++active_channels;
        }
        if (energy_ar > 1.0e-20 && energy_br > 1.0e-20) {
            correlation += std::clamp(dot_r / std::sqrt(energy_ar * energy_br), -1.0, 1.0);
            ++active_channels;
        }
        return active_channels == 0 ? -2.0 : correlation / active_channels;
    };

    double best_correlation = -2.0;
    double selected = expected;
    for (int64_t offset = -search_radius; offset <= search_radius; offset += 4) {
        const int64_t candidate = center + offset;
        const double correlation = score_candidate(candidate);
        if (correlation > best_correlation) {
            best_correlation = correlation;
            selected = static_cast<double>(candidate);
        }
    }
    if (best_correlation > -2.0) {
        const int64_t coarse_best = static_cast<int64_t>(selected);
        const int64_t fine_start = std::max(center - search_radius, coarse_best - 3);
        const int64_t fine_end = std::min(center + search_radius, coarse_best + 3);
        for (int64_t candidate = fine_start; candidate <= fine_end; ++candidate) {
            if (candidate == coarse_best) continue;
            const double correlation = score_candidate(candidate);
            if (correlation > best_correlation) {
                best_correlation = correlation;
                selected = static_cast<double>(candidate);
            }
        }
    }
    return selected;
}

extern "C" void hirari_wsola_render_frame_reference(
    const float* left, const float* right, uint64_t source_samples,
    uint64_t source_offset, uint64_t source_span, uint8_t reverse,
    double resample_step, const float* kernel, const double* positions,
    const float* weights, uint32_t grain_count, float* output) {
    if (output == nullptr) return;
    for (int i = 0; i < 4; ++i) output[i] = 0.0f;
    if (left == nullptr || right == nullptr || kernel == nullptr ||
        positions == nullptr || weights == nullptr || grain_count > 4) return;
    float weight_sum = 0.0f;
    for (uint32_t grain = 0; grain < grain_count; ++grain) {
        float left_sample[2]{};
        float right_sample[2]{};
        hirari_region_read_warped_reference(
            left, source_samples, source_offset, source_span, positions[grain],
            reverse, 0, resample_step, kernel, left_sample);
        hirari_region_read_warped_reference(
            right, source_samples, source_offset, source_span, positions[grain],
            reverse, 0, resample_step, kernel, right_sample);
        output[0] += left_sample[0] * weights[grain];
        output[1] += left_sample[1] * weights[grain];
        output[2] += right_sample[0] * weights[grain];
        output[3] += right_sample[1] * weights[grain];
        weight_sum += weights[grain];
    }
    if (weight_sum > 1.0e-6f) {
        for (int i = 0; i < 4; ++i) output[i] /= weight_sum;
    }
}

// Frozen Track-level, no-note-curve WSOLA scheduler used to compare the new
// Rust region-frame entry point without depending on its implementation.
extern "C" void hirari_region_wsola_frame_reference(
    const float* left, const float* right, uint64_t source_samples,
    uint64_t source_offset, uint64_t source_span, uint64_t region_length,
    uint64_t loop_relative, double sample_rate, double source_rate,
    double base_pitch_ratio, double effective_pitch_ratio,
    uint64_t* cache_ids, double* cache_starts, size_t cache_capacity,
    uint32_t sync_group, uint8_t reverse, const float* kernel,
    const float* window, float* output) {
    if (!output) return;
    for (int i = 0; i < 4; ++i) output[i] = 0.0f;
    if (!left || !right || !cache_ids || !cache_starts || !kernel || !window ||
        cache_capacity == 0 || source_span == 0 || region_length == 0) return;
    constexpr uint64_t hop = 256, grain_size = 1024;
    constexpr int64_t radius = 64;
    const auto source_position = [&](uint64_t sample) {
        return std::clamp(static_cast<double>(sample) * source_rate,
                          0.0, static_cast<double>(source_span));
    };
    const auto pitch_ratio = [&](uint64_t) { return base_pitch_ratio; };
    const auto grain_start = [&](uint64_t grain) {
        const size_t index = static_cast<size_t>(grain % cache_capacity);
        if (cache_ids[index] == grain) return cache_starts[index];
        const uint64_t output_start = std::min(grain * hop, region_length - 1);
        const double expected = source_position(output_start);
        double selected = expected;
        const double grain_ratio = pitch_ratio(grain);
        if (sync_group == 0 && grain > 0 && expected < source_span) {
            const double previous_ratio = pitch_ratio(grain - 1);
            const uint64_t previous_start = (grain - 1) * hop;
            const double previous_expected = source_position(previous_start);
            const uint64_t previous_end = std::min(previous_start + hop, region_length - 1);
            const double previous_mapped_end = source_position(previous_end);
            const double reference = previous_expected +
                (previous_mapped_end - previous_expected) * previous_ratio;
            const uint64_t current_end = std::min(output_start + hop, region_length - 1);
            const double current_mapped_end = source_position(current_end);
            const double overlap = (current_mapped_end - expected) * grain_ratio;
            const double reference_span = (previous_mapped_end - previous_expected) * previous_ratio;
            const int64_t center = static_cast<int64_t>(std::llround(expected));
            const int64_t last = static_cast<int64_t>(source_span - 1);
            const int64_t search_radius = radius + static_cast<int64_t>(std::llround(
                static_cast<double>(hop) * std::abs(source_rate - 1.0)));
            double current_offsets[32]{};
            double previous_offsets[32]{};
            for (uint64_t point = 0; point < hop; point += 8) {
                const size_t i = static_cast<size_t>(point / 8);
                const uint64_t current_sample = std::min(output_start + point, region_length - 1);
                const uint64_t previous_sample = std::min(previous_start + point, region_length - 1);
                current_offsets[i] = (source_position(current_sample) - expected) * grain_ratio;
                previous_offsets[i] = (source_position(previous_sample) - previous_expected) * previous_ratio;
            }
            selected = hirari_wsola_select_grain_reference(
                left, right, source_samples, source_offset, source_span, expected,
                reference, overlap, reference_span, center, last, search_radius,
                reverse, current_offsets, previous_offsets, 32);
        }
        cache_ids[index] = grain;
        cache_starts[index] = selected;
        return selected;
    };
    const uint64_t current = loop_relative / hop;
    const uint64_t first = current > 3 ? current - 3 : 0;
    double positions[4]{};
    float weights[4]{};
    uint32_t count = 0;
    for (uint64_t grain = first; grain <= current; ++grain) {
        const uint64_t start = grain * hop;
        const uint64_t frame = loop_relative - start;
        if (frame >= grain_size) continue;
        const uint64_t frame_sample = std::min(start + frame, region_length - 1);
        const double position = grain_start(grain) +
            (source_position(frame_sample) - source_position(start)) * effective_pitch_ratio;
        if (position < 0.0 || position >= source_span || count >= 4) continue;
        positions[count] = position;
        weights[count] = window[frame];
        ++count;
    }
    hirari_wsola_render_frame_reference(
        left, right, source_samples, source_offset, source_span, reverse,
        effective_pitch_ratio, kernel, positions, weights, count, output);
    (void)sample_rate;
}
