#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>

namespace {
constexpr uint32_t kMaxHrtfTaps = 128;
struct HolographicPannerReference {
    explicit HolographicPannerReference(double rate = 48000.0) : sampleRate(rate) {
        std::fill(std::begin(delayL), std::end(delayL), 0.0f);
        std::fill(std::begin(delayR), std::end(delayR), 0.0f);
    }
    double sampleRate;
    float delayL[512]{}, delayR[512]{};
    float hrtfL[kMaxHrtfTaps]{}, hrtfR[kMaxHrtfTaps]{};
    float history[kMaxHrtfTaps]{};
    uint32_t hrtfTaps = 0;
    uint32_t historyPtr = 0;
    uint32_t ptr = 0;

    void setSampleRate(double rate) {
        if (!std::isfinite(rate) || rate <= 0.0) return;
        sampleRate = rate;
        std::fill(std::begin(delayL), std::end(delayL), 0.0f);
        std::fill(std::begin(delayR), std::end(delayR), 0.0f);
        std::fill(std::begin(history), std::end(history), 0.0f);
        ptr = 0;
        historyPtr = 0;
    }

    bool setKernel(const float* left, const float* right, uint32_t taps) {
        if (!left || !right || taps == 0 || taps > kMaxHrtfTaps) return false;
        for (uint32_t i = 0; i < taps; ++i) {
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        }
        hrtfTaps = taps;
        std::copy(left, left + taps, hrtfL);
        std::copy(right, right + taps, hrtfR);
        std::fill(hrtfL + taps, hrtfL + kMaxHrtfTaps, 0.0f);
        std::fill(hrtfR + taps, hrtfR + kMaxHrtfTaps, 0.0f);
        std::fill(std::begin(history), std::end(history), 0.0f);
        historyPtr = 0;
        return true;
    }

    void clearKernel() {
        hrtfTaps = 0;
        std::fill(std::begin(hrtfL), std::end(hrtfL), 0.0f);
        std::fill(std::begin(hrtfR), std::end(hrtfR), 0.0f);
    }

    void process(float* l, float* r, uint32_t samples, float x, float y, float z) {
        if (!l || !r || samples == 0) return;
        x = std::isfinite(x) ? std::clamp(x, -1.0f, 1.0f) : 0.0f;
        y = std::isfinite(y) ? std::clamp(y, -1.0f, 1.0f) : 0.0f;
        z = std::isfinite(z) ? std::clamp(z, -1.0f, 1.0f) : 0.0f;
        const double rate = std::isfinite(sampleRate) && sampleRate > 0.0 ? sampleRate : 48000.0;
        const float distance = std::clamp(1.0f - 0.35f * std::max(0.0f, z) -
            0.15f * std::abs(y), 0.25f, 1.0f);
        const float angle = (x + 1.0f) * 0.25f * 3.14159265358979323846f;
        const float leftGain = std::cos(angle) * distance;
        const float rightGain = std::sin(angle) * distance;
        const uint32_t delaySamples = std::min<uint32_t>(511u,
            static_cast<uint32_t>(std::abs(x) * 0.0007 * rate));
        for (uint32_t i = 0; i < samples; ++i) {
            const float inL = l[i], inR = r[i];
            const float mono = 0.5f * ((std::isfinite(inL) ? inL : 0.0f) +
                                      (std::isfinite(inR) ? inR : 0.0f));
            const uint32_t slot = ptr++ % 512u;
            delayL[slot] = mono;
            delayR[slot] = mono;
            const uint32_t delayed = (slot + 512u - delaySamples) % 512u;
            if (hrtfTaps != 0) {
                history[historyPtr++ % kMaxHrtfTaps] = mono;
                float outL = 0.0f, outR = 0.0f;
                for (uint32_t tap = 0; tap < hrtfTaps; ++tap) {
                    const uint32_t index = (historyPtr + kMaxHrtfTaps - 1u - tap) % kMaxHrtfTaps;
                    outL += history[index] * hrtfL[tap];
                    outR += history[index] * hrtfR[tap];
                }
                l[i] = std::isfinite(outL) ? outL * distance : 0.0f;
                r[i] = std::isfinite(outR) ? outR * distance : 0.0f;
            } else {
                l[i] = std::isfinite(delayL[delayed] * leftGain)
                    ? delayL[delayed] * leftGain : 0.0f;
                r[i] = std::isfinite(delayR[delayed] * rightGain)
                    ? delayR[delayed] * rightGain : 0.0f;
            }
        }
    }
};
} // namespace

extern "C" void* hirari_track_holographic_panner_create_reference() {
    return new HolographicPannerReference();
}
extern "C" void hirari_track_holographic_panner_destroy_reference(void* state) {
    delete static_cast<HolographicPannerReference*>(state);
}
extern "C" void hirari_track_holographic_panner_set_sample_rate_reference(void* state, double rate) {
    if (state) static_cast<HolographicPannerReference*>(state)->setSampleRate(rate);
}
extern "C" bool hirari_track_holographic_panner_set_kernel_reference(
    void* state, const float* left, const float* right, uint32_t taps) {
    return state && static_cast<HolographicPannerReference*>(state)->setKernel(left, right, taps);
}
extern "C" void hirari_track_holographic_panner_clear_kernel_reference(void* state) {
    if (state) static_cast<HolographicPannerReference*>(state)->clearKernel();
}
extern "C" void hirari_track_holographic_panner_process_reference(
    void* state, float* left, float* right, uint32_t frames, float x, float y, float z) {
    if (state) static_cast<HolographicPannerReference*>(state)->process(left, right, frames, x, y, z);
}
