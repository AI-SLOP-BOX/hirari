#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>
#include "rust_ffi.hpp"

namespace Hirari::Core {

/** C++ message/API adapter for the Rust-owned bounded MPMC engine status queue. */
class StatusQueue {
public:
    static constexpr size_t kStateStorageBytes = 48 * 1024;

    static StatusQueue& getInstance() {
        static StatusQueue instance;
        return instance;
    }

    enum class Severity : uint32_t { Info, Warning, Error, Critical };

    struct Message {
        Severity severity{};
        char text[128]{};
    };
    static_assert(sizeof(Message) == 132);
    static_assert(offsetof(Message, text) == sizeof(uint32_t));

    StatusQueue(const StatusQueue&) = delete;
    StatusQueue& operator=(const StatusQueue&) = delete;
    ~StatusQueue() { hirari_status_queue_destroy(m_state); }

    void pushFromAudio(Severity severity, std::string_view text) noexcept {
        (void)hirari_status_queue_push(
            m_state, static_cast<uint32_t>(severity),
            reinterpret_cast<const uint8_t*>(text.data()), text.size());
    }

    bool pop(Message& output) noexcept {
        return hirari_status_queue_pop(m_state, &output);
    }

    uint64_t droppedCount() const noexcept {
        return hirari_status_queue_dropped(m_state);
    }

    uint64_t takeDroppedCount() noexcept {
        return hirari_status_queue_take_dropped(m_state);
    }

private:
    StatusQueue() : m_state(hirari_status_queue_init(m_stateStorage, sizeof(m_stateStorage))) {}

    alignas(64) std::byte m_stateStorage[kStateStorageBytes]{};
    void* m_state = nullptr;
};

} // namespace Hirari::Core
