#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <mutex>
#include <map>
#include <set>
#include <unordered_map>

namespace Aura::Core::Engine {

/**
 * @struct UserChange
 * @brief Representation of a remote project change for CRDT sync.
 */
struct UserChange {
    uint64_t timestamp;
    uint32_t userId;
    uint32_t targetId; // Track, Region, or Parameter ID
    float newValue;
    char meta[128];
};

/**
 * @struct Commit
 * @brief Git-style immutable transaction commit.
 * Part of the Immutable Timeline Commit History DAG.
 */
struct Commit {
    std::string hash;              // Unique SHA-like hash identifying this commit
    std::string parentHash;        // Primary parent commit hash
    std::string mergeParentHash;   // Optional secondary merge parent hash (for 3-way merges)
    uint64_t timestamp;            // Commit timestamp
    uint32_t userId;               // User who created the transaction
    std::string description;       // Description of the action
    std::vector<UserChange> deltas; // Delta changes introduced by this transaction
};

/**
 * @class CloudSyncOrchestrator
 * @brief Multi-user collaboration engine.
 * Leverages Git-style immutable commit graphs and LWW-CRDT conflict resolution.
 */
class CloudSyncOrchestrator {
public:
    CloudSyncOrchestrator();

    struct RemoteUser {
        uint32_t id;
        char name[64];
        bool isActive;
    };

    // --- CONNECTION ---
    bool connect(const std::string& serverUrl, const std::string& sessionId);
    void disconnect();
    bool isConnected() const noexcept { return m_isSyncing.load(std::memory_order_acquire); }
    std::string serverUrl() const;
    std::string sessionId() const;
    void upsertRemoteUser(const RemoteUser& user);
    void removeRemoteUser(uint32_t userId);
    bool hasRemoteUser(uint32_t userId) const;

    // --- STATE SYNCHRONIZATION ---
    void pushChange(const UserChange& change);
    void pullChanges(std::vector<UserChange>& out);
    void resolveConflicts();
    size_t pendingChangeCount() const;

    // --- GIT-STYLE IMMUTABLE COMMIT DAG & 3-WAY MERGE ---
    std::string commitState(uint32_t userId, const std::string& desc, const std::vector<UserChange>& changes);
    bool mergeBranch(const std::string& remoteHeadHash);
    std::string getHeadHash() const;
    // The returned copy is safe after the internal mutex is released.
    bool copyCommit(const std::string& hash, Commit& destination) const;
    [[deprecated("Use copyCommit; returned pointers are invalidated by later edits")]]
    const Commit* getCommit(const std::string& hash) const;
    // --- USER MANAGEMENT ---
    std::vector<RemoteUser> getActiveUsers() const;

private:
    std::string calculateHash(const std::string& parentHash, const std::string& mergeParentHash, const std::vector<UserChange>& deltas, uint64_t ts, uint32_t uid);
    std::string findLowestCommonAncestor(const std::string& localHash, const std::string& remoteHash);

    std::atomic<bool> m_isSyncing;
    std::vector<UserChange> m_pendingChanges;
    std::map<uint32_t, RemoteUser> m_remoteUsers;
    
    // Git/CRDT DAG Storage
    std::unordered_map<std::string, Commit> m_commits;
    std::string m_headCommitHash;
    std::string m_serverUrl;
    std::string m_sessionId;
    
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Engine
