#pragma once
#include <atomic>
#include <vector>
#include <string>
#include <optional>
#include <array>
#include <cstring>

namespace Hirari::Core::Concurrency {

/**
 * @struct EngineStatus
 * @brief Thread-safe message packet from Engine to UI.
 */
    struct EngineStatus {
        enum class Type {
            AIProgress,
            ClippingAlert,
            AnalysisResult,
            PurgeEvent
        };

        EngineStatus() = default;
        EngineStatus(Type t, uint32_t tid, float v, const char* m) : type(t), trackId(tid), value(v) {
            std::strncpy(message, m, 63);
            message[63] = '\0';
        }

        Type type;
        uint32_t trackId;
        float value;
        char message[64];
    };

/**
 * @class StatusQueue
 * @brief Lock-free SPSC Queue for Engine-to-UI telemetry.
 * HONEST FIX: Prevents the UI from polling expensive engine state by 
 * pushing only changed events asynchronously.
 */
class StatusQueue {
public:
    static StatusQueue& getInstance() { static StatusQueue i; return i; }

    bool push(const EngineStatus& status) {
        uint32_t tail = m_tail.load(std::memory_order_relaxed);
        uint32_t nextTail = (tail + 1) % kCapacity;
        if (nextTail == m_head.load(std::memory_order_acquire)) return false; // Full

        m_buffer[tail] = status;
        m_tail.store(nextTail, std::memory_order_release);
        return true;
    }

    std::optional<EngineStatus> pop() {
        uint32_t head = m_head.load(std::memory_order_relaxed);
        if (head == m_tail.load(std::memory_order_acquire)) return std::nullopt; // Empty

        EngineStatus status = m_buffer[head];
        m_head.store((head + 1) % kCapacity, std::memory_order_release);
        return status;
    }

private:
    static constexpr uint32_t kCapacity = 1024;
    std::array<EngineStatus, kCapacity> m_buffer;
    std::atomic<uint32_t> m_head{0}, m_tail{0};
};

} // namespace Hirari::Core::Concurrency
