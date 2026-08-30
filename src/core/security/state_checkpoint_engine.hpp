#pragma once
#include <vector>
#include <cstdint>
#include <mutex>
#include <algorithm>

namespace Aura::Core::Security {

/**
 * @class StateCheckpointEngine
 * @brief Manages high-frequency project state snapshots.
 */
class StateCheckpointEngine {
public:
    static StateCheckpointEngine& getInstance() {
        static StateCheckpointEngine instance;
        return instance;
    }

    /**
     * @brief Takes an incremental snapshot of the project state.
     */
    void takeSnapshot() {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_pendingSnapshot.empty()) return;
        m_checkpoints.push_back(m_pendingSnapshot);
        if (m_checkpoints.size() > kMaxCheckpoints) m_checkpoints.erase(m_checkpoints.begin());
    }

    /**
     * @brief Restores the engine to the last healthy snapshot.
     */
    void restoreLastHealthy() {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_checkpoints.empty()) return;
        m_restoredSnapshot = m_checkpoints.back();
    }

    void setSnapshot(std::vector<uint8_t> state) {
        if (state.size() > kMaxSnapshotBytes) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_pendingSnapshot = std::move(state);
    }

    std::vector<uint8_t> consumeRestoredSnapshot() {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<uint8_t> result;
        result.swap(m_restoredSnapshot);
        return result;
    }

    size_t checkpointCount() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_checkpoints.size();
    }

private:
    StateCheckpointEngine() = default;
    static constexpr size_t kMaxCheckpoints = 8;
    static constexpr size_t kMaxSnapshotBytes = 16u * 1024u * 1024u;
    std::vector<std::vector<uint8_t>> m_checkpoints;
    std::vector<uint8_t> m_pendingSnapshot;
    std::vector<uint8_t> m_restoredSnapshot;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Security
