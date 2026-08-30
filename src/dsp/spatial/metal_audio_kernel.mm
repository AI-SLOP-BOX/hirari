#import <Metal/Metal.h>
#include "metal_audio_kernel.hpp"
#include <iostream>
#include <dispatch/dispatch.h>

namespace Aura::DSP::Spatial {

static constexpr int kMaxInflightBuffers = 3;

struct MetalAudioKernel::Implementation {
    id<MTLDevice> device;
    id<MTLCommandQueue> queue;
    id<MTLComputePipelineState> pipeline;
    
    // Triple-Buffering
    id<MTLBuffer> inputBuffers[kMaxInflightBuffers];
    id<MTLBuffer> outputBuffers[kMaxInflightBuffers];
    id<MTLBuffer> paramsBuffers[kMaxInflightBuffers];
    dispatch_semaphore_t semaphore;
    uint32_t currentBufferIndex = 0;

    Implementation() {
        device = MTLCreateSystemDefaultDevice();
        queue = [device newCommandQueue];
        semaphore = dispatch_semaphore_create(kMaxInflightBuffers);
        
        // Load MSL Kernel
        NSError* error = nil;
        id<MTLLibrary> library = [device newDefaultLibrary];
        if (!library) {
            pipeline = nil;
            return;
        }
        id<MTLFunction> function = [library newFunctionWithName:@"spatial_panner"];
        if (!function) {
            pipeline = nil;
            return;
        }
        pipeline = [device newComputePipelineStateWithFunction:function error:&error];
        
        // Pre-allocate Shared Memory (RT-Safe)
        for(int i=0; i<kMaxInflightBuffers; ++i) {
            inputBuffers[i] = [device newBufferWithLength:4096 * sizeof(float) options:MTLResourceStorageModeShared];
            outputBuffers[i] = [device newBufferWithLength:4096 * 2 * sizeof(float) options:MTLResourceStorageModeShared];
            paramsBuffers[i] = [device newBufferWithLength:sizeof(float) * 3 options:MTLResourceStorageModeShared];
        }
    }
};

MetalAudioKernel::MetalAudioKernel() : m_impl(nullptr) {}
MetalAudioKernel::~MetalAudioKernel() = default;

bool MetalAudioKernel::initialize() {
    try {
        m_impl = std::make_unique<Implementation>();
        return m_impl->device != nil;
    } catch (...) {
        return false;
    }
}

void MetalAudioKernel::processAsync(const float* input, float* output, uint32_t size, float az, float el) {
    if (!m_impl || !m_impl->pipeline || !input || !output || size == 0 || size > 4096) return;

    // 1. Wait for In-flight Buffer (Backpressure)
    dispatch_semaphore_wait(m_impl->semaphore, DISPATCH_TIME_FOREVER);
    
    uint32_t idx = m_impl->currentBufferIndex;
    
    // 2. Zero-Copy Data Mapping
    std::memcpy([m_impl->inputBuffers[idx] contents], input, size * sizeof(float));
    float params[3] = {az, el, 0.0f};
    std::memcpy([m_impl->paramsBuffers[idx] contents], params, sizeof(params));

    // 3. Encode & Dispatch
    id<MTLCommandBuffer> commandBuffer = [m_impl->queue commandBuffer];
    id<MTLComputeCommandEncoder> encoder = [commandBuffer computeCommandEncoder];
    
    [encoder setComputePipelineState:m_impl->pipeline];
    [encoder setBuffer:m_impl->inputBuffers[idx] offset:0 atIndex:0];
    [encoder setBuffer:m_impl->outputBuffers[idx] offset:0 atIndex:1];
    [encoder setBuffer:m_impl->paramsBuffers[idx] offset:0 atIndex:2];
    
    MTLSize gridSize = MTLSizeMake(size, 1, 1);
    MTLSize threadGroupSize = MTLSizeMake(std::min((uint32_t)size, (uint32_t)64), 1, 1);
    [encoder dispatchThreads:gridSize threadsPerThreadgroup:threadGroupSize];
    [encoder endEncoding];

    // 4. Completion Handler
    __block float* outPtr = output;
    __block id<MTLBuffer> outBuf = m_impl->outputBuffers[idx];
    __block dispatch_semaphore_t sem = m_impl->semaphore;
    __block uint32_t outSize = size * 2;

    [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer> _Nonnull) {
        std::memcpy(outPtr, [outBuf contents], outSize * sizeof(float));
        dispatch_semaphore_signal(sem);
    }];

    [commandBuffer commit];
    
    m_impl->currentBufferIndex = (idx + 1) % kMaxInflightBuffers;
}

void MetalAudioKernel::sync() {
    // In a real Atmos buss, this might wait for the last buffer
}

} // namespace Aura::DSP::Spatial
