#pragma once

#include <array>
#include <algorithm>
#include <cstdint>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * C++ handle for a Rust-owned fixed-capacity stereo summing node.
 *
 * The bus owns both sides of the stage boundary:
 *   contributors -> pre-FX -> BusTrack/FX -> post-FX
 * No vector growth, mutex, or reference-count operation is required by the
 * audio-side methods.  The capacity is deliberately bounded so an invalid
 * driver block is rejected instead of allocating in the callback.
 */
class Bus {
public:
    static constexpr uint32_t kMaxSamples = 8192;

    Bus() : m_audioState(hirari_bus_audio_create()) {}
    explicit Bus(uint32_t id) : Bus() { m_id = id; }
    ~Bus() { hirari_bus_audio_destroy(m_audioState); }
    Bus(const Bus&) = delete;
    Bus& operator=(const Bus&) = delete;
    Bus(Bus&&) = delete;
    Bus& operator=(Bus&&) = delete;

    uint32_t id() const { return m_id; }
    void setId(uint32_t id) { m_id = id; }
    void* nativeAudioState() const noexcept { return m_audioState; }

    bool addSamples(const float* l, const float* r, uint32_t len,
                    float gain = 1.0f) noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        if (!std::isfinite(gain)) return false;
        return hirari_bus_audio_accumulate(m_audioState, l, r, len, gain);
    }

    bool replacePostSamples(const float* l, const float* r, uint32_t len) noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        return hirari_bus_audio_replace_post(m_audioState, l, r, len);
    }

    void clear(uint32_t len) noexcept {
        hirari_bus_audio_clear(m_audioState, std::min(len, kMaxSamples));
    }

    // Copies the pre-FX stage to the post-FX stage.  A BusTrack can replace
    // this stage with the result of its Track FX chain afterwards.
    bool commitDryToPost(uint32_t len) noexcept {
        if (len == 0 || len > kMaxSamples) return false;
        return hirari_bus_audio_commit_dry(m_audioState, len);
    }

    uint32_t samples() const noexcept { return hirari_bus_audio_samples(m_audioState); }
    bool readPre(float* l, float* r, uint32_t len) const noexcept { return read(l, r, len, false); }
    bool readPost(float* l, float* r, uint32_t len) const noexcept { return read(l, r, len, true); }

private:
    friend class BusSystem;
    bool read(float* l, float* r, uint32_t len, bool post) const noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        return hirari_bus_audio_read(m_audioState, l, r, len, post);
    }

    uint32_t m_id = 0;
    void* m_audioState = nullptr;
};

class BusSystem {
public:
    BusSystem() {
        for (uint32_t i = 0; i < kMaxBuses; ++i) {
            m_buses[i].setId(i);
            hirari_bus_system_bind(m_state, i, m_buses[i].m_audioState);
        }
    }
    ~BusSystem() { hirari_bus_system_destroy(m_state); }
    BusSystem(const BusSystem&) = delete;
    BusSystem& operator=(const BusSystem&) = delete;
    static constexpr uint32_t kMaxBuses = 128;
    static BusSystem& getInstance() { static BusSystem instance; return instance; }

    Bus* registerBus(uint32_t id) noexcept {
        if (id >= kMaxBuses) return nullptr;
        m_buses[id].setId(id);
        return hirari_bus_system_register(m_state, id) ? &m_buses[id] : nullptr;
    }

    // Publish the buses owned by the current project. Bus storage is fixed
    // for the lifetime of the engine, so removal only changes membership; a
    // callback holding an older BusTrack pointer remains memory-safe.
    void reconcileBuses(const std::array<bool, kMaxBuses>& active) noexcept {
        for (uint32_t id = 0; id < kMaxBuses; ++id) {
            if (active[id]) m_buses[id].setId(id);
        }
        hirari_bus_system_reconcile(m_state, active.data(), kMaxBuses);
    }

    void resetForProject() noexcept {
        hirari_bus_system_reset(m_state);
    }

    Bus* getBus(uint32_t id) noexcept {
        if (id >= kMaxBuses || !hirari_bus_system_is_present(m_state, id)) return nullptr;
        return &m_buses[id];
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
    std::array<Bus, kMaxBuses> m_buses{};
    void* m_state = hirari_bus_system_create();
};

} // namespace Hirari::Core::Engine
