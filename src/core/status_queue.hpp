#pragma once
#include <string_view>
#include <optional>
#include <atomic>
#include <algorithm>
#include <cstring>
#include "concurrency/lock_free.hpp"

namespace Aura::Core {

/**
 * @brief StatusQueue: Truly lock-free communication for engine health logs.
 * HONEST FIX: Replaced std::string with fixed-size char array for RT-safety.
 */
class StatusQueue {
public:
    static StatusQueue& getInstance() {
        static StatusQueue instance;
        return instance;
    }

    enum class Severity { Info, Warning, Error, Critical };

    struct Message {
        Severity severity;
        char text[128];
    };

    /**
     * @brief Pushes a status update from the Audio thread.
     * HONEST FIX: No heap allocation here.
     */
    void pushFromAudio(Severity severity, std::string_view text) {
        Message m;
        m.severity = severity;
        const size_t length = std::min(text.size(), sizeof(m.text) - 1);
        std::memcpy(m.text, text.data(), length);
        m.text[length] = '\0';
        if (!m_queue.push(m)) m_dropped.fetch_add(1, std::memory_order_relaxed);
    }

    /**
     * @brief Pops a status message for the UI thread.
     */
    bool pop(Message& out) {
        auto msg = m_queue.pop();
        if (msg) {
            out = *msg;
            return true;
        }
        return false;
    }

    uint64_t droppedCount() const noexcept {
        return m_dropped.load(std::memory_order_relaxed);
    }

    uint64_t takeDroppedCount() noexcept {
        return m_dropped.exchange(0, std::memory_order_acq_rel);
    }

private:
    Concurrency::SPSCQueue<Message, 256> m_queue;
    std::atomic<uint64_t> m_dropped{0};
};

} // namespace Aura::Core
