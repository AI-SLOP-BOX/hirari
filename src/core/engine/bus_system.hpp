#pragma once

#include <array>
#include <algorithm>
#include <cstdint>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * Handle table for the Rust-owned fixed-capacity stereo summing nodes.
 *
 * Rust (bus_audio.rs) owns each bus's pre/post stages and the summing system
 * that orders them; this type only creates the per-bus handles once, binds
 * them into that system, and hands the raw handle to BusTrack. No vector
 * growth, mutex, or reference-count operation happens on the audio side.
 */
class BusSystem {
public:
    static constexpr uint32_t kMaxSamples = 8192;
    static constexpr uint32_t kMaxBuses = 128;

    BusSystem() {
        for (uint32_t i = 0; i < kMaxBuses; ++i) {
            m_audio[i] = hirari_bus_audio_create();
            hirari_bus_system_bind(m_state, i, m_audio[i]);
        }
    }
    ~BusSystem() {
        hirari_bus_system_destroy(m_state);
        for (void* state : m_audio) hirari_bus_audio_destroy(state);
    }
    BusSystem(const BusSystem&) = delete;
    BusSystem& operator=(const BusSystem&) = delete;
    static BusSystem& getInstance() { static BusSystem instance; return instance; }

    /// Returns the bus summing stage, or null when the id is out of range or
    /// the system rejects the registration.
    void* registerBus(uint32_t id) noexcept {
        if (id >= kMaxBuses || !hirari_bus_system_register(m_state, id)) return nullptr;
        return m_audio[id];
    }

    // Publish the buses owned by the current project. Bus storage is fixed
    // for the lifetime of the engine, so removal only changes membership; a
    // callback holding an older handle remains memory-safe.
    void reconcileBuses(const std::array<bool, kMaxBuses>& active) noexcept {
        hirari_bus_system_reconcile(m_state, active.data(), kMaxBuses);
    }

    void resetForProject() noexcept {
        hirari_bus_system_reset(m_state);
    }

    void* getBus(uint32_t id) noexcept {
        if (id >= kMaxBuses || !hirari_bus_system_is_present(m_state, id)) return nullptr;
        return m_audio[id];
    }

    void process(uint32_t len) noexcept {
        hirari_bus_system_process(m_state, len);
    }

    // Control-thread API.  Only a bounded copy is published; the audio side
    // never retains or traverses the caller's vector.
    void updateRouting(const std::vector<uint32_t>& newOrder) noexcept {
        hirari_bus_system_update_routing(m_state, newOrder.data(), newOrder.size());
    }

    void clear(uint32_t len) noexcept {
        hirari_bus_system_clear(m_state, len);
    }

private:
    std::array<void*, kMaxBuses> m_audio{};
    void* m_state = hirari_bus_system_create();
};

} // namespace Hirari::Core::Engine
