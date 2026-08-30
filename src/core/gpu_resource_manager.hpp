#pragma once
#include <atomic>
#include <mutex>
#include <vector>
#include <map>
#include <array>

namespace Aura::Core::BridgeFFI {

/**
 * @class GpuResourceManager
 * @brief RT-Safe Sovereign GPU Buffer Orchestrator for the FFI layer.
 * Migrated to FFI namespace to prevent symbol collisions with logic core types.
 */
class GpuResourceManager {
public:
    static GpuResourceManager& getInstance() { static GpuResourceManager i; return i; }

    void release_gpu_resource(uint64_t handle) const {
        const_cast<GpuResourceManager*>(this)->queue_release(handle);
    }

    void process_deferred_releases() const {
        const_cast<GpuResourceManager*>(this)->process_reclamations();
    }

    void queue_release(uint64_t handle) {
        uint32_t writeIdx = m_writePtr.fetch_add(1) % kQueueSize;
        m_reclaimQueue[writeIdx].handle.store(handle, std::memory_order_release);
        m_reclaimQueue[writeIdx].framesRemaining.store(3, std::memory_order_release);
    }

    void process_reclamations() {
        uint32_t readEnd = m_writePtr.load(std::memory_order_acquire);
        for (uint32_t i = m_readPtr; i != (readEnd % kQueueSize); i = (i + 1) % kQueueSize) {
            auto& item = m_reclaimQueue[i];
            uint32_t frames = item.framesRemaining.load(std::memory_order_acquire);
            
            if (frames > 0) {
                item.framesRemaining.store(frames - 1, std::memory_order_release);
            } else {
                uint64_t handle = item.handle.exchange(0, std::memory_order_acq_rel);
                m_readPtr = (i + 1) % kQueueSize;
            }
        }
    }

private:
    GpuResourceManager() {
        for(auto& item : m_reclaimQueue) { item.handle = 0; item.framesRemaining = 0; }
    }
    
    struct ReclaimItem {
        std::atomic<uint64_t> handle;
        std::atomic<uint32_t> framesRemaining;
    };
    
    static constexpr uint32_t kQueueSize = 1024;
    std::array<ReclaimItem, kQueueSize> m_reclaimQueue;
    std::atomic<uint32_t> m_readPtr{0}, m_writePtr{0};

    std::mutex m_poolMutex;
    std::map<size_t, std::vector<uint64_t>> m_bufferPool;
};

inline const GpuResourceManager& get_gpu_manager() { return GpuResourceManager::getInstance(); }

} // namespace Aura::Core::BridgeFFI
