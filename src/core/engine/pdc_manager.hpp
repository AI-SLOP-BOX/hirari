#pragma once

#include <cstddef>
#include <cstdint>

#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade. Rust owns PDC configuration, solving, thread ownership,
// and atomic publication; the C++ engine keeps its established method surface.
class PDCManager {
public:
    static constexpr size_t kMaxTracks = 512;
    static constexpr size_t kMaxBuses = 128;
    static constexpr uint32_t kMasterID = 0xFFFFFFFF;

    PDCManager() : m_state(hirari_native_pdc_create()) {}
    ~PDCManager() { hirari_native_pdc_destroy(m_state); }

    PDCManager(const PDCManager&) = delete;
    PDCManager& operator=(const PDCManager&) = delete;

    static PDCManager& getInstance() {
        static PDCManager instance;
        return instance;
    }

    uint32_t getCompensationOffset(uint32_t trackId) const {
        return hirari_native_pdc_get_track_offset(m_state, trackId);
    }
    uint32_t getGlobalMaxLatency() const {
        return hirari_native_pdc_get_global_latency(m_state);
    }
    uint32_t getMaxLatency() const { return getGlobalMaxLatency(); }
    bool hasCycle() const { return hirari_native_pdc_has_cycle(m_state); }
    bool lowLatencyMode() const { return hirari_native_pdc_low_latency_mode(m_state); }
    uint64_t configurationGeneration() const {
        return hirari_native_pdc_configuration_generation(m_state);
    }

    bool bindControlThread() noexcept {
        return hirari_native_pdc_bind_control_thread(m_state);
    }
    uint32_t getBusOffset(uint32_t busId) const {
        return hirari_native_pdc_get_bus_offset(m_state, busId);
    }
    uint32_t getSendCompensationSamples(uint32_t sourceId, uint32_t destId) const noexcept {
        return hirari_native_pdc_get_send_offset(m_state, sourceId, destId);
    }

    bool setTrackDest(uint32_t trackId, uint32_t destId) {
        return hirari_native_pdc_set_track_destination(m_state, trackId, destId);
    }
    bool setBusDest(uint32_t busId, uint32_t destId) {
        return hirari_native_pdc_set_bus_destination(m_state, busId, destId);
    }
    bool setSendRoute(uint32_t sourceId, uint32_t destId, bool enabled) {
        return hirari_native_pdc_set_send_route(m_state, sourceId, destId, enabled);
    }
    bool setTrackLatency(uint32_t trackId, uint32_t samples) {
        return hirari_native_pdc_set_track_latency(m_state, trackId, samples);
    }
    bool setBusLatency(uint32_t busId, uint32_t samples) {
        return hirari_native_pdc_set_bus_latency(m_state, busId, samples);
    }
    bool setLowLatencyMode(bool active) {
        return hirari_native_pdc_set_low_latency_mode(m_state, active);
    }
    bool clearRoutingConfiguration() {
        return hirari_native_pdc_clear_configuration(m_state);
    }
    bool markDirty() { return hirari_native_pdc_mark_dirty(m_state); }

    void resetForProject() { hirari_native_pdc_reset_for_project(m_state); }
    void recalculate() { hirari_native_pdc_recalculate(m_state); }
    bool auditAgainstSharedSolver() const { return hirari_native_pdc_audit(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
