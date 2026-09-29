#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <utility>
#include "../log_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Rust owns history, callback IDs, grouping, coalescing, eviction, and dispatch.
// The native closures remain here because they mutate the C++ engine graph.
class UndoTransactionManager {
public:
    static constexpr size_t kMaxHistoryDepth = 128;

    UndoTransactionManager() : m_rustState(hirari_undo_manager_create(kMaxHistoryDepth, 300)) {}
    ~UndoTransactionManager() { hirari_undo_manager_destroy(m_rustState); }
    UndoTransactionManager(const UndoTransactionManager&) = delete;
    UndoTransactionManager& operator=(const UndoTransactionManager&) = delete;

    static UndoTransactionManager& getInstance() {
        static UndoTransactionManager instance;
        return instance;
    }

    void performAction(const std::string& name, std::function<void()> undo,
                       std::function<void()> redo) {
        if (!undo || !redo) return;
        const auto [undoId, redoId] = storeCallbacks(std::move(undo), std::move(redo));
        if (!undoId || !redoId) {
            discard(undoId);
            discard(redoId);
            return;
        }
        if (!hirari_undo_manager_invoke_native_callback(m_rustState, redoId)) {
            discard(undoId);
            discard(redoId);
            return;
        }
        if (!record(name, undoId, redoId, true)) {
            discard(undoId);
            discard(redoId);
            return;
        }
        postActionLog("UNDO | PERFORMED | ", name);
    }

    void recordAppliedAction(const std::string& name, std::function<void()> undo,
                             std::function<void()> redo, bool coalesce = false) {
        if (!undo || !redo) return;
        const auto [undoId, redoId] = storeCallbacks(std::move(undo), std::move(redo));
        if (!undoId || !redoId || !record(name, undoId, redoId, coalesce)) {
            discard(undoId);
            discard(redoId);
            return;
        }
        postActionLog("UNDO | RECORDED | ", name);
    }

    void beginTransaction(const std::string& name) {
        if (transactionActive()) abortTransaction();
        (void)hirari_undo_manager_begin_transaction(
            m_rustState, reinterpret_cast<const uint8_t*>(name.data()), name.size());
    }

    bool transactionActive() const {
        return hirari_undo_manager_transaction_active(m_rustState);
    }

    bool endTransaction() { return hirari_undo_manager_end_transaction(m_rustState); }
    bool abortTransaction() { return hirari_undo_manager_abort_and_invoke(m_rustState); }
    void undo() { applyHistory(false); }
    void redo() { applyHistory(true); }

    void clear() { hirari_undo_manager_clear(m_rustState); }

    size_t getUndoCount() const { return hirari_undo_manager_undo_count(m_rustState); }
    size_t getRedoCount() const { return hirari_undo_manager_redo_count(m_rustState); }

private:
    static void invokeNative(void* context) noexcept {
        auto* callback = static_cast<std::function<void()>*>(context);
        try { (*callback)(); } catch (...) { /* Never unwind across the Rust ABI. */ }
    }

    static void destroyNative(void* context) noexcept {
        delete static_cast<std::function<void()>*>(context);
    }

    uint64_t registerCallback(std::function<void()> callback) {
        auto owned = std::make_unique<std::function<void()>>(std::move(callback));
        const uint64_t id = hirari_undo_manager_register_native_callback(
            m_rustState, owned.get(), &invokeNative, &destroyNative);
        if (id) owned.release();
        return id;
    }

    std::pair<uint64_t, uint64_t> storeCallbacks(
        std::function<void()> undo, std::function<void()> redo) {
        return {registerCallback(std::move(undo)), registerCallback(std::move(redo))};
    }

    void discard(uint64_t id) {
        if (id) hirari_undo_manager_discard_native_callback(m_rustState, id);
    }

    bool record(const std::string& name, uint64_t undoId, uint64_t redoId, bool coalesce) {
        return hirari_undo_manager_record(m_rustState,
            reinterpret_cast<const uint8_t*>(name.data()), name.size(),
            undoId, redoId, hirari_undo_manager_timestamp_ms(), coalesce);
    }

    void applyHistory(bool redoDirection) {
        const std::string name = topName(redoDirection);
        if (!hirari_undo_manager_apply_and_invoke(m_rustState, redoDirection)) {
            Diagnostics::LogBuffer::post(1, 0xA001,
                redoDirection ? "UNDO | REDO_STACK_EMPTY" : "UNDO | STACK_EMPTY");
            return;
        }
        postActionLog(redoDirection ? "UNDO | REDONE | " : "UNDO | UNDONE | ", name);
    }

    std::string topName(bool redoDirection) const {
        const size_t required = hirari_undo_manager_top_name(
            m_rustState, redoDirection, nullptr, 0);
        if (required == 0) return {};
        std::string name(required, '\0');
        const size_t count = hirari_undo_manager_top_name(
            m_rustState, redoDirection, reinterpret_cast<uint8_t*>(name.data()), name.size());
        name.resize(std::min(count, name.size()));
        return name;
    }

    static void postActionLog(const char* prefix, const std::string& name) {
        const std::string message = prefix + name;
        Diagnostics::LogBuffer::post(0, 0xA001,
            message.substr(0, Diagnostics::LogBuffer::kMaxLogLen - 1));
    }

    void* m_rustState = nullptr;
};

} // namespace Hirari::Core::Engine
