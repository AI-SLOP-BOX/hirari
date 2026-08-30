#pragma once
#include <vector>
#include <memory>
#include <string>
#include <deque>
#include <algorithm>

namespace Aura::Core::Undo {

/**
 * @interface Command
 * @brief THE UNDO CONTRACT: Every user action must be reversible.
 */
class Command {
public:
    virtual ~Command() = default;
    virtual void execute() = 0;
    virtual void undo() = 0;
    virtual std::string getName() const = 0;
};

/**
 * @class UndoManager
 * @brief Professional Undo/Redo Engine with Transaction support.
 * HONEST FIX: Implemented Transactions and Action Grouping to prevent history bloat.
 */
class UndoManager {
public:
    static UndoManager& getInstance() { static UndoManager i; return i; }

    /**
     * @brief Performs a command and adds it to the history.
     * If a transaction is active, the command is added to the transaction instead.
     */
    void perform(std::unique_ptr<Command> cmd) {
        if (m_activeTransaction) {
            cmd->execute();
            m_activeTransaction->addCommand(std::move(cmd));
            return;
        }

        cmd->execute();
        m_undoHistory.push_back(std::move(cmd));
        m_redoHistory.clear();
        
        if (m_undoHistory.size() > m_maxSteps) {
            m_undoHistory.pop_front(); 
        }
    }

    /**
     * @brief Groups multiple commands into a single undo step.
     */
    class Transaction {
    public:
        Transaction(UndoManager& parent, const std::string& name) 
            : m_parent(parent), m_name(name) {}
        
        void addCommand(std::unique_ptr<Command> cmd) {
            m_commands.push_back(std::move(cmd));
        }

        void commit() {
            if (m_commands.empty() || m_committed) return;
            auto macro = std::make_unique<MacroCommand>(m_name, std::move(m_commands));
            m_parent.m_undoHistory.push_back(std::move(macro));
            m_parent.m_redoHistory.clear();
            while (m_parent.m_undoHistory.size() > m_parent.m_maxSteps) {
                m_parent.m_undoHistory.pop_front();
            }
            m_committed = true;
        }

        void rollback() {
            if (m_committed) return;
            for (auto it = m_commands.rbegin(); it != m_commands.rend(); ++it) {
                if (*it) (*it)->undo();
            }
            m_commands.clear();
        }

    private:
        UndoManager& m_parent;
        std::string m_name;
        std::vector<std::unique_ptr<Command>> m_commands;
        bool m_committed = false;
    };

    void beginTransaction(const std::string& name) {
        if (m_activeTransaction || name.empty()) return;
        m_activeTransaction = std::make_unique<Transaction>(*this, name);
    }

    void endTransaction() {
        if (!m_activeTransaction) return;
        m_activeTransaction->commit();
        m_activeTransaction.reset();
    }

    /** Roll back commands already executed in a transaction after failure. */
    void abortTransaction() {
        if (!m_activeTransaction) return;
        m_activeTransaction->rollback();
        m_activeTransaction.reset();
    }

    bool transactionActive() const noexcept { return m_activeTransaction != nullptr; }

    void undo() {
        if (m_undoHistory.empty()) return;
        auto cmd = std::move(m_undoHistory.back());
        m_undoHistory.pop_back();
        cmd->undo();
        m_redoHistory.push_back(std::move(cmd));
    }

    void redo() {
        if (m_redoHistory.empty()) return;
        auto cmd = std::move(m_redoHistory.back());
        m_redoHistory.pop_back();
        cmd->execute();
        m_undoHistory.push_back(std::move(cmd));
    }

    void clear() {
        if (m_activeTransaction) m_activeTransaction->rollback();
        m_activeTransaction.reset();
        m_undoHistory.clear();
        m_redoHistory.clear();
    }

    size_t getUndoCount() const noexcept { return m_undoHistory.size(); }
    size_t getRedoCount() const noexcept { return m_redoHistory.size(); }

private:
    class MacroCommand final : public Command {
    public:
        MacroCommand(std::string name, std::vector<std::unique_ptr<Command>> commands)
            : m_name(std::move(name)), m_commands(std::move(commands)) {}
        void execute() override { for (auto& command : m_commands) command->execute(); }
        void undo() override {
            for (auto it = m_commands.rbegin(); it != m_commands.rend(); ++it) (*it)->undo();
        }
        std::string getName() const override { return m_name; }
    private:
        std::string m_name;
        std::vector<std::unique_ptr<Command>> m_commands;
    };

    UndoManager() = default;
    std::deque<std::unique_ptr<Command>> m_undoHistory;
    std::deque<std::unique_ptr<Command>> m_redoHistory;
    std::unique_ptr<Transaction> m_activeTransaction;
    size_t m_maxSteps = 100;
};

} // namespace Aura::Core::Undo
