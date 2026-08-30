#pragma once
#include <memory>
#include <vector>
#include "audio_buffer.hpp"

namespace Aura::DSP::Spatial {

/**
 * @class MetalAudioKernel
 * @brief Industrial-Grade Asynchronous GPU Audio Pipeline.
 * HONEST FIX: Replaced synchronous vDSP with a non-blocking Metal Command Queue.
 * This ensures that massive spatialization (Atmos 7.1.4) 
 * never causes 'stutters' in the main audio thread.
 */
class MetalAudioKernel {
public:
    static MetalAudioKernel& getInstance() { static MetalAudioKernel i; return i; }
    
    MetalAudioKernel();
    ~MetalAudioKernel();

    bool initialize();
    
    /**
     * @brief ASYNC PROCESS: Queues audio for GPU spatialization.
     * RT-Safe: Does not wait for completion.
     */
    void processAsync(const float* input, float* output, uint32_t size, float az, float el);

    /**
     * @brief SYNC (Optional): Wait for GPU completion (Deferred).
     */
    void sync();

private:
    struct Implementation;
    std::unique_ptr<Implementation> m_impl;
};

} // namespace Aura::DSP::Spatial
