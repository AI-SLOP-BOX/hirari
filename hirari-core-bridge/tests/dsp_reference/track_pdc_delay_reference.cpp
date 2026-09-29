#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>

namespace {
constexpr uint32_t kMaxDelay = 8192;
constexpr uint32_t kCapacity = kMaxDelay + 1;
struct TrackPdcDelayReference {
    std::array<float, kCapacity> delayL{};
    std::array<float, kCapacity> delayR{};
    std::array<float, kCapacity> preDelayL{};
    std::array<float, kCapacity> preDelayR{};
    uint32_t previous = 0;
    uint32_t active = 0;
    uint32_t transitionRemaining = 0;
    uint32_t write = 0;

    float delayed(uint32_t delay, bool right, bool preTap) const {
        if (delay == 0) return 0.0f;
        const uint32_t read = (write + kCapacity - delay) % kCapacity;
        if (preTap) return right ? preDelayR[read] : preDelayL[read];
        return right ? delayR[read] : delayL[read];
    }

    void reset(uint32_t requested) {
        delayL.fill(0.0f);
        delayR.fill(0.0f);
        preDelayL.fill(0.0f);
        preDelayR.fill(0.0f);
        write = 0;
        active = std::min(requested, kMaxDelay);
        previous = active;
        transitionRemaining = 0;
    }

    void process(float* left, float* right, float* preLeft, float* preRight,
                 uint32_t frames, uint32_t requested) {
        requested = std::min(requested, kMaxDelay);
        if (requested != active) {
            previous = active;
            active = requested;
            transitionRemaining = 64;
        }
        for (uint32_t i = 0; i < frames; ++i) {
            const float inL = left[i];
            const float inR = right[i];
            const float preInL = preLeft ? preLeft[i] : 0.0f;
            const float preInR = preRight ? preRight[i] : 0.0f;
            const float oldL = previous == 0 ? inL : delayed(previous, false, false);
            const float oldR = previous == 0 ? inR : delayed(previous, true, false);
            const float newL = active == 0 ? inL : delayed(active, false, false);
            const float newR = active == 0 ? inR : delayed(active, true, false);
            const float oldPreL = previous == 0 ? preInL : delayed(previous, false, true);
            const float oldPreR = previous == 0 ? preInR : delayed(previous, true, true);
            const float newPreL = active == 0 ? preInL : delayed(active, false, true);
            const float newPreR = active == 0 ? preInR : delayed(active, true, true);
            if (transitionRemaining > 0) {
                const float progress = static_cast<float>(64 - transitionRemaining) / 63.0f;
                left[i] = oldL + (newL - oldL) * progress;
                right[i] = oldR + (newR - oldR) * progress;
                if (preLeft && preRight) {
                    preLeft[i] = oldPreL + (newPreL - oldPreL) * progress;
                    preRight[i] = oldPreR + (newPreR - oldPreR) * progress;
                }
                --transitionRemaining;
            } else {
                left[i] = newL;
                right[i] = newR;
                if (preLeft && preRight) {
                    preLeft[i] = newPreL;
                    preRight[i] = newPreR;
                }
            }
            delayL[write] = std::isfinite(inL) ? inL : 0.0f;
            delayR[write] = std::isfinite(inR) ? inR : 0.0f;
            preDelayL[write] = std::isfinite(preInL) ? preInL : 0.0f;
            preDelayR[write] = std::isfinite(preInR) ? preInR : 0.0f;
            write = (write + 1) % kCapacity;
        }
    }
};
} // namespace

extern "C" void* hirari_track_pdc_delay_create_reference() {
    return new TrackPdcDelayReference{};
}
extern "C" void hirari_track_pdc_delay_destroy_reference(void* state) {
    delete static_cast<TrackPdcDelayReference*>(state);
}
extern "C" void hirari_track_pdc_delay_reset_reference(void* state, uint32_t requested) {
    if (state) static_cast<TrackPdcDelayReference*>(state)->reset(requested);
}
extern "C" void hirari_track_pdc_delay_process_reference(
    void* state, float* left, float* right, float* preLeft, float* preRight,
    uint32_t frames, uint32_t requested) {
    if (state && left && right) {
        static_cast<TrackPdcDelayReference*>(state)->process(
            left, right, preLeft, preRight, frames, requested);
    }
}
