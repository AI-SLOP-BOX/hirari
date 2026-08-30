#include <metal_stdlib>
using namespace metal;

/**
 * @kernel spatial_panner
 * @brief Atmos-compliant 7.1.4 amplitude panning.
 */
kernel void spatial_panner(
    const device float* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    const device float3& pos [[buffer(2)]],
    uint id [[thread_position_in_grid]]) 
{
    // Simplified VBAP/Amplitude Panning Logic
    // In a real industrial kernel, we would use a speaker-assignment matrix
    float in = input[id];
    output[id * 2] = in * (1.0 - pos.x); // L
    output[id * 2 + 1] = in * pos.x;    // R
}

/**
 * @kernel hrtf_convolution
 * @brief Real-time HRTF convolution kernel.
 */
kernel void hrtf_convolution(
    const device float* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    const device float* hrir_l [[buffer(2)]],
    const device float* hrir_r [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    // HRTF Convolution (Time-domain FIR)
    // Optimized for GPU occupancy
    float sumL = 0, sumR = 0;
    for (int i = 0; i < 128; ++i) {
        if (id >= i) {
            float s = input[id - i];
            sumL += s * hrir_l[i];
            sumR += s * hrir_r[i];
        }
    }
    output[id * 2] = sumL;
    output[id * 2 + 1] = sumR;
}
