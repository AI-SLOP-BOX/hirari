#pragma once
#include <deque>
#include <string>
#include <unordered_map>
#include <mutex>
#include <array>
#include <nlohmann/json.hpp>

namespace Aura::Core::Engine {

using State = nlohmann::json;

/**
 * @class DeltaStateManager
 * @brief Thread-safe, lock-sharded State Management for DAWs.
 * Combines sharded mutex locks to eliminate cross-target thread contention
 * with periodic snapshots to reduce memory footprint.
 */
class DeltaStateManager {
public:
    struct Entry {
        State data;
        bool isFull;
    };

    static constexpr size_t kSnapshotInterval = 32;
    static constexpr size_t kMaxHistorySize = 100;
    static constexpr size_t kNumShards = 16;

    /**
     * @brief Pushes a new state entry into the target's transaction history.
     * @warning Do NOT call this method from the real-time audio render thread.
     * nlohmann::json operations perform heap allocations (malloc) which violate 
     * real-time safety constraints. Execute state mutations on the UI/Message thread.
     */
    void push(const std::string& targetId, const State& newState) {
        size_t shardIdx = getShardIndex(targetId);
        auto& shard = m_shards[shardIdx];
        
        std::unique_lock<std::mutex> lock(shard.mutex);
        auto& hist = shard.historyEntries[targetId];
        auto& last = shard.lastStates[targetId];

        if (!hist.empty() && last == newState) return;

        // Store full snapshots periodically to avoid constant JSON diffing overhead
        bool shouldBeFull = hist.empty() || (shard.totalPushCount[targetId] % kSnapshotInterval == 0);
        shard.totalPushCount[targetId]++;

        hist.push_back({newState, shouldBeFull});
        
        // Prune older history to stay within memory limits
        while (hist.size() > kMaxHistorySize) {
            hist.pop_front();
        }
        
        // If we popped the base snapshot, promote the next front to be full
        if (!hist.empty() && !hist.front().isFull) {
            hist.front().isFull = true; 
        }

        last = newState;
    }

    /**
     * @brief Reconstructs and returns the latest state for the given target.
     */
    State recover(const std::string& targetId) {
        size_t shardIdx = getShardIndex(targetId);
        auto& shard = m_shards[shardIdx];
        
        std::unique_lock<std::mutex> lock(shard.mutex);
        auto it = shard.historyEntries.find(targetId);
        if (it == shard.historyEntries.end() || it->second.empty()) return State();
        
        const auto& hist = it->second;
        
        // Reconstruct from the last available base snapshot
        for (int i = (int)hist.size() - 1; i >= 0; --i) {
            if (hist[i].isFull) return hist[i].data; 
        }
        return hist.back().data;
    }

private:
    struct StateShard {
        std::unordered_map<std::string, std::deque<Entry>> historyEntries;
        std::unordered_map<std::string, State> lastStates;
        std::unordered_map<std::string, uint64_t> totalPushCount;
        std::mutex mutex;
    };

    size_t getShardIndex(const std::string& targetId) const {
        std::hash<std::string> hasher;
        return hasher(targetId) % kNumShards;
    }

    std::array<StateShard, kNumShards> m_shards;
};

} // namespace Aura::Core::Engine
