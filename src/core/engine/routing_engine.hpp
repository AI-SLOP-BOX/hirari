#pragma once
#include <cstddef>
#include <cstdint>
#include <type_traits>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * @struct RoutingEngine
 * @brief Transfer records and limits for the Rust-owned routing runtime.
 *
 * The graph, gain, feedback, send-PDC and topology stores live in
 * hirari-core-bridge/src/routing_engine_core.rs behind one opaque handle; the
 * engine holds that handle and calls the FFI directly. This type only names
 * the records that cross the boundary and the node/edge limits the host
 * checks before calling.
 */
struct RoutingEngine {
    struct Connection {
        uint32_t sourceId;
        uint32_t destId;
        float gain = 1.0f;
        bool send = false;
        bool preFader = false;
    };
    static_assert(std::is_standard_layout_v<Connection>);
    static_assert(offsetof(Connection, sourceId) == 0 &&
                  offsetof(Connection, destId) == 4 &&
                  offsetof(Connection, gain) == 8 &&
                  offsetof(Connection, send) == 12 &&
                  offsetof(Connection, preFader) == 13 && sizeof(Connection) == 16);

    struct FeedbackConnection {
        uint32_t sourceId = 0;
        uint32_t destId = 0;
        float gain = 1.0f;
    };
    static_assert(std::is_standard_layout_v<FeedbackConnection>);
    static_assert(sizeof(FeedbackConnection) == sizeof(uint32_t) * 2 + sizeof(float));

    static constexpr uint32_t kMaxNodes = 128;
    static constexpr uint32_t kMaxFeedbackConnections = 16;
    static constexpr uint32_t kMaxFeedbackSamples = 8192;

    /// Reader section around the Send PDC delay histories owned by Rust.
    class SendPdcReadGuard {
    public:
        explicit SendPdcReadGuard(void* state) noexcept : m_state(state) {
            hirari_routing_engine_enter_send_pdc_read(m_state);
        }
        ~SendPdcReadGuard() { hirari_routing_engine_leave_send_pdc_read(m_state); }
        SendPdcReadGuard(const SendPdcReadGuard&) = delete;
        SendPdcReadGuard& operator=(const SendPdcReadGuard&) = delete;

    private:
        void* m_state;
    };
};

} // namespace Hirari::Core::Engine
