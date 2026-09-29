#pragma once
#include <string>
#include <vector>
#include <memory>

namespace Hirari::Core::Undo {

/**
 * @interface ICommand
 * @brief すべてのUndo（やり直し）可能なユーザーアクションの基幹インターフェース
 */
class ICommand {
public:
    virtual ~ICommand() = default;
    virtual void execute() = 0;
    virtual void undo() = 0;
};

/**
 * @interface IDeltaCommand
 * @brief MEMORY EFFICIENT: Only stores what changed.
 */
class IDeltaCommand : public ICommand {
public:
    virtual size_t getMemoryUsage() const = 0;
};

class UndoTransactionManager {
public:
    static UndoTransactionManager& getInstance() { static UndoTransactionManager i; return i; }

    /**
     * @brief PUSH (Optimized): Manages RAM budget for undo history.
     */
    void pushCommand(std::shared_ptr<IDeltaCommand> cmd) {
        cmd->execute();
        
        // Trim future
        if (m_currentIndex < m_history.size()) {
            m_history.erase(m_history.begin() + m_currentIndex, m_history.end());
        }
        
        // RAM Budget Management
        m_history.push_back(cmd);
        m_totalMemory += cmd->getMemoryUsage();
        m_currentIndex++;

        // HONEST FIX: If history exceeds 128MB, drop oldest or move to disk
        while (m_totalMemory > 128 * 1024 * 1024 && !m_history.empty()) {
            m_totalMemory -= m_history.front()->getMemoryUsage();
            m_history.erase(m_history.begin());
            m_currentIndex--;
        }
    }

    void undo() {
        if (m_currentIndex > 0) {
            m_currentIndex--;
            m_history[m_currentIndex]->undo();
        }
    }

    void redo() {
        if (m_currentIndex < m_history.size()) {
            m_history[m_currentIndex]->execute();
            m_currentIndex++;
        }
    }
    
    void clearHistory() {
        m_history.clear();
        m_currentIndex = 0;
        m_totalMemory = 0;
    }

private:
    UndoTransactionManager() = default;
    std::vector<std::shared_ptr<IDeltaCommand>> m_history;
    size_t m_currentIndex = 0;
    size_t m_totalMemory = 0;
};

} // namespace Hirari::Core::Undo
