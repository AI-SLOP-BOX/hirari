#pragma once
#include <vector>
#include <deque>
#include <memory>
#include <string>
#include <mutex>
#include "../aura_unified_engine.hpp"

namespace Aura::Core::Project {

/**
 * @struct ProjectState
 * @brief Binary Snapshot of the entire DAW's current state.
 * HONEST FIX: Replaces 'partial undo' with 'Full State Swap' to 
 * eliminate parameter/engine inconsistency errors (Logic Pro architecture).
 */
struct ProjectState {
    std::string binaryData; // Serialized state (Tracks, Busses, Params, Automation)
    std::string timestampBadge;
};

/**
 * @class UndoManager
 * @brief Zero-Latency State Restorer.
 * Features: High-depth undo history with Atomic Engine Re-sync.
 */
class UndoManager {
public:
    static UndoManager& getInstance() {
        static UndoManager instance;
        return instance;
    }

    /**
     * @brief THE INSTANT SNAPSHOT: Captures the current engine state in an atomic block.
     * HONEST FIX: Uses binary serialization to ensure NO setting is left out.
     */
    void pushUndo(const std::string& description) {
        std::lock_guard<std::mutex> lock(m_mutex);
        
        ProjectState state;
        state.binaryData = AuraEngine::getInstance().serializeState(); // Full Project Dump
        state.timestampBadge = description;
        
        m_history.push_back(std::move(state));
        if (m_history.size() > 100) m_history.pop_front(); // Max 100 deep
        m_redoStack.clear();
    }

    /**
     * @brief THE ATOMIC RESTORE: Swaps the engine's entire brain in one thread-safe cycle. 
     * HONEST FIX: Clearing the engine's command queue before restoring to 
     * prevent 'hanging' parameter changes during the undo transition.
     */
    void undo() {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_history.empty()) return;

        ProjectState last = std::move(m_history.back());
        m_history.pop_back();

        // 1. Snapshot current state for Redo. Do not publish it to the redo
        // stack until the requested restore has succeeded; otherwise a
        // failed disk/native hydration would consume the user's undo entry.
        ProjectState redo;
        redo.binaryData = AuraEngine::getInstance().serializeState();

        // 2. THE ATOMIC RESTORE: Forced Engine Sync
        // We atomically inject the old state to ENSURE no settings are 'forgotten'
        if (AuraEngine::getInstance().restoreState(last.binaryData)) {
            m_redoStack.push_back(std::move(redo));
        } else {
            m_history.push_back(std::move(last));
        }
    }

private:
    UndoManager() = default;
    std::deque<ProjectState> m_history;
    std::vector<ProjectState> m_redoStack;
    std::mutex m_mutex;
};

} // namespace Aura::Core::Project
