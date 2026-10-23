#pragma once

#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Network {

struct TouchEvent {
    uint32_t trackId;
    uint32_t paramId;
    float value;
    uint64_t timestamp;
};
static_assert(offsetof(TouchEvent, trackId) == 0 && offsetof(TouchEvent, paramId) == 4 &&
              offsetof(TouchEvent, value) == 8 && offsetof(TouchEvent, timestamp) == 16 &&
              sizeof(TouchEvent) == 24);

/** Native API for the Rust POSIX UDP bridge and fixed-capacity touch queue. */
class MobileBridgeKernel {
public:
    static MobileBridgeKernel& getInstance() {
        static MobileBridgeKernel instance;
        return instance;
    }
    ~MobileBridgeKernel() { hirari_mobile_bridge_destroy(m_state); }
    MobileBridgeKernel(const MobileBridgeKernel&) = delete;
    MobileBridgeKernel& operator=(const MobileBridgeKernel&) = delete;
    MobileBridgeKernel(MobileBridgeKernel&&) = delete;
    MobileBridgeKernel& operator=(MobileBridgeKernel&&) = delete;

    void broadcastParameter(uint32_t trackId, uint32_t paramId, float value) {
        hirari_mobile_bridge_broadcast(m_state, trackId, paramId, value);
    }
    bool pushEvent(const TouchEvent& event) {
        return hirari_mobile_bridge_push_event(m_state, event.trackId, event.paramId,
                                               event.value, event.timestamp);
    }
    bool popEvent(TouchEvent& event) {
        return hirari_mobile_bridge_pop_event(m_state, &event.trackId, &event.paramId,
                                              &event.value, &event.timestamp);
    }
    uint64_t getBytesSent() const {
        return hirari_mobile_bridge_bytes_sent(m_state);
    }

private:
    MobileBridgeKernel() : m_state(hirari_mobile_bridge_create()) {}
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Network
