#pragma once
#include <vector>
#include <Metal/Metal.h>
#include <Foundation/Foundation.h>
#include <algorithm>
#include <string>
#include <atomic>
#include <cmath>

namespace Aura::DSP::Analysis {

class GPUSpectrumAnalyzer {
public:
    static constexpr size_t kFFTSize = 2048;

    GPUSpectrumAnalyzer() : m_pipelineState(nil) {
        m_device = MTLCreateSystemDefaultDevice();
        if (m_device) {
            m_commandQueue = [m_device newCommandQueue];
        }

        // Initialize atomic spectrum bins to silence
        for (size_t i = 0; i < kFFTSize / 2; ++i) {
            m_atomicMagnitude[i].store(-100.0f, std::memory_order_relaxed);
        }

        // Pre-allocate GPU Shared Buffers to ensure RT-safety (no allocations in process loop)
        if (m_device) {
            m_inputBuffer = [m_device newBufferWithLength:kFFTSize * sizeof(float)
                                                  options:MTLResourceStorageModeShared];
            m_outputBuffer = [m_device newBufferWithLength:(kFFTSize / 2) * sizeof(float)
                                                   options:MTLResourceStorageModeShared];
        }
        
        m_shaderSource = R"(
            #include <metal_stdlib>
            using namespace metal;

            kernel void computeFFT(
                device const float* samples [[buffer(0)]],
                device float* magnitude [[buffer(1)]],
                uint local_id [[thread_index_in_threadgroup]]) 
            {
                // Parallel Cooley-Tukey Radix-2 FFT inside threadgroup
                threadgroup float tg_real[2048];
                threadgroup float tg_imag[2048];

                uint idx0 = local_id;
                uint idx1 = local_id + 1024;

                tg_real[idx0] = samples[idx0];
                tg_imag[idx0] = 0.0;
                tg_real[idx1] = samples[idx1];
                tg_imag[idx1] = 0.0;

                threadgroup_barrier(mem_flags::mem_threadgroup);

                // Helper lambda for bit reversal
                uint val0 = idx0;
                uint rev0 = 0;
                for (int i = 0; i < 11; ++i) {
                    rev0 = (rev0 << 1) | (val0 & 1);
                    val0 >>= 1;
                }

                uint val1 = idx1;
                uint rev1 = 0;
                for (int i = 0; i < 11; ++i) {
                    rev1 = (rev1 << 1) | (val1 & 1);
                    val1 >>= 1;
                }

                float temp_r0 = tg_real[idx0];
                float temp_r1 = tg_real[idx1];

                threadgroup_barrier(mem_flags::mem_threadgroup);

                tg_real[rev0] = temp_r0;
                tg_imag[rev0] = 0.0;
                tg_real[rev1] = temp_r1;
                tg_imag[rev1] = 0.0;

                threadgroup_barrier(mem_flags::mem_threadgroup);

                // Cooley-Tukey butterfly stages
                for (uint size = 2; size <= 2048; size <<= 1) {
                    uint half_size = size >> 1;
                    float angle = -2.0 * 3.141592653589793 * (local_id % half_size) / size;
                    float c = cos(angle);
                    float s = sin(angle);

                    uint step = local_id / half_size;
                    uint base = step * size + (local_id % half_size);

                    uint even_idx = base;
                    uint odd_idx = base + half_size;

                    float r_odd = tg_real[odd_idx];
                    float i_odd = tg_imag[odd_idx];

                    float t_r = r_odd * c - i_odd * s;
                    float t_i = r_odd * s + i_odd * c;

                    float r_even = tg_real[even_idx];
                    float i_even = tg_imag[even_idx];

                    threadgroup_barrier(mem_flags::mem_threadgroup);

                    tg_real[even_idx] = r_even + t_r;
                    tg_imag[even_idx] = i_even + t_i;
                    tg_real[odd_idx]  = r_even - t_r;
                    tg_imag[odd_idx]  = i_even - t_i;

                    threadgroup_barrier(mem_flags::mem_threadgroup);
                }

                if (local_id < 1024) {
                    float r = tg_real[local_id];
                    float i = tg_imag[local_id];
                    float mag = sqrt(r * r + i * i) / 1024.0;
                    magnitude[local_id] = 20.0 * log10(max(mag, 1e-6f));
                }
            }
        )";
        setupPipeline();
    }

    void analyze(const float* l, const float* r, uint32_t samples) {
        if (!m_device || !m_pipelineState || !m_inputBuffer || !m_outputBuffer || samples == 0) return;
        
        uint32_t N = kFFTSize;
        float* pIn = static_cast<float*>([m_inputBuffer contents]);
        
        // Zero-fill and load samples
        uint32_t limit = std::min(samples, static_cast<uint32_t>(N));
        for (uint32_t i = 0; i < limit; ++i) {
            pIn[i] = (l[i] + (r ? r[i] : l[i])) * 0.5f; 
        }
        for (uint32_t i = limit; i < N; ++i) {
            pIn[i] = 0.0f;
        }
        
        id<MTLCommandBuffer> commandBuffer = [m_commandQueue commandBuffer];
        id<MTLComputeCommandEncoder> commandEncoder = [commandBuffer computeCommandEncoder];
        
        [commandEncoder setComputePipelineState:m_pipelineState];
        [commandEncoder setBuffer:m_inputBuffer offset:0 atIndex:0];
        [commandEncoder setBuffer:m_outputBuffer offset:0 atIndex:1];
        
        // Dispatch exactly 1 threadgroup containing 1024 threads so they share threadgroup memory
        MTLSize gridSize = MTLSizeMake(1024, 1, 1);
        MTLSize threadGroupSize = MTLSizeMake(1024, 1, 1);
        
        [commandEncoder dispatchThreads:gridSize threadsPerThreadgroup:threadGroupSize];
        [commandEncoder endEncoding];
        
        // Asynchronous completion handler: updates atomic registers once GPU computation completes
        id<MTLBuffer> outBuf = m_outputBuffer;
        auto* atomicMagnitudes = m_atomicMagnitude;
        [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer> /*cb*/) {
            float* resultData = static_cast<float*>([outBuf contents]);
            for (size_t i = 0; i < kFFTSize / 2; ++i) {
                atomicMagnitudes[i].store(resultData[i], std::memory_order_relaxed);
            }
        }];

        [commandBuffer commit];
    }

    std::vector<float> getLatestSpectrum() const {
        std::vector<float> res(kFFTSize / 2);
        for (size_t i = 0; i < kFFTSize / 2; ++i) {
            res[i] = m_atomicMagnitude[i].load(std::memory_order_relaxed);
        }
        return res;
    }

private:
    void setupPipeline() {
        if (!m_device) return;
        NSError* error = nil;
        NSString* sourceStr = [NSString stringWithUTF8String:m_shaderSource.c_str()];
        MTLCompileOptions* options = [MTLCompileOptions new];
        id<MTLLibrary> library = [m_device newLibraryWithSource:sourceStr options:options error:&error];
        if (!library) {
            return;
        }
        id<MTLFunction> function = [library newFunctionWithName:@"computeFFT"];
        if (!function) return;
        m_pipelineState = [m_device newComputePipelineStateWithFunction:function error:&error];
    }

    id<MTLDevice> m_device;
    id<MTLCommandQueue> m_commandQueue;
    id<MTLComputePipelineState> m_pipelineState;
    std::string m_shaderSource;

    id<MTLBuffer> m_inputBuffer;
    id<MTLBuffer> m_outputBuffer;

    // Lock-free data cache for thread safety
    mutable std::atomic<float> m_atomicMagnitude[kFFTSize / 2];
};

} // namespace Aura::DSP::Analysis
