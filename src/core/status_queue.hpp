#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>

extern "C" bool hirari_status_queue_push_default(
    uint32_t severity, const uint8_t* text, size_t text_size);
extern "C" bool hirari_status_queue_pop_default(void* output_message);
extern "C" uint64_t hirari_status_queue_take_dropped_default();

namespace Hirari::Core {

enum class StatusSeverity : uint32_t { Info, Warning, Error, Critical };

struct StatusMessage {
    StatusSeverity severity{};
    char text[128]{};
};

inline void pushEngineStatus(StatusSeverity severity, std::string_view text) noexcept {
    (void)hirari_status_queue_push_default(static_cast<uint32_t>(severity),
        reinterpret_cast<const uint8_t*>(text.data()), text.size());
}

inline bool popEngineStatus(StatusMessage& output) noexcept {
    return hirari_status_queue_pop_default(&output);
}

inline uint64_t takeDroppedEngineStatus() noexcept {
    return hirari_status_queue_take_dropped_default();
}

} // namespace Hirari::Core
