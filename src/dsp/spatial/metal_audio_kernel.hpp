#pragma once
#include <memory>
#include <mutex>
#include <vector>
#include "audio_buffer.hpp"

namespace Hirari::DSP::Spatial {

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

    /// True once `initialize()` has built a usable device/pipeline.
    bool isInitialized() const;
    
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
    // Guards `m_impl` construction and teardown. Every engine construction
    // (`HirariCore::new_with_mode`) calls `initialize()`, so without this lock
    // two concurrent constructions raced on the singleton's `unique_ptr`:
    // thread B assigned a fresh `Implementation` while thread A's assignment
    // destroyed the objects A had already published, aborting the process in
    // `_os_object_release_without_xref_dispose` (SIGTRAP, observed in the UI
    // test suite at >= 8 concurrent engine constructions).
    mutable std::mutex m_initMutex;
    // Guards `m_impl` *use* (`processAsync`), separately from construction so
    // a slow Metal setup on the control thread never blocks the audio thread
    // for the duration of the init. `processAsync` only copies into buffers
    // and commits a command buffer, so the critical section stays short.
    mutable std::mutex m_useMutex;
};

} // namespace Hirari::DSP::Spatial
