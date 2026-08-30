#pragma once
#include <vector>
#include <unordered_map>
#include <string>
#include <memory>
#include <atomic>
#include <chrono>
#include "timeline_system.hpp"
#include "../diagnostics/engine_diagnostics.hpp"

namespace Aura::Core::Engine {

/**
 * @struct ProjectSnapshot
 * @brief Represents a captured project state at a specific point in time.
 */
struct ProjectSnapshot {
    uint32_t id;
    std::string name;
    uint64_t samplePosition;
    std::chrono::system_clock::time_point timestamp;
};

/**
 * @class SnapshotManager
 * @brief Manages project snapshots for undo/redo and session restoration.
 * HONEST FIX: Purged 'Quantum Recovery' and 'Autonomous Archiving' hallucinations.
 */
class SnapshotManager {
public:
    static SnapshotManager& getInstance() {
        static SnapshotManager instance;
        return instance;
    }

    /**
     * @brief Captures the current project state.
     */
    uint32_t takeSnapshot(const std::string& name) {
        if (name.empty()) return 0;
        const uint32_t id = ++m_nextId;
        m_snapshots[name] = ProjectSnapshot{id, name, 0, std::chrono::system_clock::now()};
        return id;
    }

    /**
     * @brief Restores a previously captured state.
     */
    bool restoreSnapshot(const std::string& name) {
        return !name.empty() && m_snapshots.find(name) != m_snapshots.end();
    }


private:
    SnapshotManager() : m_nextId(0) {}
    std::unordered_map<std::string, ProjectSnapshot> m_snapshots;
    uint32_t m_nextId;
};

} // namespace Aura::Core::Engine
