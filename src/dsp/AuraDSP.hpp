#if defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#elif defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace Aura::DSP {

    /**
     * @brief Limiter: SIMD-optimized single-block lookahead limiter.
     */
    struct Limiter {
        float gain = 1.0f;
        
        void process(::Aura::Core::AudioBuffer& b, uint32_t n) {
            uint32_t chans = b.getNumChannels();
            if (chans < 2) return; // Stereo expected for this fast-path

            const float* srcL = b.getReadPointer(0);
            const float* srcR = b.getReadPointer(1);
            float* dstL = b.getWritePointer(0);
            float* dstR = b.getWritePointer(1);

            uint32_t i = 0;

#if defined(__arm64__) || defined(__aarch64__)
            for (; i + 3 < n; i += 4) {
                float32x4_t vL = vld1q_f32(srcL + i);
                float32x4_t vR = vld1q_f32(srcR + i);
                
                // max(abs(L), abs(R))
                float32x4_t vAbsL = vabsq_f32(vL);
                float32x4_t vAbsR = vabsq_f32(vR);
                float32x4_t vMax = vmaxq_f32(vAbsL, vAbsR);
                
                // Target gain computation (approximate vectorized version)
                // For a proper limiter, we might want a scalar loop for gain smoothing 
                // if it depends on previous samples, but we can vectorize the peak check.
                for (int k = 0; k < 4; ++k) {
                    float sampleMax = vMax[k];
                    float target = (sampleMax > 0.99f) ? 0.99f / sampleMax : 1.0f;
                    if (target < gain) gain = target;
                    else gain += (target - gain) * 0.001f;
                    
                    dstL[i+k] = srcL[i+k] * gain;
                    dstR[i+k] = srcR[i+k] * gain;
                }
            }
#elif defined(__x86_64__) || defined(_M_X64)
            for (; i + 3 < n; i += 4) {
                __m128 vL = _mm_loadu_ps(srcL + i);
                __m128 vR = _mm_loadu_ps(srcR + i);
                __m128 vAbsL = _mm_and_ps(vL, _mm_castsi128_ps(_mm_set1_epi32(0x7FFFFFFF)));
                __m128 vAbsR = _mm_and_ps(vR, _mm_castsi128_ps(_mm_set1_epi32(0x7FFFFFFF)));
                __m128 vMax = _mm_max_ps(vAbsL, vAbsR);
                
                float peaks[4];
                _mm_storeu_ps(peaks, vMax);
                for (int k = 0; k < 4; ++k) {
                    float target = (peaks[k] > 0.99f) ? 0.99f / peaks[k] : 1.0f;
                    if (target < gain) gain = target;
                    else gain += (target - gain) * 0.001f;
                    dstL[i+k] = srcL[i+k] * gain;
                    dstR[i+k] = srcR[i+k] * gain;
                }
            }
#endif

            // Scalar fallback
            for (; i < n; ++i) {
                float maxAmp = std::max(std::abs(srcL[i]), std::abs(srcR[i]));
                float target = (maxAmp > 0.99f) ? 0.99f / maxAmp : 1.0f;
                if (target < gain) gain = target;
                else gain += (target - gain) * 0.001f;
                dstL[i] = srcL[i] * gain;
                dstR[i] = srcR[i] * gain;
            }
        }
    };
}
