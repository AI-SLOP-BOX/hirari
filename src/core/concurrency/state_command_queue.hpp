#pragma once
#include <atomic>
#include <vector>
#include <cstdint>
#include <cmath>

namespace Aura::Core::Concurrency {

/**
 * @enum CommandType
 * @brief Types of state changes that can be dispatched to the audio engine.
 */
enum class CommandType {
    SetVolume,
    SetPan,
    SetMute,
    SetSolo,
    SetPluginParam
};

/**
 * @struct StateCommand
 * @brief A sample-accurate state change command.
 */
struct StateCommand {
    uint32_t trackId;
    CommandType type;
    float value;
    uint32_t sampleOffset;
};

/**
 * @class StateCommandQueue
 * @brief Lock-Free Single-Producer Single-Consumer Queue for audio thread state updates.
 */
class StateCommandQueue {
public:
    static constexpr uint32_t kCapacity = 4096;

    StateCommandQueue() : m_head(0), m_tail(0) {
        m_buffer.resize(kCapacity);
    }

    /**
     * @brief Pushes a command to the queue from the UI/Logic thread.
     */
    bool push(const StateCommand& cmd) {
        if (cmd.trackId >= 1u << 20 || !std::isfinite(cmd.value)) {
            m_rejected.fetch_add(1, std::memory_order_relaxed);
            return false;
        }
        uint32_t head = m_head.load(std::memory_order_relaxed);
        uint32_t nextHead = (head + 1) % kCapacity;
        if (nextHead == m_tail.load(std::memory_order_acquire)) {
            m_dropped.fetch_add(1, std::memory_order_relaxed);
            return false; // Full
        }

        m_buffer[head] = cmd;
        m_head.store(nextHead, std::memory_order_release);
        return true;
    }

    uint32_t sizeApprox() const noexcept {
        const uint32_t head = m_head.load(std::memory_order_acquire);
        const uint32_t tail = m_tail.load(std::memory_order_acquire);
        return head >= tail ? head - tail : kCapacity - tail + head;
    }

    uint64_t droppedCount() const noexcept { return m_dropped.load(std::memory_order_relaxed); }
    uint64_t rejectedCount() const noexcept { return m_rejected.load(std::memory_order_relaxed); }

    /**
     * @brief Pops a command from the queue on the Audio thread.
     */
    bool pop(StateCommand& cmd) {
        uint32_t tail = m_tail.load(std::memory_order_relaxed);
        if (tail == m_head.load(std::memory_order_acquire)) return false; // Empty

        cmd = m_buffer[tail];
        m_tail.store((tail + 1) % kCapacity, std::memory_order_release);
        return true;
    }

private:
    std::vector<StateCommand> m_buffer;
    std::atomic<uint32_t> m_head, m_tail;
    std::atomic<uint64_t> m_dropped{0};
    std::atomic<uint64_t> m_rejected{0};
};

} // namespace Aura::Core::Concurrency
