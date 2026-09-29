#include "../external/signalsmith-stretch/signalsmith-stretch.h"
#include <exception>
#include <new>

namespace {
using Stretch = signalsmith::stretch::SignalsmithStretch<float, void, true>;
}

extern "C" void* hirari_stretch_kernel_create() noexcept {
    return new (std::nothrow) Stretch();
}

// Deterministic construction is used only by the frozen differential fixture.
extern "C" void* hirari_stretch_kernel_create_seeded(long seed) noexcept {
    return new (std::nothrow) Stretch(seed);
}

extern "C" void hirari_stretch_kernel_destroy(void* handle) noexcept {
    delete static_cast<Stretch*>(handle);
}

extern "C" bool hirari_stretch_kernel_prepare(void* handle, float sample_rate) noexcept {
    if (!handle) return false;
    try {
        static_cast<Stretch*>(handle)->presetDefault(2, sample_rate, true);
        return true;
    } catch (...) {
        return false;
    }
}

extern "C" void hirari_stretch_kernel_reset(void* handle) noexcept {
    if (handle) static_cast<Stretch*>(handle)->reset();
}

extern "C" int hirari_stretch_kernel_seek_length(void* handle, float rate) noexcept {
    return handle ? static_cast<Stretch*>(handle)->outputSeekLength(rate) : 0;
}

extern "C" int hirari_stretch_kernel_latency(void* handle) noexcept {
    return handle ? static_cast<Stretch*>(handle)->outputLatency() : 0;
}

extern "C" bool hirari_stretch_kernel_set_controls(
    void* handle, float transpose, float formant, float formant_base) noexcept {
    if (!handle) return false;
    try {
        auto& engine = *static_cast<Stretch*>(handle);
        engine.setTransposeSemitones(transpose);
        engine.setFormantSemitones(formant, true);
        if (formant_base > 0.0f) engine.setFormantBase(formant_base);
        return true;
    } catch (...) {
        return false;
    }
}

extern "C" bool hirari_stretch_kernel_seek(
    void* handle, const float* left, const float* right, int samples) noexcept {
    if (!handle || !left || !right || samples <= 0) return false;
    try {
        const float* channels[2] = {left, right};
        static_cast<Stretch*>(handle)->outputSeek(channels, samples);
        return true;
    } catch (...) {
        return false;
    }
}

extern "C" bool hirari_stretch_kernel_process(
    void* handle, const float* input_left, const float* input_right,
    int input_samples, float* output_left, float* output_right,
    int output_samples) noexcept {
    if (!handle || !input_left || !input_right || !output_left || !output_right ||
        input_samples <= 0 || output_samples <= 0) return false;
    try {
        const float* inputs[2] = {input_left, input_right};
        float* outputs[2] = {output_left, output_right};
        static_cast<Stretch*>(handle)->process(inputs, input_samples, outputs, output_samples);
        return true;
    } catch (...) {
        return false;
    }
}
