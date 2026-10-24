#include <cmath>
#include <cstddef>

extern "C" void hirari_vocal_remove_stereo_reference(
    float* left, float* right, size_t frames) {
    if (!left || !right || frames == 0) return;
    float bass_left = 0.0f;
    float bass_right = 0.0f;
    constexpr float coefficient = 0.02f;
    for (size_t frame = 0; frame < frames; ++frame) {
        const float input_left = left[frame];
        const float input_right = right[frame];
        bass_left += coefficient * (input_left - bass_left);
        bass_right += coefficient * (input_right - bass_right);
        const float vocal = input_left - input_right;
        left[frame] = vocal * 0.5f + bass_left * 0.5f;
        right[frame] = -vocal * 0.5f + bass_right * 0.5f;
    }
}

extern "C" float hirari_stereo_correlation_reference(
    const float* left, const float* right, size_t frames) {
    if (!left || !right || frames == 0) return 1.0f;
    double sum_lr = 0.0;
    double sum_left = 0.0;
    double sum_right = 0.0;
    for (size_t frame = 0; frame < frames; ++frame) {
        const double l = left[frame];
        const double r = right[frame];
        sum_lr += l * r;
        sum_left += l * l;
        sum_right += r * r;
    }
    const double denominator = std::sqrt(sum_left * sum_right);
    return denominator < 1.0e-9 ? 1.0f : static_cast<float>(sum_lr / denominator);
}
