#pragma once
#include <deque>
#include <functional>
#include <string>
#include <memory>
#include <vector>
#include <utility>
#include <chrono>
#include "../log_buffer.hpp"

namespace Aura::Core::Engine {

/**
 * @struct UndoAction
 * @brief Lambda-based Command Pattern for professional DAW undo/redo.
 */
struct UndoAction {
    std::string name;
    std::function<void()> undo;
    std::function<void()> redo;
    uint64_t timestampMs = 0;
};

/**
 * @class UndoTransactionManager
 * @brief Non-blocking, deque-backed Undo/Redo history stack.
 *
 * Design improvements:
 * - std::deque instead of std::vector: O(1) front eviction (no shift copies).
 * - LogBuffer::post instead of std::cout: lock-free, non-blocking logging.
 * - kMaxHistoryDepth enforced via pop_front on deque (O(1) vs O(N) vector erase).
 */
class UndoTransactionManager {
public:
    static constexpr size_t kMaxHistoryDepth = 128;

    static UndoTransactionManager& getInstance() {
        static UndoTransactionManager instance;
        return instance;
    }

    /**
     * @brief Executes an action and pushes it onto the undo stack.
     * UI/message thread only.
     */
    void performAction(const std::string& name, std::function<void()> undo,
                       std::function<void()> redo) {
        if (!redo || !undo) return;

        const uint64_t now = timestamp();
        redo();

        if (m_transactionActive) {
            // Transactions deliberately bypass knob coalescing: every command
            // must remain available for an all-or-nothing rollback.
            m_transactionActions.push_back({name, std::move(undo), std::move(redo), now});
            return;
        }

        // A knob drag emits many values, but users expect one undo step. Keep
        // the original undo closure and replace only the redo closure while
        // the same semantic action continues within the coalescing window.
        if (!m_undoStack.empty()) {
            auto& previous = m_undoStack.back();
            if (previous.name == name && now >= previous.timestampMs &&
                now - previous.timestampMs <= kCoalesceWindowMs) {
                previous.redo = std::move(redo);
                previous.timestampMs = now;
                m_redoStack.clear();
                return;
            }
        }

        m_undoStack.push_back({name, std::move(undo), std::move(redo), now});

        // O(1) eviction via deque::pop_front
        if (m_undoStack.size() > kMaxHistoryDepth) {
            m_undoStack.pop_front();
        }

        // New action invalidates the redo stack
        m_redoStack.clear();

        Diagnostics::LogBuffer::post(0, 0xA001,
            ("UNDO | PERFORMED | " + name).substr(0, Diagnostics::LogBuffer::kMaxLogLen - 1));
    }

    /**
     * @brief Records an already-applied mutation without executing it again.
     *
     * Useful for operations that must publish their state while holding a
     * different engine lock. Calling performAction() after such a mutation
     * would execute redo a second time and duplicate the side effect.
     */
    void recordAppliedAction(const std::string& name, std::function<void()> undo,
                             std::function<void()> redo) {
        if (!redo || !undo) return;
        const uint64_t now = timestamp();
        if (m_transactionActive) {
            m_transactionActions.push_back({name, std::move(undo), std::move(redo), now});
            return;
        }
        pushCompletedAction({name, std::move(undo), std::move(redo), now});
        Diagnostics::LogBuffer::post(0, 0xA001,
            ("UNDO | RECORDED | " + name).substr(0, Diagnostics::LogBuffer::kMaxLogLen - 1));
    }

    void beginTransaction(const std::string& name) {
        if (m_transactionActive) {
            abortTransaction();
        }
        m_transactionActive = true;
        m_transactionName = name.empty() ? "transaction" : name;
        m_transactionActions.clear();
    }

    bool transactionActive() const { return m_transactionActive; }

    bool endTransaction() {
        if (!m_transactionActive) return false;
        m_transactionActive = false;
        if (m_transactionActions.empty()) {
            m_transactionName.clear();
            return true;
        }

        auto actions = std::make_shared<std::vector<UndoAction>>(
            std::move(m_transactionActions));
        const std::string name = std::move(m_transactionName);
        const auto undo = [actions]() {
            for (auto it = actions->rbegin(); it != actions->rend(); ++it) {
                if (it->undo) it->undo();
            }
        };
        const auto redo = [actions]() {
            for (auto& action : *actions) {
                if (action.redo) action.redo();
            }
        };
        m_transactionActions.clear();
        m_transactionName.clear();
        pushCompletedAction({name, undo, redo, timestamp()});
        return true;
    }

    bool abortTransaction() {
        if (!m_transactionActive) return false;
        for (auto it = m_transactionActions.rbegin();
             it != m_transactionActions.rend(); ++it) {
            if (it->undo) it->undo();
        }
        m_transactionActions.clear();
        m_transactionName.clear();
        m_transactionActive = false;
        return true;
    }

    /**
     * @brief Reverts the last action.
     */
    void undo() {
        if (m_undoStack.empty()) {
            Diagnostics::LogBuffer::post(1, 0xA001, "UNDO | STACK_EMPTY");
            return;
        }

        UndoAction action = std::move(m_undoStack.back());
        m_undoStack.pop_back();

        action.undo();

        Diagnostics::LogBuffer::post(0, 0xA001,
            ("UNDO | UNDONE | " + action.name).substr(0, Diagnostics::LogBuffer::kMaxLogLen - 1));

        m_redoStack.push_back(std::move(action));
    }

    /**
     * @brief Restores the last undone action.
     */
    void redo() {
        if (m_redoStack.empty()) {
            Diagnostics::LogBuffer::post(1, 0xA001, "UNDO | REDO_STACK_EMPTY");
            return;
        }

        UndoAction action = std::move(m_redoStack.back());
        m_redoStack.pop_back();

        action.redo();

        Diagnostics::LogBuffer::post(0, 0xA001,
            ("UNDO | REDONE | " + action.name).substr(0, Diagnostics::LogBuffer::kMaxLogLen - 1));

        m_undoStack.push_back(std::move(action));
    }

    void clear() {
        m_transactionActions.clear();
        m_transactionName.clear();
        m_transactionActive = false;
        m_undoStack.clear();
        m_redoStack.clear();
    }

    size_t getUndoCount() const { return m_undoStack.size(); }
    size_t getRedoCount() const { return m_redoStack.size(); }

public:
    UndoTransactionManager() = default;

private:

    static uint64_t timestamp() {
        const auto now = std::chrono::steady_clock::now().time_since_epoch();
        return static_cast<uint64_t>(
            std::chrono::duration_cast<std::chrono::milliseconds>(now).count());
    }

    static constexpr uint64_t kCoalesceWindowMs = 300;

    void pushCompletedAction(UndoAction action) {
        m_undoStack.push_back(std::move(action));
        if (m_undoStack.size() > kMaxHistoryDepth) m_undoStack.pop_front();
        m_redoStack.clear();
    }

    std::deque<UndoAction> m_undoStack;
    std::deque<UndoAction> m_redoStack;
    std::vector<UndoAction> m_transactionActions;
    std::string m_transactionName;
    bool m_transactionActive = false;
};

} // namespace Aura::Core::Engine
