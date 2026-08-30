#pragma once
#include <mutex>
#include <vector>
#include <string>
#include <memory>
#include <stdexcept>
#include "aura_unified_engine.hpp"

namespace Aura::Core::BridgeFFI {

/**
 * @class ProjectManager
 * @brief Handles project lifetime for the FFI orchestration layer.
 * Migrated to FFI namespace to prevent symbol collisions with internal Bridge types.
 */
class ProjectManager {
public:
    static ProjectManager& getInstance() {
        static std::once_flag flag;
        static std::unique_ptr<ProjectManager> instance;
        std::call_once(flag, [] { instance.reset(new ProjectManager()); });
        return *instance;
    }

    void load_project(rust::String path) const {
        std::string rust_path(path.data(), path.length());
        // Loading is a control-plane mutation. The previous async-then-get
        // implementation paid for a thread and immediately joined it, while
        // also making shutdown ordering less predictable. Keep the state
        // transition synchronous and explicit under the manager lock.
        std::lock_guard<std::mutex> lock(m_mutex);
        if (rust_path.empty()) throw std::runtime_error("Invalid project path provided.");
        m_currentPath = std::move(rust_path);
    }

    rust::Vec<uint8_t> serialize_project_bytes() const {
        return ::Aura::Core::Engine::getInstance().serialize_project();
    }

    void undo() const { ::Aura::Core::Engine::getInstance().undo(); }
    void redo() const { ::Aura::Core::Engine::getInstance().redo(); }

    size_t get_undo_history_count() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_history.size();
    }

    rust::String get_undo_name(size_t idx) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (idx < m_history.size()) {
            return rust::String(m_history[idx]);
        }
        return rust::String("");
    }

private:
    mutable std::string m_currentPath;
    mutable std::vector<std::string> m_history;
    mutable std::mutex m_mutex;
    ProjectManager() = default;
};

inline const ProjectManager& get_project_manager() { return ProjectManager::getInstance(); }

} // namespace Aura::Core::BridgeFFI
