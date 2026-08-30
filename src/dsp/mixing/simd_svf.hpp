#include <arm_neon.h>
#include <cmath>
#include <algorithm>
#include "../../core/Aura.hpp"

namespace Aura::DSP::Mixing {

/**
 * @class SIMDSVF
 * @brief INDUSTRIAL High-Performance State Variable Filter.
 * CORRECTED: Stereo-parallel TPT implementation.
 */
class SIMDSVF {
public:
    SIMDSVF(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
    }

    void reset() {
        m_ic1 = vdupq_n_f32(0.0f); 
        m_ic2 = vdupq_n_f32(0.0f);
    }

    void process(const float* inL, const float* inR, float* outL, float* outR, 
                 float cutoff, float res, uint32_t numSamples) {
        if (!inL || !inR || !outL || !outR || numSamples == 0) return;
        // Pre-calculate coefficients
        const float sr = static_cast<float>(m_sampleRate);
        if (!std::isfinite(sr) || sr <= 1000.0f) return;
        const float safeCutoff = std::clamp(std::isfinite(cutoff) ? cutoff : 1000.0f,
                                            5.0f, sr * 0.49f);
        const float safeRes = std::clamp(std::isfinite(res) ? res : 0.707f,
                                         0.05f, 4.0f);
        float g = std::tan(static_cast<float>(M_PI) * safeCutoff / sr);
        float k = 1.0f / safeRes;
        float a1 = 1.0f / (1.0f + g * (g + k));
        float a2 = g * a1;
        float a3 = g * a2;
        if (!std::isfinite(a1) || !std::isfinite(a2) || !std::isfinite(a3)) {
            a1 = 1.0f;
            a2 = 0.0f;
            a3 = 0.0f;
        }

        float32x4_t va1 = vdupq_n_f32(a1);
        float32x4_t va2 = vdupq_n_f32(a2);
        float32x4_t va3 = vdupq_n_f32(a3);
        float32x4_t vTwo = vdupq_n_f32(2.0f);

        for (uint32_t i = 0; i < numSamples; ++i) {
            // Load Stereo Sample into lanes [L, R, 0, 0]
            float32x4_t vIn = { inL[i], inR[i], 0.0f, 0.0f };

            // TPT SVF State Update (Stereo Parallel)
            float32x4_t v1 = vaddq_f32(vmulq_f32(va1, m_ic1), vmulq_f32(va2, vsubq_f32(vIn, m_ic2)));
            float32x4_t v2 = vaddq_f32(vaddq_f32(vmulq_f32(va2, m_ic1), vmulq_f32(va3, vsubq_f32(vIn, m_ic2))), m_ic2);
            
            m_ic1 = vsubq_f32(vmulq_f32(vTwo, v1), m_ic1);
            m_ic2 = vsubq_f32(vmulq_f32(vTwo, v2), m_ic2);

            // Store results (v2 is LP output)
            const float left = vgetq_lane_f32(v2, 0);
            const float right = vgetq_lane_f32(v2, 1);
            outL[i] = std::isfinite(left) ? left : 0.0f;
            outR[i] = std::isfinite(right) ? right : 0.0f;
        }
    }

    void setSampleRate(double sr) {
        if (std::isfinite(sr) && sr > 1000.0) {
            m_sampleRate = sr;
            reset();
        }
    }

private:
    double m_sampleRate;
    float32x4_t m_ic1, m_ic2; // Lanes 0=L, 1=R
};

} // namespace Aura::DSP::Mixing
