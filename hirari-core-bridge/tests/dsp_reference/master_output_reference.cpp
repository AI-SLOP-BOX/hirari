#include <algorithm>
#include <cmath>
#include <cstdint>

extern "C" void hirari_master_output_reference_process(
    float* left,
    float* right,
    float* click_left,
    float* click_right,
    uint32_t frames,
    float master_gain,
    bool include_click) {
    if (!left || !right || (include_click && (!click_left || !click_right))) return;
    if (include_click) {
        for (uint32_t sample = 0; sample < frames; ++sample) {
            const float left_click = std::isfinite(click_left[sample]) ? click_left[sample] : 0.0f;
            const float right_click = std::isfinite(click_right[sample]) ? click_right[sample] : 0.0f;
            click_left[sample] = left_click;
            click_right[sample] = right_click;
            left[sample] = std::clamp(
                (std::isfinite(left[sample]) ? left[sample] : 0.0f) + left_click,
                -16.0f, 16.0f);
            right[sample] = std::clamp(
                (std::isfinite(right[sample]) ? right[sample] : 0.0f) + right_click,
                -16.0f, 16.0f);
        }
    }
    for (uint32_t sample = 0; sample < frames; ++sample) {
        if (!std::isfinite(left[sample])) left[sample] = 0.0f;
        if (!std::isfinite(right[sample])) right[sample] = 0.0f;
        left[sample] *= master_gain;
        right[sample] *= master_gain;
    }
}
