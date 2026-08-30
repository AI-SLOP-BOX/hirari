#pragma once
#include <atomic>
#include <array>
#include <optional>
#include <cstddef>
#include <utility>

namespace Aura::Core::Concurrency {

/**
 * @class MPMCQueue
 * @brief Multi-Producer Multi-Consumer Lock-Free Index Queue.
 */
template <typename T, size_t Capacity = 4096>
class MPMCQueue {
    static_assert(Capacity >= 2 && (Capacity & (Capacity - 1)) == 0,
                  "MPMCQueue capacity must be a power of two and at least 2");
    struct Slot {
        T data;
        std::atomic<size_t> sequence;
    };

public:
    MPMCQueue() : m_enqueuePos(0), m_dequeuePos(0) {
        for (size_t i = 0; i < Capacity; ++i) m_buffer[i].sequence.store(i, std::memory_order_relaxed);
    }

    bool push(const T& data) {
        Slot* slot;
        size_t pos = m_enqueuePos.load(std::memory_order_relaxed);
        for (;;) {
            slot = &m_buffer[pos & (Capacity - 1)];
            size_t seq = slot->sequence.load(std::memory_order_acquire);
            intptr_t diff = (intptr_t)seq - (intptr_t)pos;
            if (diff == 0) {
                if (m_enqueuePos.compare_exchange_weak(pos, pos + 1, std::memory_order_relaxed)) break;
            } else if (diff < 0) return false;
            else pos = m_enqueuePos.load(std::memory_order_relaxed);
        }
        slot->data = data;
        slot->sequence.store(pos + 1, std::memory_order_release);
        return true;
    }

    std::optional<T> pop() {
        Slot* slot;
        size_t pos = m_dequeuePos.load(std::memory_order_relaxed);
        for (;;) {
            slot = &m_buffer[pos & (Capacity - 1)];
            size_t seq = slot->sequence.load(std::memory_order_acquire);
            intptr_t diff = (intptr_t)seq - (intptr_t)(pos + 1);
            if (diff == 0) {
                if (m_dequeuePos.compare_exchange_weak(pos, pos + 1, std::memory_order_relaxed)) break;
            } else if (diff < 0) return std::nullopt;
            else pos = m_dequeuePos.load(std::memory_order_relaxed);
        }
        T data = std::move(slot->data); // HONEST FIX: Move to prevent ref-count sticking
        slot->sequence.store(pos + Capacity, std::memory_order_release);
        return data;
    }

    bool pop(T& out) {
        auto value = pop();
        if (!value) return false;
        out = std::move(*value);
        return true;
    }

private:
    std::array<Slot, Capacity> m_buffer;
    alignas(64) std::atomic<size_t> m_enqueuePos;
    alignas(64) std::atomic<size_t> m_dequeuePos;
};

/**
 * @class SPSCQueue
 * @brief Single-Producer Single-Consumer Lock-Free Ring Buffer.
 */
template <typename T, size_t Capacity = 1024>
class SPSCQueue {
    static_assert((Capacity & (Capacity - 1)) == 0, "Capacity must be a power of 2");

public:
    SPSCQueue() : m_writeIdx(0), m_readIdx(0) {}

    bool push(const T& value) {
        size_t writeIdx = m_writeIdx.load(std::memory_order_relaxed);
        size_t nextWrite = (writeIdx + 1) & (Capacity - 1);
        
        if (nextWrite == m_readIdx.load(std::memory_order_acquire)) {
            return false;
        }
        
        m_buffer[writeIdx] = value;
        m_writeIdx.store(nextWrite, std::memory_order_release);
        return true;
    }

    std::optional<T> pop() {
        size_t readIdx = m_readIdx.load(std::memory_order_relaxed);
        if (readIdx == m_writeIdx.load(std::memory_order_acquire)) {
            return std::nullopt;
        }

        T value = std::move(m_buffer[readIdx]); // HONEST FIX: Clear slot to release shared_ptr
        m_readIdx.store((readIdx + 1) & (Capacity - 1), std::memory_order_release);
        return value;
    }

    bool pop(T& outItem) {
        size_t readIdx = m_readIdx.load(std::memory_order_relaxed);
        if (readIdx == m_writeIdx.load(std::memory_order_acquire)) {
            return false;
        }

        outItem = std::move(m_buffer[readIdx]); // HONEST FIX: Clear slot
        m_readIdx.store((readIdx + 1) & (Capacity - 1), std::memory_order_release);
        return true;
    }

private:
    std::array<T, Capacity> m_buffer;
    alignas(64) std::atomic<size_t> m_writeIdx;
    alignas(64) std::atomic<size_t> m_readIdx;
};

} // namespace Aura::Core::Concurrency
