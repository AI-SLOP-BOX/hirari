#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <utility>
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
        (void)hirari_undo_manager_commit(m_rustState,
            reinterpret_cast<const uint8_t*>(name.data()), name.size(),
            undoId, redoId, true, true);
    }

    bool recordAppliedAction(const std::string& name, std::function<void()> undo,
                             std::function<void()> redo, bool coalesce = false) {
        if (!undo || !redo) return false;
        const auto [undoId, redoId] = storeCallbacks(std::move(undo), std::move(redo));
        return hirari_undo_manager_commit(m_rustState,
            reinterpret_cast<const uint8_t*>(name.data()), name.size(),
            undoId, redoId, coalesce, false);
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
    void undo() { (void)hirari_undo_manager_apply(m_rustState, false); }
    void redo() { (void)hirari_undo_manager_apply(m_rustState, true); }

    void clear() { hirari_undo_manager_clear(m_rustState); }

    size_t getUndoCount() const { return hirari_undo_manager_undo_count(m_rustState); }
    size_t getRedoCount() const { return hirari_undo_manager_redo_count(m_rustState); }

    /** Address of the Rust history this facade wraps, so Rust callers can
     *  record into the same one instead of routing through here. */
    void* rustState() const noexcept { return m_rustState; }

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

    void* m_rustState = nullptr;
};

} // namespace Hirari::Core::Engine
