#include "cloud_sync_orchestrator.hpp"
#include "../log_buffer.hpp"
#include <algorithm>
#include <cstdio>
#include <chrono>
#include <cmath>
#include <cstring>

namespace Aura::Core::Engine {

CloudSyncOrchestrator::CloudSyncOrchestrator() : m_isSyncing(false) {
    // Initialize root commit
    Commit root;
    root.hash = "0000000000000000000000000000000000000000000000000000000000000000";
    root.parentHash = "";
    root.timestamp = 0;
    root.userId = 0;
    root.description = "Root Commit: Initial State";
    
    m_commits[root.hash] = root;
    m_headCommitHash = root.hash;
}

bool CloudSyncOrchestrator::connect(const std::string& serverUrl, const std::string& sessionId) {
    if (serverUrl.empty() || sessionId.empty()) return false;
    {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_serverUrl = serverUrl;
        m_sessionId = sessionId;
    }
    m_isSyncing.store(true);
    return true;
}

void CloudSyncOrchestrator::disconnect() {
    m_isSyncing.store(false);
    std::lock_guard<std::mutex> lock(m_mutex);
    m_serverUrl.clear();
    m_sessionId.clear();
}

std::string CloudSyncOrchestrator::serverUrl() const {
    std::lock_guard<std::mutex> lock(m_mutex);
    return m_serverUrl;
}

std::string CloudSyncOrchestrator::sessionId() const {
    std::lock_guard<std::mutex> lock(m_mutex);
    return m_sessionId;
}

void CloudSyncOrchestrator::pushChange(const UserChange& change) {
    if (change.targetId == 0 || !std::isfinite(change.newValue)) return;
    std::lock_guard<std::mutex> lock(m_mutex);
    for (auto it = m_pendingChanges.begin(); it != m_pendingChanges.end();) {
        if (it->targetId == change.targetId && it->userId == change.userId &&
            it->timestamp <= change.timestamp) it = m_pendingChanges.erase(it);
        else ++it;
    }
    m_pendingChanges.push_back(change);
}

size_t CloudSyncOrchestrator::pendingChangeCount() const {
    std::lock_guard<std::mutex> lock(m_mutex);
    return m_pendingChanges.size();
}

void CloudSyncOrchestrator::upsertRemoteUser(const RemoteUser& user) {
    if (user.id == 0) return;
    std::lock_guard<std::mutex> lock(m_mutex);
    m_remoteUsers[user.id] = user;
}

void CloudSyncOrchestrator::removeRemoteUser(uint32_t userId) {
    std::lock_guard<std::mutex> lock(m_mutex);
    m_remoteUsers.erase(userId);
}

bool CloudSyncOrchestrator::hasRemoteUser(uint32_t userId) const {
    std::lock_guard<std::mutex> lock(m_mutex);
    return m_remoteUsers.find(userId) != m_remoteUsers.end();
}

void CloudSyncOrchestrator::pullChanges(std::vector<UserChange>& out) {
    std::lock_guard<std::mutex> lock(m_mutex);
    out = m_pendingChanges;
    m_pendingChanges.clear();
}

void CloudSyncOrchestrator::resolveConflicts() {
    std::lock_guard<std::mutex> lock(m_mutex);
    
    // Sort changes by timestamp (ascending) to apply oldest first
    std::sort(m_pendingChanges.begin(), m_pendingChanges.end(), [](const auto& a, const auto& b) {
        return a.timestamp < b.timestamp;
    });

    // LWW (Last-Writer-Wins) CRDT: Keep only the latest update per targetId
    std::map<uint32_t, UserChange> latestChanges;
    for (const auto& change : m_pendingChanges) {
        latestChanges[change.targetId] = change;
    }

    m_pendingChanges.clear();
    for (const auto& pair : latestChanges) {
        m_pendingChanges.push_back(pair.second);
        // RT-Safe: replace std::cout print with diagnostics log
        ::Aura::Core::Diagnostics::LogBuffer::post(pair.second.userId, pair.second.targetId, "CRDT_RESOLVED");
    }
}

std::string CloudSyncOrchestrator::commitState(uint32_t userId, const std::string& desc, const std::vector<UserChange>& changes) {
    std::lock_guard<std::mutex> lock(m_mutex);
    
    uint64_t ts = std::chrono::steady_clock::now().time_since_epoch().count();
    std::string newHash = calculateHash(m_headCommitHash, "", changes, ts, userId);
    
    Commit c;
    c.hash = newHash;
    c.parentHash = m_headCommitHash;
    c.timestamp = ts;
    c.userId = userId;
    c.description = desc;
    c.deltas = changes;
    
    m_commits[newHash] = c;
    m_headCommitHash = newHash;
    
    return newHash;
}

bool CloudSyncOrchestrator::mergeBranch(const std::string& remoteHeadHash) {
    std::lock_guard<std::mutex> lock(m_mutex);
    
    // 1. Trace branches and find Lowest Common Ancestor (LCA)
    std::string lcaHash = findLowestCommonAncestor(m_headCommitHash, remoteHeadHash);
    if (lcaHash.empty()) {
        return false; // Disconnected graphs cannot be merged
    }
    
    if (lcaHash == remoteHeadHash) {
        // Fast-Forward: remote is already merged into local head
        return true;
    }
    
    if (lcaHash == m_headCommitHash) {
        // Fast-Forward: local branch is behind, update head pointer directly
        m_headCommitHash = remoteHeadHash;
        return true;
    }
    
    // 2. 3-Way Merge Extraction
    // Extract local changes from LCA to local head
    std::map<uint32_t, UserChange> localModifications;
    std::string current = m_headCommitHash;
    while (current != lcaHash && !current.empty()) {
        const auto& commit = m_commits[current];
        for (const auto& change : commit.deltas) {
            if (localModifications.find(change.targetId) == localModifications.end()) {
                localModifications[change.targetId] = change;
            }
        }
        current = commit.parentHash;
    }
    
    // Extract remote changes from LCA to remote head
    std::map<uint32_t, UserChange> remoteModifications;
    current = remoteHeadHash;
    while (current != lcaHash && !current.empty()) {
        const auto& commit = m_commits[current];
        for (const auto& change : commit.deltas) {
            if (remoteModifications.find(change.targetId) == remoteModifications.end()) {
                remoteModifications[change.targetId] = change;
            }
        }
        current = commit.parentHash;
    }
    
    // 3. Resolve conflicts using LWW (Last-Writer-Wins) CRDT strategy
    std::vector<UserChange> mergedDeltas;
    std::set<uint32_t> processedTargets;
    
    // Merge remote changes
    for (const auto& [targetId, remoteChange] : remoteModifications) {
        auto it = localModifications.find(targetId);
        if (it != localModifications.end()) {
            // Conflict: parameter modified on both local and remote branches
            // LWW (Last-Writer-Wins) conflict resolver
            if (remoteChange.timestamp >= it->second.timestamp) {
                mergedDeltas.push_back(remoteChange);
            } else {
                mergedDeltas.push_back(it->second);
            }
        } else {
            // Non-conflicting remote change: apply cleanly
            mergedDeltas.push_back(remoteChange);
        }
        processedTargets.insert(targetId);
    }
    
    // Merge remaining non-conflicting local changes
    for (const auto& [targetId, localChange] : localModifications) {
        if (processedTargets.find(targetId) == processedTargets.end()) {
            mergedDeltas.push_back(localChange);
        }
    }
    
    // 4. Create Merge Commit
    uint64_t ts = std::chrono::steady_clock::now().time_since_epoch().count();
    std::string mergeHash = calculateHash(m_headCommitHash, remoteHeadHash, mergedDeltas, ts, 9999);
    
    Commit mergeCommit;
    mergeCommit.hash = mergeHash;
    mergeCommit.parentHash = m_headCommitHash;
    mergeCommit.mergeParentHash = remoteHeadHash;
    mergeCommit.timestamp = ts;
    mergeCommit.userId = 9999; // System identifier
    mergeCommit.description = "Merge Branch: 3-way LWW-CRDT Conflict Resolution";
    mergeCommit.deltas = mergedDeltas;
    
    m_commits[mergeHash] = mergeCommit;
    m_headCommitHash = mergeHash;
    
    return true;
}

std::string CloudSyncOrchestrator::getHeadHash() const {
    std::lock_guard<std::mutex> lock(m_mutex);
    return m_headCommitHash;
}

const Commit* CloudSyncOrchestrator::getCommit(const std::string& hash) const {
    std::lock_guard<std::mutex> lock(m_mutex);
    auto it = m_commits.find(hash);
    if (it != m_commits.end()) {
        return &it->second;
    }
    return nullptr;
}

bool CloudSyncOrchestrator::copyCommit(const std::string& hash, Commit& destination) const {
    std::lock_guard<std::mutex> lock(m_mutex);
    const auto it = m_commits.find(hash);
    if (it == m_commits.end()) return false;
    destination = it->second;
    return true;
}

std::vector<CloudSyncOrchestrator::RemoteUser> CloudSyncOrchestrator::getActiveUsers() const {
    std::lock_guard<std::mutex> lock(m_mutex);
    std::vector<RemoteUser> users;
    for (const auto& pair : m_remoteUsers) {
        users.push_back(pair.second);
    }
    return users;
}

std::string CloudSyncOrchestrator::calculateHash(const std::string& parentHash, const std::string& mergeParentHash, const std::vector<UserChange>& deltas, uint64_t ts, uint32_t uid) {
    uint64_t hash = 5381;
    for (char c : parentHash) {
        hash = ((hash << 5) + hash) + c;
    }
    for (char c : mergeParentHash) {
        hash = ((hash << 5) + hash) + c;
    }
    std::vector<UserChange> canonical = deltas;
    std::stable_sort(canonical.begin(), canonical.end(), [](const UserChange& a, const UserChange& b) {
        if (a.targetId != b.targetId) return a.targetId < b.targetId;
        if (a.timestamp != b.timestamp) return a.timestamp < b.timestamp;
        return a.userId < b.userId;
    });
    for (const auto& d : canonical) {
        hash = ((hash << 5) + hash) + d.targetId;
        uint32_t valueBits = 0;
        std::memcpy(&valueBits, &d.newValue, sizeof(valueBits));
        hash = ((hash << 5) + hash) + valueBits;
        hash = ((hash << 5) + hash) + d.timestamp;
        hash = ((hash << 5) + hash) + d.userId;
        for (const char c : d.meta) hash = ((hash << 5) + hash) + static_cast<unsigned char>(c);
    }
    hash = ((hash << 5) + hash) + ts;
    hash = ((hash << 5) + hash) + uid;
    
    char hex[32];
    std::snprintf(hex, sizeof(hex), "%016llx", (unsigned long long)hash);
    return std::string(hex);
}

std::string CloudSyncOrchestrator::findLowestCommonAncestor(const std::string& localHash, const std::string& remoteHash) {
    std::set<std::string> localAncestors;

    // A merge commit has two parents. Walk both sides, otherwise a remote
    // branch created from a previous merge can incorrectly look unrelated.
    std::vector<std::string> pending{localHash};
    while (!pending.empty()) {
        const std::string current = std::move(pending.back());
        pending.pop_back();
        if (current.empty() || !localAncestors.insert(current).second) continue;
        const auto it = m_commits.find(current);
        if (it == m_commits.end()) continue;
        pending.push_back(it->second.parentHash);
        pending.push_back(it->second.mergeParentHash);
    }

    pending = {remoteHash};
    while (!pending.empty()) {
        const std::string current = std::move(pending.back());
        pending.pop_back();
        if (current.empty()) continue;
        if (localAncestors.find(current) != localAncestors.end()) return current;
        const auto it = m_commits.find(current);
        if (it == m_commits.end()) continue;
        pending.push_back(it->second.parentHash);
        pending.push_back(it->second.mergeParentHash);
    }
    return "";
}

} // namespace Aura::Core::Engine
