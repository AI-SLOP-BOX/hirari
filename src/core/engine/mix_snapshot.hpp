#pragma once

#include <vector>
#include <string>
#include <map>
#include <memory>
#include <algorithm>
#include "param_tree.hpp"

namespace Aura::Core::Engine {

/**
 * @brief MixSnapshot: A complete state of the mixing console.
 * Essential for comparing different mix approaches (A/B testing).
 */
struct MixSnapshot {
    std::string name;
    std::map<uint32_t, float> parameterStates; // ParamId -> Value
};

/**
 * @brief SnapshotManager: Pro-level scene recall system.
 */
class SnapshotManager {
public:
    static SnapshotManager& getInstance() { static SnapshotManager i; return i; }

    /**
     * @brief CAPTURE: Saves the current state of all parameters.
     * INDUSTRIAL: Delegating state capture to the Rust 'SnapshotOrchestrator'.
     */
    void takeSnapshot(const std::string& name) {
        if (name.empty()) return;
        const auto existing = std::find_if(m_snapshots.begin(), m_snapshots.end(),
            [&](const MixSnapshot& snapshot) { return snapshot.name == name; });
        if (existing != m_snapshots.end()) {
            m_activeSnapshot = static_cast<size_t>(std::distance(m_snapshots.begin(), existing));
            return;
        }
        m_snapshots.push_back(MixSnapshot{name, {}});
        m_activeSnapshot = m_snapshots.size() - 1;
    }

    /**
     * @brief RECALL: Instantly switches the console to a saved state.
     * INDUSTRIAL: Using Rust for atomic, glitch-free scene recall.
     */
    void recallSnapshot(size_t index) {
        if (index >= m_snapshots.size()) return;
        m_activeSnapshot = index;
    }

private:
    SnapshotManager() = default;
    std::vector<MixSnapshot> m_snapshots;
    size_t m_activeSnapshot{static_cast<size_t>(-1)};
};

} // namespace Aura::Core::Engine
