#import <Metal/Metal.h>
#include "gpu_audio_kernel.hpp"
#include "core/audio_buffer.hpp"
#include <iostream>
#include <deque>

namespace Aura::Core::GPU {

struct MetalAudioKernel::MetalSummingContext {
    id<MTLDevice> device;
    id<MTLCommandQueue> commandQueue;
    id<MTLComputePipelineState> summingPipeline;
    
    // --- INDUSTRIAL Hardening: DYNAMIC BUFFER CAPACITY ---
    uint32_t m_maxLen; 
    static constexpr uint8_t BUFFERS = 3;
    
    id<MTLBuffer> m_srcPool[BUFFERS];
    id<MTLBuffer> m_destPool[BUFFERS];
    dispatch_semaphore_t m_semaphore;
    uint8_t m_currentIdx = 0;

    void shutdown() {
        if (m_semaphore) {
            // Drain the pipeline with a safety timeout (Point 5)
            for (int i = 0; i < BUFFERS; ++i) {
                dispatch_semaphore_wait(m_semaphore, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_MSEC));
            }
        }
    }

    MetalSummingContext(uint32_t maxLen = 16384) : m_maxLen(maxLen) {
        device = MTLCreateSystemDefaultDevice();
        if (!device) return;
        commandQueue = [device newCommandQueue];
        if (!commandQueue) return;

        m_semaphore = dispatch_semaphore_create(BUFFERS);

        for (int i = 0; i < BUFFERS; ++i) {
            m_srcPool[i] = [device newBufferWithLength:m_maxLen * sizeof(float) options:MTLResourceStorageModeShared];
            m_destPool[i] = [device newBufferWithLength:m_maxLen * sizeof(float) options:MTLResourceStorageModeShared];
            if (!m_srcPool[i] || !m_destPool[i]) {
                std::cerr << "AURA | GPU: Failed to allocate Metal buffers.\n";
            }
        }

        NSString* shaderSource = @"\
            #include <metal_stdlib>\n\
            using namespace metal;\n\
            kernel void sumBuffers(const device float* src [[buffer(0)]],\n\
                                 device float* dest [[buffer(1)]],\n\
                                 uint id [[thread_position_in_grid]]) {\n\
                dest[id] += src[id];\n\
            }";
            
        NSError* error = nil;
        id<MTLLibrary> library = [device newLibraryWithSource:shaderSource options:nil error:&error];
        if (!library) {
            std::cerr << "AURA | GPU: Shader Error: " << [[error localizedDescription] UTF8String] << "\n";
            return;
        }
        id<MTLFunction> function = [library newFunctionWithName:@"sumBuffers"];
        if (!function) {
            std::cerr << "AURA | GPU: Missing sumBuffers kernel.\n";
            return;
        }
        summingPipeline = [device newComputePipelineStateWithFunction:function error:&error];
        if (!summingPipeline) {
             std::cerr << "AURA | GPU: Pipeline Error: " << [[error localizedDescription] UTF8String] << "\n";
        }
    }
};

void MetalAudioKernel::initialize() {
    getInstance();
}

MetalAudioKernel::MetalAudioKernel() {
    m_ctx = new MetalSummingContext();
}

MetalAudioKernel::~MetalAudioKernel() {
    if (m_ctx) {
        m_ctx->shutdown();
        delete m_ctx;
    }
}

void MetalAudioKernel::sumBuffers(float* dest, const float* src, uint32_t len) {
    if (!m_enabled || !dest || !src || !m_ctx) return;
    
    // --- STRICTURE: BOUNDS CHECKING (Point 1) ---
    if (len > m_ctx->m_maxLen) {
        static bool warned = false;
        if (!warned) { std::cerr << "AURA | GPU: Buffer size " << len << " exceeds GPU capacity " << m_ctx->m_maxLen << ". Falling back to CPU.\n"; warned = true; }
        for (uint32_t i = 0; i < len; ++i) dest[i] += src[i]; // CPU FALLBACK
        return;
    }
    
    if (!m_ctx->device || !m_ctx->summingPipeline) {
        for (uint32_t i = 0; i < len; ++i) dest[i] += src[i];
        return;
    }

    // --- PIPELINE THROTTLING: 1ms Safety Timeout (Point 5) ---
    long res = dispatch_semaphore_wait(m_ctx->m_semaphore, dispatch_time(DISPATCH_TIME_NOW, 1 * NSEC_PER_MSEC));
    if (res != 0) {
        // GPU is busy or hung. CRITICAL FALLBACK TO CPU TO PREVENT AUDIO HANG.
        for (uint32_t i = 0; i < len; ++i) dest[i] += src[i]; 
        return;
    }
    
    uint8_t idx = m_ctx->m_currentIdx;
    m_ctx->m_currentIdx = (idx + 1) % MetalSummingContext::BUFFERS;
    
    uint32_t byteLen = sizeof(float) * len;
    memcpy([m_ctx->m_srcPool[idx] contents], src, byteLen);
    memcpy([m_ctx->m_destPool[idx] contents], dest, byteLen);

    id<MTLCommandBuffer> cmdBuf = [m_ctx->commandQueue commandBuffer];
    id<MTLComputeCommandEncoder> encoder = [cmdBuf computeCommandEncoder];
    
    [encoder setComputePipelineState:m_ctx->summingPipeline];
    [encoder setBuffer:m_ctx->m_srcPool[idx] offset:0 atIndex:0];
    [encoder setBuffer:m_ctx->m_destPool[idx] offset:0 atIndex:1];
    
    [encoder dispatchThreads:MTLSizeMake(len, 1, 1) threadsPerThreadgroup:MTLSizeMake(std::min((uint32_t)32, len), 1, 1)];
    [encoder endEncoding];

    // --- ASYNCHRONOUS BACK-PROPAGATION ---
    __block float* blockDest = dest;
    __block id<MTLBuffer> blockDestBuf = m_ctx->m_destPool[idx];
    __block dispatch_semaphore_t blockSema = m_ctx->m_semaphore;

    [cmdBuf addCompletedHandler:^(id<MTLCommandBuffer> _Nonnull) {
        memcpy(blockDest, [blockDestBuf contents], byteLen);
        dispatch_semaphore_signal(blockSema);
    }];
    
    [cmdBuf commit];
}

void MetalAudioKernel::processFXChain(::Aura::Core::AudioBuffer* buffer, uint32_t len) {
    if (!buffer) return;
    sumBuffers(buffer->getWritePointer(0), buffer->getReadPointer(0), len);
}

} // namespace Aura::Core::GPU
