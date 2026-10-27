#pragma once

#include <cstdint>
#include <atomic>
#include <string>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Diagnostics {

/** Native adapter for Rust's lock-free command journal and background writer. */
class ForensicJournaler {
public:
    struct Entry {
        uint32_t type;
        uint32_t tid;
        float val;
        uint64_t ts;
    };
    static_assert(sizeof(Entry) == 24, "journal entry ABI must match the Rust writer");

    ForensicJournaler() : m_state(hirari_forensic_journaler_create()) {}
    ~ForensicJournaler() {
        stop();
        hirari_forensic_journaler_destroy(m_state);
    }
    ForensicJournaler(const ForensicJournaler&) = delete;
    ForensicJournaler& operator=(const ForensicJournaler&) = delete;
    ForensicJournaler(ForensicJournaler&&) = delete;
    ForensicJournaler& operator=(ForensicJournaler&&) = delete;

    static ForensicJournaler& getInstance() {
        static ForensicJournaler instance;
        return instance;
    }

    void start(const std::string& path) {
        wakeWriter();
        hirari_forensic_journaler_start(m_state,
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), &m_wakeSequence,
            &loadWakeSequence, &waitWakeSequence, &wakeWakeSequence);
    }

    void log(uint32_t type, uint32_t tid, float value, uint64_t timestamp) {
        hirari_forensic_journaler_log(m_state, type, tid, value, timestamp);
        wakeWriter();
    }

    void logDescription(uint32_t type, const std::string& description, float value) {
        hirari_forensic_journaler_log_description(m_state, type,
            reinterpret_cast<const uint8_t*>(description.data()), description.size(), value);
        wakeWriter();
    }

    void stop() {
        wakeWriter();
        hirari_forensic_journaler_stop(m_state);
    }

    static std::vector<Entry> recover(const std::string& path) {
        struct RecoveryContext { std::vector<Entry> entries; bool failed = false; } context;
        const bool recovered = hirari_forensic_journaler_recover(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), &context,
            [](void* opaque, uint32_t type, uint32_t tid, float value,
               uint64_t timestamp) noexcept {
                auto& result = *static_cast<RecoveryContext*>(opaque);
                if (result.failed) return;
                try { result.entries.push_back({type, tid, value, timestamp}); }
                catch (...) { result.failed = true; }
            });
        if (!recovered || context.failed) return {};
        return context.entries;
    }

private:
    static uint64_t loadWakeSequence(void* context) noexcept {
        return static_cast<std::atomic<uint64_t>*>(context)->load(std::memory_order_acquire);
    }
    static void waitWakeSequence(void* context, uint64_t observed) noexcept {
        static_cast<std::atomic<uint64_t>*>(context)->wait(observed, std::memory_order_acquire);
    }
    // The writer may enter wait() after the application has already called
    // stop(), so the shutdown path needs a wake of its own; otherwise the join
    // never completes and the app hangs on exit.
    static void wakeWakeSequence(void* context) noexcept {
        auto* sequence = static_cast<std::atomic<uint64_t>*>(context);
        sequence->fetch_add(1, std::memory_order_release);
        sequence->notify_one();
    }
    void wakeWriter() noexcept {
        m_wakeSequence.fetch_add(1, std::memory_order_release);
        m_wakeSequence.notify_one();
    }

    void* m_state = nullptr;
    std::atomic<uint64_t> m_wakeSequence{0};
};

} // namespace Hirari::Core::Diagnostics
