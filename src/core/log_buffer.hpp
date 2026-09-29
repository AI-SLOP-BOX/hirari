#pragma once

#include <array>
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
            storage().state, level, componentId,
            reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
    }

    static bool pop(LogEntry& out) noexcept {
        return hirari_log_buffer_pop(storage().state, &out);
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

private:
    static constexpr size_t kStateStorageBytes = 128 * 1024;

    struct StateStorage {
        alignas(64) std::array<std::byte, kStateStorageBytes> bytes;
        void* state;

        StateStorage() : bytes{}, state(hirari_log_buffer_init(bytes.data(), bytes.size())) {}
    };

    static StateStorage m_stateStorage;

    static StateStorage& storage() {
        return m_stateStorage;
    }
};

inline LogBuffer::StateStorage LogBuffer::m_stateStorage{};

} // namespace Hirari::Core::Diagnostics
