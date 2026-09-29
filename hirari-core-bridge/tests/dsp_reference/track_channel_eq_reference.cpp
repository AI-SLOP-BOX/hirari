#include <algorithm>
#include <cmath>
#include <cstdint>

extern "C" float hirari_track_eq_gain_reference(float boostDb, float cutDb) {
    if (!std::isfinite(boostDb) || !std::isfinite(cutDb)) return 1.0f;
    const float db = std::clamp(boostDb - cutDb, -120.0f, 24.0f);
    const float gain = std::pow(10.0f, db / 20.0f);
    return std::isfinite(gain) ? gain : 1.0f;
}

extern "C" void hirari_track_eq_process_reference(
    float* left,
    float* right,
    uint32_t frames,
    float lowGain,
    float highGain,
    float* stateL,
    float* stateR) {
    if (!left || !right || !stateL || !stateR || frames == 0) return;
    if (!std::isfinite(*stateL)) *stateL = 0.0f;
    if (!std::isfinite(*stateR)) *stateR = 0.0f;
    if (!std::isfinite(lowGain)) lowGain = 1.0f;
    if (!std::isfinite(highGain)) highGain = 1.0f;
    for (uint32_t i = 0; i < frames; ++i) {
        const float inL = std::isfinite(left[i]) ? std::clamp(left[i], -1.0e6f, 1.0e6f) : 0.0f;
        const float inR = std::isfinite(right[i]) ? std::clamp(right[i], -1.0e6f, 1.0e6f) : 0.0f;
        *stateL += 0.02f * (inL - *stateL);
        *stateR += 0.02f * (inR - *stateR);
        const float outL = *stateL * lowGain + (inL - *stateL) * highGain;
        const float outR = *stateR * lowGain + (inR - *stateR) * highGain;
        if (std::isfinite(outL)) left[i] = outL;
        else { left[i] = 0.0f; *stateL = 0.0f; }
        if (std::isfinite(outR)) right[i] = outR;
        else { right[i] = 0.0f; *stateR = 0.0f; }
    }
}
