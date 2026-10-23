#pragma once

#include <cstdint>
#include <string>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Network {

/** Thread-safe native API for the Rust collaboration session registry. */
class CollaborationHubKernel {
public:
    static CollaborationHubKernel& getInstance() {
        static CollaborationHubKernel instance;
        return instance;
    }
    ~CollaborationHubKernel() { hirari_collaboration_hub_destroy(m_state); }
    CollaborationHubKernel(const CollaborationHubKernel&) = delete;
    CollaborationHubKernel& operator=(const CollaborationHubKernel&) = delete;
    CollaborationHubKernel(CollaborationHubKernel&&) = delete;
    CollaborationHubKernel& operator=(CollaborationHubKernel&&) = delete;

    void logEdit(const std::string& userId, uint64_t sequenceNum, const std::string& editData) {
        hirari_collaboration_hub_log_edit(m_state,
            reinterpret_cast<const uint8_t*>(userId.data()), userId.size(), sequenceNum,
            reinterpret_cast<const uint8_t*>(editData.data()), editData.size());
    }
    bool isLocked(uint32_t trackId, const std::string& requesterId) const {
        return hirari_collaboration_hub_is_locked(m_state, trackId,
            reinterpret_cast<const uint8_t*>(requesterId.data()), requesterId.size());
    }
    void acquireLock(uint32_t trackId, const std::string& userId) {
        hirari_collaboration_hub_acquire(m_state, trackId,
            reinterpret_cast<const uint8_t*>(userId.data()), userId.size());
    }
    void refreshLock(uint32_t trackId, const std::string& userId) {
        hirari_collaboration_hub_refresh(m_state, trackId,
            reinterpret_cast<const uint8_t*>(userId.data()), userId.size());
    }

private:
    CollaborationHubKernel() : m_state(hirari_collaboration_hub_create()) {}
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Network
