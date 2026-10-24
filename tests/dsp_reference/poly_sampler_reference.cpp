// Frozen scalar reference for the former HirariSamplerPro voice renderer.
#include <algorithm>
#include <cmath>
#include <cstdint>

namespace {
float sample_at(const float* data, int64_t raw, uint32_t length,
                bool looping, uint32_t loop_start, uint32_t loop_end) {
    int64_t index = raw;
    if (looping && loop_end > loop_start + 1) {
        const int64_t start = loop_start;
        const int64_t end = loop_end;
        const int64_t span = end - start;
        if (index < start) {
            index = end - ((start - index) % span);
            if (index == end) index = start;
        } else if (index >= end) {
            index = start + ((index - start) % span);
        }
    } else {
        index = std::clamp<int64_t>(index, 0, static_cast<int64_t>(length - 1));
    }
    const float value = data[static_cast<size_t>(index)];
    return std::isfinite(value) ? value : 0.0f;
}

float interpolate(const float* data, uint32_t length, double position,
                  bool looping, uint32_t loop_start, uint32_t loop_end) {
    const auto i = static_cast<int64_t>(std::floor(position));
    const float t = static_cast<float>(position - static_cast<double>(i));
    const float y0 = sample_at(data, i - 1, length, looping, loop_start, loop_end);
    const float y1 = sample_at(data, i, length, looping, loop_start, loop_end);
    const float y2 = sample_at(data, i + 1, length, looping, loop_start, loop_end);
    const float y3 = sample_at(data, i + 2, length, looping, loop_start, loop_end);
    const float c0 = y1;
    const float c1 = 0.5f * (y2 - y0);
    const float c2 = y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3;
    const float c3 = 0.5f * (y3 - y0) + 1.5f * (y1 - y2);
    const float result = ((c3 * t + c2) * t + c1) * t + c0;
    return std::isfinite(result) ? result : y1;
}
}

extern "C" void hirari_poly_sampler_reference_process(
    const float* sample, uint32_t sample_size, float* output_left, float* output_right,
    uint32_t frames, double sample_rate, double source_rate, uint32_t loop_start,
    uint32_t loop_end, uint32_t release_at, float velocity) {
    std::fill(output_left, output_left + frames, 0.0f);
    std::fill(output_right, output_right + frames, 0.0f);
    float level = 0.0f;
    float filter_left = 0.0f;
    float filter_right = 0.0f;
    const float attack_step = 1.0f / (0.005f * static_cast<float>(sample_rate));
    const float decay_step = 0.2f / (0.1f * static_cast<float>(sample_rate));
    const float release_coefficient = static_cast<float>(std::exp(-1.0 / (0.3 * sample_rate)));
    const float cutoff = 1000.0f;
    const float rc = 1.0f / (2.0f * static_cast<float>(M_PI) * cutoff);
    const float alpha = std::clamp((1.0f / static_cast<float>(sample_rate)) /
                                   (rc + 1.0f / static_cast<float>(sample_rate)), 0.001f, 1.0f);
    const double rate_ratio = source_rate >= 8000.0 && source_rate <= 384000.0
        ? source_rate / sample_rate : 1.0;
    double position = 0.0;
    int stage = 1; // attack, decay, sustain, release, idle
    for (uint32_t frame = 0; frame < frames; ++frame) {
        if (frame == release_at) stage = 4;
        if (stage == 0) break;
        if (position >= loop_end) {
            const double loop_length = static_cast<double>(loop_end - loop_start);
            position = loop_start + std::fmod(std::max(0.0, position - loop_start), loop_length);
        }
        const float sample_value = interpolate(sample, sample_size, position, true, loop_start, loop_end);
        if (stage == 1) {
            level = std::min(1.0f, level + attack_step);
            if (level >= 1.0f) stage = 2;
        } else if (stage == 2) {
            level = std::max(0.8f, level - decay_step);
            if (level <= 0.8f) stage = 3;
        } else if (stage == 3) {
            level = 0.8f;
        } else {
            level *= release_coefficient;
            if (level <= 1.0e-5f) { level = 0.0f; stage = 0; }
        }
        const float amplitude = level * velocity;
        const float input = sample_value * amplitude;
        filter_left += alpha * (input - filter_left);
        filter_right += alpha * (input - filter_right);
        output_left[frame] = std::isfinite(filter_left) ? filter_left : input;
        output_right[frame] = std::isfinite(filter_right) ? filter_right : input;
        position += rate_ratio;
    }
}
