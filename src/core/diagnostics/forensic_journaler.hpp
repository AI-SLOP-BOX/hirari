#pragma once

#include <cstdint>
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
        hirari_forensic_journaler_start(m_state,
            reinterpret_cast<const uint8_t*>(path.data()), path.size());
    }

    void log(uint32_t type, uint32_t tid, float value, uint64_t timestamp) {
        hirari_forensic_journaler_log(m_state, type, tid, value, timestamp);
    }

    void logDescription(uint32_t type, const std::string& description, float value) {
        hirari_forensic_journaler_log_description(m_state, type,
            reinterpret_cast<const uint8_t*>(description.data()), description.size(), value);
    }

    void stop() { hirari_forensic_journaler_stop(m_state); }

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
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Diagnostics
