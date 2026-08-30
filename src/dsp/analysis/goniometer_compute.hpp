#pragma once
#include <vector>
#include <Metal/Metal.h>
#include <Foundation/Foundation.h>
#include <algorithm>
#include <string>
#include <atomic>

namespace Aura::DSP::Analysis {

class GoniometerCompute {
public:
    struct Point { float x; float y; };

    GoniometerCompute() : m_pipelineState(nil) {
        m_device = MTLCreateSystemDefaultDevice();
        if (m_device) {
            m_commandQueue = [m_device newCommandQueue];
        }

        m_numPoints.store(0, std::memory_order_relaxed);
        for (size_t i = 0; i < 1024; ++i) {
            m_atomicPointsX[i].store(0.0f, std::memory_order_relaxed);
            m_atomicPointsY[i].store(0.0f, std::memory_order_relaxed);
        }

        // Pre-allocate GPU buffers for up to 1024 samples (max block size) to ensure RT-safety
        if (m_device) {
            m_inputBufferL = [m_device newBufferWithLength:1024 * sizeof(float)
                                                   options:MTLResourceStorageModeShared];
            m_inputBufferR = [m_device newBufferWithLength:1024 * sizeof(float)
                                                   options:MTLResourceStorageModeShared];
            m_pointsBuffer = [m_device newBufferWithLength:1024 * sizeof(float) * 2
                                                   options:MTLResourceStorageModeShared];
        }
        
        m_shaderSource = R"(
            #include <metal_stdlib>
            using namespace metal;

            kernel void computeLissajous(
                device const float* l [[buffer(0)]],
                device const float* r [[buffer(1)]],
                device float2* points [[buffer(2)]],
                uint id [[thread_position_in_grid]]) 
            {
                float mid = (l[id] + r[id]) * 0.5f;
                float side = (l[id] - r[id]) * 0.5f;
                points[id] = float2(side, mid);
            }
        )";
        setupPipeline();
    }

    void analyze(const float* l, const float* r, uint32_t samples) {
        if (!m_device || !m_pipelineState || !m_inputBufferL || !m_inputBufferR || !m_pointsBuffer || samples == 0) return;
        
        uint32_t limit = std::min(samples, 1024u);
        
        // Copy to pre-allocated buffers on the CPU side (Shared storage mode)
        float* pL = static_cast<float*>([m_inputBufferL contents]);
        float* pR = static_cast<float*>([m_inputBufferR contents]);
        std::copy(l, l + limit, pL);
        std::copy(r, r + limit, pR);
        
        id<MTLCommandBuffer> commandBuffer = [m_commandQueue commandBuffer];
        id<MTLComputeCommandEncoder> commandEncoder = [commandBuffer computeCommandEncoder];
        
        [commandEncoder setComputePipelineState:m_pipelineState];
        [commandEncoder setBuffer:m_inputBufferL offset:0 atIndex:0];
        [commandEncoder setBuffer:m_inputBufferR offset:0 atIndex:1];
        [commandEncoder setBuffer:m_pointsBuffer offset:0 atIndex:2];
        
        MTLSize gridSize = MTLSizeMake(limit, 1, 1);
        NSUInteger threadGroupSizeVal = std::min(static_cast<NSUInteger>(limit), m_pipelineState.maxTotalThreadsPerThreadgroup);
        MTLSize threadGroupSize = MTLSizeMake(threadGroupSizeVal, 1, 1);
        
        [commandEncoder dispatchThreads:gridSize threadsPerThreadgroup:threadGroupSize];
        [commandEncoder endEncoding];
        
        // Asynchronous completion handler to pull data back in background thread
        id<MTLBuffer> outPoints = m_pointsBuffer;
        auto* pAtomicX = m_atomicPointsX;
        auto* pAtomicY = m_atomicPointsY;
        auto* pNumPoints = &m_numPoints;

        [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer> /*cb*/) {
            struct float2 { float x; float y; };
            float2* resultData = static_cast<float2*>([outPoints contents]);
            pNumPoints->store(limit, std::memory_order_relaxed);
            for (uint32_t i = 0; i < limit; ++i) {
                pAtomicX[i].store(resultData[i].x, std::memory_order_relaxed);
                pAtomicY[i].store(resultData[i].y, std::memory_order_relaxed);
            }
        }];

        [commandBuffer commit];
    }

    std::vector<Point> getLatestPoints() const {
        uint32_t count = m_numPoints.load(std::memory_order_relaxed);
        std::vector<Point> res(count);
        for (uint32_t i = 0; i < count; ++i) {
            res[i].x = m_atomicPointsX[i].load(std::memory_order_relaxed);
            res[i].y = m_atomicPointsY[i].load(std::memory_order_relaxed);
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
        id<MTLFunction> function = [library newFunctionWithName:@"computeLissajous"];
        if (!function) return;
        m_pipelineState = [m_device newComputePipelineStateWithFunction:function error:&error];
    }

    id<MTLDevice> m_device;
    id<MTLCommandQueue> m_commandQueue;
    id<MTLComputePipelineState> m_pipelineState;
    std::string m_shaderSource;

    id<MTLBuffer> m_inputBufferL;
    id<MTLBuffer> m_inputBufferR;
    id<MTLBuffer> m_pointsBuffer;

    // Lock-free data cache for thread safety
    mutable std::atomic<uint32_t> m_numPoints;
    mutable std::atomic<float> m_atomicPointsX[1024];
    mutable std::atomic<float> m_atomicPointsY[1024];
};

} // namespace Aura::DSP::Analysis
