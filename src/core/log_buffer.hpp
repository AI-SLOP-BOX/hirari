#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <string_view>

#include "rust_ffi.hpp"

namespace Hirari::Core::Diagnostics {

/** Rust-owned bounded diagnostic ring with a source-compatible C++ adapter. */
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

    static uint64_t getTimestamp() noexcept {
        return hirari_log_buffer_timestamp();
    }

    static void post(uint32_t level, uint32_t componentId, std::string_view msg) noexcept {
        hirari_log_buffer_post(
            state(), level, componentId,
            reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
    }

    static bool pop(LogEntry& out) noexcept {
        return hirari_log_buffer_pop(state(), &out);
    }

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

    /** Address of the process-wide ring Rust owns, for Rust-side callers that log. */
    static void* state() noexcept {
        return const_cast<void*>(hirari_log_buffer_default_state());
    }
};

} // namespace Hirari::Core::Diagnostics
