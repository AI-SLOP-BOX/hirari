#include "hirari-core-bridge/src/lib.rs.h"
#include <string>
#include <iostream>
#include <cmath>
#include <algorithm>

namespace Hirari::Core::BridgeFFI {

void HIRARI_LOG(uint32_t level, const std::string& msg) {
    ::Hirari::Core::Bridge::report_hirari_log(level, rust::Str(msg));
}

} // namespace Hirari::Core::BridgeFFI

#include "core/status_queue.hpp"

namespace Hirari::Core::Bridge {

} // namespace Hirari::Core::Bridge

// Frozen reference for the pre-migration AnalogSaturator / Oversampler2x path.
extern "C" void hirari_analog_saturator_frozen_reference(
    const float* input_left, const float* input_right,
    float* output_left, float* output_right, uint32_t frames,
    float drive, float warmth, uint32_t model) {
    constexpr float a1 = 0.12967654578647f;
    constexpr float a2 = 0.48418923434341f;
    constexpr float dc_cut = 0.995f;
    float phase_l1 = 0.0f, phase_l2 = 0.0f, phase_r1 = 0.0f, phase_r2 = 0.0f;
    float dc_l = 0.0f, dc_r = 0.0f;
    const float pre_gain = std::pow(10.0f, std::clamp(std::isfinite(drive) ? drive : 0.0f, 0.0f, 4.0f) * 12.0f / 20.0f);
    const float safe_warmth = std::clamp(std::isfinite(warmth) ? warmth : 0.5f, 0.0f, 1.0f);
    const float post_gain = 1.0f / std::max(1.0f, pre_gain * 0.35f);
    const auto shape = [safe_warmth, model](float x) {
        if (model == 1) {
            const float ax = std::abs(x);
            return ax < 1.0f ? x * (1.5f - 0.5f * x * x) : (x > 0.0f ? 1.0f : -1.0f);
        }
        if (model == 2) return std::tanh(x);
        const float bias = safe_warmth * 0.25f;
        return (x + bias) / (1.0f + std::abs(x + bias)) - bias / (1.0f + std::abs(bias));
    };
    for (uint32_t i = 0; i < frames; ++i) {
        const float in_l = std::isfinite(input_left[i]) ? input_left[i] : 0.0f;
        const float in_r = std::isfinite(input_right[i]) ? input_right[i] : 0.0f;
        const float up_l1 = a1 * (in_l * pre_gain - phase_l1) + phase_l1; phase_l1 = up_l1;
        const float up_l2 = a2 * (in_l * pre_gain - phase_l2) + phase_l2; phase_l2 = up_l2;
        const float up_r1 = a1 * (in_r * pre_gain - phase_r1) + phase_r1; phase_r1 = up_r1;
        const float up_r2 = a2 * (in_r * pre_gain - phase_r2) + phase_r2; phase_r2 = up_r2;
        const float wet_l = (shape(up_l1) * post_gain + shape(up_l2) * post_gain) * 0.5f;
        const float wet_r = (shape(up_r1) * post_gain + shape(up_r2) * post_gain) * 0.5f;
        dc_l = dc_cut * dc_l + (1.0f - dc_cut) * wet_l;
        dc_r = dc_cut * dc_r + (1.0f - dc_cut) * wet_r;
        const float out_l = wet_l - dc_l, out_r = wet_r - dc_r;
        output_left[i] = std::isfinite(out_l) ? out_l : 0.0f;
        output_right[i] = std::isfinite(out_r) ? out_r : 0.0f;
    }
}

extern "C" bool hirari_status_queue_cpp_wrapper_smoke() {
    Hirari::Core::StatusMessage message{};
    Hirari::Core::pushEngineStatus(Hirari::Core::StatusSeverity::Warning, "C++ bridge status");
    if (!Hirari::Core::popEngineStatus(message) ||
        message.severity != Hirari::Core::StatusSeverity::Warning ||
        std::string_view(message.text) != "C++ bridge status") return false;
    for (size_t index = 0; index < 256; ++index) {
        Hirari::Core::pushEngineStatus(Hirari::Core::StatusSeverity::Info, "bounded");
    }
    Hirari::Core::pushEngineStatus(Hirari::Core::StatusSeverity::Error, "overflow");
    const bool counted = Hirari::Core::takeDroppedEngineStatus() == 1;
    while (Hirari::Core::popEngineStatus(message)) {}
    return counted;
}

