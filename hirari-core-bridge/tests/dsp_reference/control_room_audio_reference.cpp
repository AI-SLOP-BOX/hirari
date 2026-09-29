#include <algorithm>
#include <cmath>
#include <cstdint>

extern "C" void hirari_control_room_process_monitor_reference(
    float* left, float* right, const float* talkback, uint32_t frames,
    float monitor_gain, float talkback_gain, bool talkback_enabled) {
    if (!left || !right) return;
    const float monitor = std::clamp(monitor_gain, 0.0f, 4.0f);
    for (uint32_t i = 0; i < frames; ++i) {
        const float in_left = std::isfinite(left[i]) ? left[i] : 0.0f;
        const float in_right = std::isfinite(right[i]) ? right[i] : 0.0f;
        left[i] = std::clamp(in_left * monitor, -16.0f, 16.0f);
        right[i] = std::clamp(in_right * monitor, -16.0f, 16.0f);
    }
    if (!talkback || !talkback_enabled) return;
    const float gain = std::clamp(talkback_gain, 0.0f, 4.0f);
    for (uint32_t i = 0; i < frames; ++i) {
        const float sample = std::isfinite(talkback[i])
            ? std::clamp(talkback[i], -16.0f, 16.0f) * gain : 0.0f;
        left[i] = std::clamp((std::isfinite(left[i]) ? left[i] : 0.0f) + sample, -16.0f, 16.0f);
        right[i] = std::clamp((std::isfinite(right[i]) ? right[i] : 0.0f) + sample, -16.0f, 16.0f);
    }
}

extern "C" void hirari_control_room_mix_cue_reference(
    const float* source_left, const float* source_right,
    const float* click_left, const float* click_right,
    float* output_left, float* output_right, uint32_t frames,
    float cue_gain, bool click_enabled) {
    if (!source_left || !source_right || !output_left || !output_right) return;
    for (uint32_t i = 0; i < frames; ++i) {
        const float left = std::isfinite(source_left[i]) ? source_left[i] : 0.0f;
        const float right = std::isfinite(source_right[i]) ? source_right[i] : 0.0f;
        const float click_l = click_enabled && click_left && std::isfinite(click_left[i])
            ? click_left[i] : 0.0f;
        const float click_r = click_enabled && click_right && std::isfinite(click_right[i])
            ? click_right[i] : 0.0f;
        output_left[i] = std::clamp(left * cue_gain + click_l, -16.0f, 16.0f);
        output_right[i] = std::clamp(right * cue_gain + click_r, -16.0f, 16.0f);
    }
}
