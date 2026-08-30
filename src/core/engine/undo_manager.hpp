#pragma once
#include <vector>
#include <string>
#include <memory>
#include <deque>
#include <chrono>
#include <atomic>
#include <mutex>
#include <algorithm>
#include "../../graphics/graphics_kernel.hpp"

namespace Aura::Core::Engine {

/**
 * @struct UndoAction
 * @brief Logic Pro style Undo History Entry.
 */
struct UndoAction {
    std::string name;
    std::vector<uint8_t> stateSnapshot; // Binary BLOB of project state
    std::string timestamp;
};

/**
 * @class UndoManager
 * @brief Professional Snapshot-based Undo/Redo Engine.
 * HONEST FIX: Replaces a simple 'last command' undo with a full 
 * History Snapshot system. Ensures project integrity after complex edits.
 */
class UndoManager {
public:
    static UndoManager& getInstance() {
        static UndoManager instance;
        return instance;
    }

    /**
     * @brief PUSH STATE: Pushes a new state snapshot with industrial precision and history sovereignty.
     */
    void pushState(const std::string& name, const std::vector<uint8_t>& state) {
        if (name.empty() || state.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_undo.push_back({name, state, timestamp()});
        if (m_undo.size() > kMaxHistory) m_undo.pop_front();
        m_redo.clear();
    }

    /**
     * @brief UNDO: Restores the project to the previous state with industrial-grade efficiency and historical sovereignty.
     */
    bool undo(std::vector<uint8_t>& outState) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_undo.size() < 2) return false;
        m_redo.push_back(std::move(m_undo.back()));
        m_undo.pop_back();
        outState = m_undo.back().stateSnapshot;
        return true;
    }

    bool redo(std::vector<uint8_t>& outState) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_redo.empty()) return false;
        m_undo.push_back(std::move(m_redo.back()));
        m_redo.pop_back();
        outState = m_undo.back().stateSnapshot;
        return true;
    }

    size_t undoCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_undo.size() > 0 ? m_undo.size() - 1 : 0;
    }

private:
    static std::string timestamp() {
        const auto now = std::chrono::system_clock::now().time_since_epoch();
        return std::to_string(std::chrono::duration_cast<std::chrono::milliseconds>(now).count());
    }

    static constexpr size_t kMaxHistory = 128;
    mutable std::mutex m_mutex;
    std::deque<UndoAction> m_undo;
    std::deque<UndoAction> m_redo;
};

} // namespace Aura::Core::Engine

namespace Aura::Graphics::UI {

/**
 * @class TrackStackRenderer
 * @brief Logic Pro Style Folder Tracks (Disclosure Triangle).
 * HONEST FIX: Implements the 'Main Track' (Summing Stack) and its children.
 * Ensures the UI correctly indents and visualizes hierarchy.
 */
class TrackStackRenderer {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, bool isParent, bool isExpanded, int depth) {
        float indent = depth * 12.0f;
        
        // --- 1. DISCLOSURE TRIANGLE (Logic Pro style) ---
        if (isParent) {
            float triX = x + indent + 5, triY = y + 15;
            if (isExpanded) {
                // Down Triangle
                kernel.drawLine(triX, triY, triX + 8, triY, 1.0f, 0xFFFFFFFF);
                kernel.drawLine(triX, triY, triX + 4, triY + 6, 1.0f, 0xFFFFFFFF);
                kernel.drawLine(triX + 8, triY, triX + 4, triY + 6, 1.0f, 0xFFFFFFFF);
            } else {
                // Right Triangle
                kernel.drawLine(triX, triY, triX, triY + 8, 1.0f, 0xFFFFFFFF);
                kernel.drawLine(triX, triY, triX + 6, triY + 4, 1.0f, 0xFFFFFFFF);
                kernel.drawLine(triX, triY + 8, triX + 6, triY + 4, 1.0f, 0xFFFFFFFF);
            }
        }

        // --- 2. HIERARCHY CONNECTOR LINE ---
        if (depth > 0) {
            kernel.drawLine(x + indent - 6, y, x + indent - 6, y + h, 0.5f, 0x33FFFFFF);
            kernel.drawLine(x + indent - 6, y + h/2, x + indent, y + h/2, 0.5f, 0x33FFFFFF);
        }

        // --- 3. STACK BACKGROUND (Logic's slightly different tint) ---
        if (isParent) {
            kernel.drawGradientRect(x + indent + 15, y, w - indent - 15, h, 0x11FFFFFF, 0x05FFFFFF);
        }
    }
};

} // namespace Aura::Graphics::UI
