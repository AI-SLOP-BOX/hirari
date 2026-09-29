#include <algorithm>
#include <cmath>
#include <cstdint>

extern "C" void hirari_region_read_warped_reference(
    const float*, uint64_t, uint64_t, uint64_t, double, uint8_t, uint8_t,
    double, const float*, float*);

extern "C" bool region_pitch_correction_reference_delays(
    double sample_rate, double reference_pitch_cents, double* output) {
    if (!output || !std::isfinite(sample_rate) || sample_rate <= 0.0) return false;
    const double reference_hz = std::isfinite(reference_pitch_cents) && reference_pitch_cents > 0.0
        ? 440.0 * std::exp2(std::clamp((reference_pitch_cents - 6900.0) / 1200.0, -5.0, 5.0))
        : 140.0;
    const double reference_period = sample_rate / std::clamp(reference_hz, 20.0, 2000.0);
    const double minimum_delay = std::clamp(std::max(2.0 * reference_period, 256.0), 256.0, 2048.0);
    const double maximum_delay = std::clamp(
        std::max(8.0 * reference_period, minimum_delay + 256.0),
        minimum_delay + 256.0, 7000.0);
    output[0] = minimum_delay;
    output[1] = maximum_delay - minimum_delay;
    output[2] = reference_hz;
    return true;
}

extern "C" bool region_pitch_correction_reference_frame(
    const float* source_left, const float* source_right,
    uint64_t source_samples, uint64_t source_offset, uint64_t source_span,
    double warped_position, double local_source_rate, double effective_pitch_ratio,
    double base_pitch_ratio, uint64_t loop_relative, double note_correction_seconds,
    double sample_rate, double minimum_delay, double delay_range, uint8_t reverse,
    const float* kernel, float* output) {
    if (!source_left || !source_right || !kernel || !output || delay_range <= 0.0) return false;
    const double delay_travel = local_source_rate *
        ((1.0 - base_pitch_ratio) * static_cast<double>(loop_relative) -
         base_pitch_ratio * note_correction_seconds * sample_rate);
    double phase_a = std::fmod(delay_travel / delay_range, 1.0);
    if (phase_a < 0.0) phase_a += 1.0;
    double phase_b = phase_a + 0.5;
    if (phase_b >= 1.0) phase_b -= 1.0;
    const double delay_a = minimum_delay + phase_a * delay_range;
    const double delay_b = minimum_delay + phase_b * delay_range;
    const float weight_a = static_cast<float>(1.0 - 2.0 * std::abs(phase_a - 0.5));
    const float weight_b = static_cast<float>(1.0 - 2.0 * std::abs(phase_b - 0.5));
    const double step = local_source_rate * effective_pitch_ratio;
    float left_a[2]{}, left_b[2]{}, right_a[2]{}, right_b[2]{};
    hirari_region_read_warped_reference(source_left, source_samples, source_offset, source_span,
        warped_position - delay_a, reverse, 1, step, kernel, left_a);
    hirari_region_read_warped_reference(source_left, source_samples, source_offset, source_span,
        warped_position - delay_b, reverse, 1, step, kernel, left_b);
    hirari_region_read_warped_reference(source_right, source_samples, source_offset, source_span,
        warped_position - delay_a, reverse, 1, step, kernel, right_a);
    hirari_region_read_warped_reference(source_right, source_samples, source_offset, source_span,
        warped_position - delay_b, reverse, 1, step, kernel, right_b);
    output[0] = left_a[0] * weight_a + left_b[0] * weight_b;
    output[1] = left_a[1] * weight_a + left_b[1] * weight_b;
    output[2] = right_a[0] * weight_a + right_b[0] * weight_b;
    output[3] = right_a[1] * weight_a + right_b[1] * weight_b;
    return true;
}
