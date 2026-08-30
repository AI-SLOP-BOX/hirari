#pragma once
#include <atomic>
#include <array>
#include <string_view>
#include <cstring>
#include <chrono>
#include <cstdint>
#if defined(__APPLE__)
#include <mach/mach_time.h>
#endif

namespace Aura::Core::Diagnostics {

/**
 * @class LogBuffer
 * @brief INDUSTRIAL: Forensic "Black Box" Stream.
 * Mach-precise, lock-free, and high-density diagnostic capture.
 */
class LogBuffer {
public:
    static constexpr size_t kMaxLogs = 1024;
    static constexpr size_t kMaxLogLen = 96;

    struct LogEntry {
        uint64_t timestamp;
        uint32_t level;
        uint32_t componentId;
        char msg[kMaxLogLen];
    };

    static uint64_t getTimestamp() {
#if defined(__APPLE__)
        return mach_absolute_time();
#else
        return std::chrono::steady_clock::now().time_since_epoch().count();
#endif
    }

    // Bounded MPMC queue. Audio, UI, recovery, and worker threads may all
    // publish diagnostics; a plain fetch_add ring lets a consumer observe a
    // half-written slot when there are multiple producers.
    static void post(uint32_t level, uint32_t componentId, std::string_view msg) noexcept {
        uint64_t position = m_enqueuePosition.load(std::memory_order_relaxed);
        Slot* slot = nullptr;
        for (;;) {
            slot = &m_storage.slots[position & (kMaxLogs - 1)];
            const uint64_t sequence = slot->sequence.load(std::memory_order_acquire);
            const auto difference = static_cast<std::int64_t>(sequence - position);
            if (difference == 0) {
                if (m_enqueuePosition.compare_exchange_weak(
                        position, position + 1, std::memory_order_relaxed)) break;
            } else if (difference < 0) {
                return; // Full: diagnostics must never block the producer.
            } else {
                position = m_enqueuePosition.load(std::memory_order_relaxed);
            }
        }

        auto& entry = slot->entry;
        entry.timestamp = getTimestamp();
        entry.level = level;
        entry.componentId = componentId;
        const size_t len = std::min(msg.length(), kMaxLogLen - 1);
        std::memcpy(entry.msg, msg.data(), len);
        entry.msg[len] = '\0';
        slot->sequence.store(position + 1, std::memory_order_release);
    }

    static bool pop(LogEntry& out) noexcept {
        uint64_t position = m_dequeuePosition.load(std::memory_order_relaxed);
        Slot* slot = nullptr;
        for (;;) {
            slot = &m_storage.slots[position & (kMaxLogs - 1)];
            const uint64_t sequence = slot->sequence.load(std::memory_order_acquire);
            const auto difference = static_cast<std::int64_t>(sequence - (position + 1));
            if (difference == 0) {
                if (m_dequeuePosition.compare_exchange_weak(
                        position, position + 1, std::memory_order_relaxed)) break;
            } else if (difference < 0) {
                return false; // Empty.
            } else {
                position = m_dequeuePosition.load(std::memory_order_relaxed);
            }
        }

        out = slot->entry;
        slot->sequence.store(position + kMaxLogs, std::memory_order_release);
        return true;
    }

    /**
     * @struct BlackBoxRegister
     * @brief Last-Known-Good Engine State.
     */
    struct BlackBoxRegister {
        std::atomic<uint32_t> lastTrackCount{0};
        std::atomic<uint32_t> lastBlockSize{0};
        std::atomic<float> lastCPULoad{0.0f};
        std::atomic<uint64_t> lastSyncTimestamp{0};
        
        static BlackBoxRegister& getInstance() {
            static BlackBoxRegister instance;
            return instance;
        }
    };

private:
    struct Slot {
        std::atomic<uint64_t> sequence{0};
        LogEntry entry{};
    };

    struct Storage {
        std::array<Slot, kMaxLogs> slots{};
        Storage() noexcept {
            for (uint64_t index = 0; index < kMaxLogs; ++index) {
                slots[index].sequence.store(index, std::memory_order_relaxed);
            }
        }
    };

    static inline Storage m_storage{};
    static inline std::atomic<uint64_t> m_enqueuePosition{0};
    static inline std::atomic<uint64_t> m_dequeuePosition{0};
};

} // namespace Aura::Core::Diagnostics
