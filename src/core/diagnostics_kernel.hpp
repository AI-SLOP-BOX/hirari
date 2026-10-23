#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <functional>
#include <memory>
#include <string_view>

#include "log_buffer.hpp"

extern "C" {
using HirariNativeCallback = void (*)(void*);
using HirariHotSwapLog = void (*)(void*, uint32_t, uint32_t, const uint8_t*, size_t);
using HirariCpuLoad = float (*)(void*);
void* hirari_hot_swap_state_create(void*, HirariHotSwapLog, HirariCpuLoad);
void hirari_hot_swap_state_destroy(void*);
void hirari_hot_swap_state_start(void*);
void hirari_hot_swap_state_stop(void*);
bool hirari_hot_swap_state_begin(
    void*, size_t, void*, HirariNativeCallback, HirariNativeCallback);
void hirari_hot_swap_state_acknowledge(void*);
bool hirari_hot_swap_state_quiesce_requested(const void*);
uint32_t hirari_hot_swap_state_quiesce_blocks(const void*);
bool hirari_hot_swap_state_exceeded_budget(const void*);
}

namespace Hirari::Core::Diagnostics {

/** Watchdog facade; hot-swap state and its health daemon are owned by Rust. */
class DiagnosticsKernel {
public:
    static DiagnosticsKernel& getInstance() {
        static DiagnosticsKernel instance;
        return instance;
    }

    DiagnosticsKernel(const DiagnosticsKernel&) = delete;
    DiagnosticsKernel& operator=(const DiagnosticsKernel&) = delete;

    bool isQuiesceRequested() const {
        return hirari_hot_swap_state_quiesce_requested(m_state);
    }
    void acknowledgeQuiesce() { hirari_hot_swap_state_acknowledge(m_state); }
    uint32_t quiesceBlocks() const noexcept {
        return hirari_hot_swap_state_quiesce_blocks(m_state);
    }
    bool quiesceExceededBudget() const noexcept {
        return hirari_hot_swap_state_exceeded_budget(m_state);
    }
    void startHealthDaemon() { hirari_hot_swap_state_start(m_state); }

    bool attemptHotSwap(uintptr_t targetKernel, std::function<void()> fallback) {
        if (!fallback) return false;
        auto owned = std::make_unique<std::function<void()>>(std::move(fallback));
        if (!hirari_hot_swap_state_begin(m_state, targetKernel, owned.get(),
                                         &invokeFallback, &destroyFallback)) return false;
        owned.release();
        return true;
    }

    static bool PreFlightCheck() { return true; }

private:
    DiagnosticsKernel()
        : m_state(hirari_hot_swap_state_create(nullptr, &postLog, &cpuLoad)) {}
    ~DiagnosticsKernel() { hirari_hot_swap_state_destroy(m_state); }

    static void invokeFallback(void* context) {
        std::unique_ptr<std::function<void()>> callback(
            static_cast<std::function<void()>*>(context));
        try { (*callback)(); } catch (...) { std::terminate(); }
    }
    static void destroyFallback(void* context) {
        delete static_cast<std::function<void()>*>(context);
    }
    static void postLog(void*, uint32_t level, uint32_t code,
                        const uint8_t* message, size_t length) {
        LogBuffer::post(level, code,
            std::string_view(reinterpret_cast<const char*>(message), length));
    }
    static float cpuLoad(void*) {
        return LogBuffer::BlackBoxRegister::getInstance().lastCPULoad.load(
            std::memory_order_relaxed);
    }

    void* m_state = nullptr;
};

} // namespace Hirari::Core::Diagnostics
