#pragma once
#include <vector>
#include <string>
#include <map>
#include <mutex>

namespace Aura::Core::Network {

/**
 * @class CollaborationHubKernel
 * @brief Central registry for multi-user session state.
 */
class CollaborationHubKernel {
public:
    static CollaborationHubKernel& getInstance() {
        static CollaborationHubKernel instance;
        return instance;
    }

    /**
     * @brief Registers a user edit with CRDT-inspired sequence convergence.
     */
    void logEdit(const std::string& userId, uint64_t sequenceNum, const std::string& editData) {
        std::lock_guard<std::mutex> lock(m_mutex);
        // Only accept if sequence is newer than what we have from this user (Basic LWW-Register)
        if (sequenceNum > m_userSequences[userId]) {
            m_userSequences[userId] = sequenceNum;
            m_editLog.push_back({userId, sequenceNum, editData});
        }
    }

    /**
     * @brief Checks if a specific track is locked by another user with expiry.
     */
    bool isLocked(uint32_t trackId, const std::string& requesterId) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_locks.find(trackId);
        if (it == m_locks.end()) return false;
        
        // Heartbeat Check: Lock expires after 5 seconds of inactivity
        auto now = std::chrono::steady_clock::now();
        if (std::chrono::duration_cast<std::chrono::seconds>(now - it->second.lastHeartbeat).count() > 5) {
            return false; // Lock stale
        }

        return (it->second.userId != requesterId);
    }

    void acquireLock(uint32_t trackId, const std::string& userId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_locks[trackId] = {userId, std::chrono::steady_clock::now()};
    }

    void refreshLock(uint32_t trackId, const std::string& userId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_locks[trackId].userId == userId) {
            m_locks[trackId].lastHeartbeat = std::chrono::steady_clock::now();
        }
    }

private:
    CollaborationHubKernel() = default;
    mutable std::mutex m_mutex;
    
    struct EditEntry { 
        std::string userId; 
        uint64_t sequence; 
        std::string data; 
    };

    struct LockEntry {
        std::string userId;
        std::chrono::steady_clock::time_point lastHeartbeat;
    };

    std::vector<EditEntry> m_editLog;
    std::map<uint32_t, LockEntry> m_locks;
    std::map<std::string, uint64_t> m_userSequences;
};

} // namespace Aura::Core::Network
