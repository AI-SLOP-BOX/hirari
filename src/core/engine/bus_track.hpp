#pragma once

#include <atomic>
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>

#include "track.hpp"

namespace Hirari::Core::Engine {

/** AUX/Bus track with an explicit pre-FX input and post-FX output boundary.
 *  Inserts live in the inherited Track chain; the track type selects the
 *  bus-side DSP variants. */
class BusTrack : public Track {
public:
    static constexpr uint32_t kMaxSamples = 8192;
    static constexpr uint32_t kMaxBuses = 128;

    BusTrack(uint32_t id, const std::string& name, uint32_t busId,
             std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : Track(id, name, Track::Type::Bus, std::move(layoutRevision)), m_busId(busId) {}

    void resolveBus(void* busState) noexcept { m_busState = busState; }
    uint32_t busId() const noexcept { return m_busId; }

    /// The Rust engine walk stages this bus's pre-FX input and post-FX output
    /// through the bus handle directly, so it needs the pointer, not the type.
    void* busStateHandle() const noexcept { return m_busState; }

    bool addPreFxInput(const float* left, const float* right,
                       uint32_t len, float gain) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples) return false;
        return hirari_bus_audio_accumulate(m_busState, left, right, len, gain);
    }

    bool loadPreFxIntoWork(uint32_t len) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples || !canProcess(len)) return false;
        void* work = workBufferStorage();
        return hirari_bus_audio_read(m_busState,
                                     hirari_audio_buffer_storage_write_pointer(work, 0),
                                     hirari_audio_buffer_storage_write_pointer(work, 1),
                                     len, false);
    }

    // BusTrack processing boundary: pre-FX bus input is placed in the Track
    // work buffer, then the inherited Track chain processes it and the result
    // is published as the bus post-FX stage. No allocation is performed here.
    bool publishPostFx(uint32_t len) noexcept {
        if (!m_busState || len == 0 || len > kMaxSamples || !canProcess(len)) return false;
        void* work = workBufferStorage();
        return hirari_bus_audio_replace_post(
            m_busState, hirari_audio_buffer_storage_read_pointer(work, 0, 0),
            hirari_audio_buffer_storage_read_pointer(work, 1, 0), len);
    }

private:
    uint32_t m_busId = 0;
    void* m_busState = nullptr;
};

} // namespace Hirari::Core::Engine
