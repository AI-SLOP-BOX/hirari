#include <cmath>
#include <cstdint>

extern "C" bool hirari_bus_accumulate_stereo_reference(
    float* bus_left, float* bus_right, const float* input_left,
    const float* input_right, uint32_t frames, float gain) {
    if (!bus_left || !bus_right || !input_left || !input_right || frames == 0 ||
        frames > 8192 || !std::isfinite(gain)) return false;
    for (uint32_t frame = 0; frame < frames; ++frame) {
        const float left = std::isfinite(input_left[frame]) ? input_left[frame] : 0.0f;
        const float right = std::isfinite(input_right[frame]) ? input_right[frame] : 0.0f;
        bus_left[frame] += left * gain;
        bus_right[frame] += right * gain;
    }
    return true;
}
