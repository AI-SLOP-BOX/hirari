#pragma once

#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/** C++ compatibility facade for Rust-owned global parameter smoothing. */
class AutomationManager {
public:
    static constexpr size_t kMaxTracks = 1024;
    static constexpr size_t kMaxParamsPerTrack = 512;

    static AutomationManager& getInstance() {
        static AutomationManager instance;
        return instance;
    }

    void prepareToPlay(double sampleRate) {
        hirari_automation_manager_prepare(m_state, sampleRate);
    }

    void process(uint32_t numSamples) {
        hirari_automation_manager_process(m_state, numSamples);
    }

    float getValue(uint32_t trackId, uint32_t paramId) const {
        return hirari_automation_manager_get_value(m_state, trackId, paramId);
    }

    bool setTarget(uint32_t trackId, uint32_t paramId, float value) {
        return hirari_automation_manager_set_target(m_state, trackId, paramId, value);
    }

    float getTarget(uint32_t trackId, uint32_t paramId) const {
        return hirari_automation_manager_get_target(m_state, trackId, paramId);
    }

    void reset() { hirari_automation_manager_reset(m_state); }

    AutomationManager(const AutomationManager&) = delete;
    AutomationManager& operator=(const AutomationManager&) = delete;

private:
    AutomationManager() : m_state(hirari_automation_manager_create()) {}
    ~AutomationManager() { hirari_automation_manager_destroy(m_state); }

    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
