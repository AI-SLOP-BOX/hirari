#pragma once

#include <cstddef>
#include <cstdint>

extern "C" float hirari_dsp_hard_clip(float value);
extern "C" float hirari_dsp_decay_coefficient(float release_ms, double sample_rate);
extern "C" float hirari_dsp_db_to_linear(float db);
extern "C" float hirari_dsp_interpolate_hermite(
    float y0, float y1, float y2, float y3, float t);
extern "C" float hirari_dsp_sanitize(float value);

namespace Hirari::DSP::Utils {

class DSPUtils {
public:
    static constexpr size_t kMaxBufferSize = 1024;
    static constexpr uint32_t kIndexMask = kMaxBufferSize - 1;
    static constexpr float PI = 3.14159265358979323846f;
    static constexpr float TWO_PI = 6.28318530717958647692f;
    static constexpr float HALF_PI = 1.57079632679489661923f;

    static inline float hardClip(float in) { return hirari_dsp_hard_clip(in); }
    static inline float calculateDecayCoeff(float releaseMs, double sampleRate) {
        return hirari_dsp_decay_coefficient(releaseMs, sampleRate);
    }
    static inline float dbToLinear(float db) { return hirari_dsp_db_to_linear(db); }
    static inline float interpolateHermite(float y0, float y1, float y2, float y3, float t) {
        return hirari_dsp_interpolate_hermite(y0, y1, y2, y3, t);
    }
    static inline float sanitize(float value) { return hirari_dsp_sanitize(value); }
};

} // namespace Hirari::DSP::Utils
