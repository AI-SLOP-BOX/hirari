#pragma once

#include <array>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <vector>

namespace Aura::Core::Engine {

/**
 * A fixed-capacity stereo summing node.
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

    Bus() = default;
    explicit Bus(uint32_t id) : m_id(id) {}

    uint32_t id() const { return m_id; }
    void setId(uint32_t id) { m_id = id; }

    bool addSamples(const float* l, const float* r, uint32_t len,
                    float gain = 1.0f) noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        if (!std::isfinite(gain)) return false;
        for (uint32_t i = 0; i < len; ++i) {
            const float left = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float right = std::isfinite(r[i]) ? r[i] : 0.0f;
            m_preL[i] += left * gain;
            m_preR[i] += right * gain;
        }
        m_samples.store(len, std::memory_order_release);
        return true;
    }

    bool replacePostSamples(const float* l, const float* r, uint32_t len) noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        std::memcpy(m_postL.data(), l, static_cast<size_t>(len) * sizeof(float));
        std::memcpy(m_postR.data(), r, static_cast<size_t>(len) * sizeof(float));
        m_samples.store(len, std::memory_order_release);
        return true;
    }

    void clear(uint32_t len) noexcept {
        const uint32_t n = std::min(len, kMaxSamples);
        std::fill_n(m_preL.data(), n, 0.0f);
        std::fill_n(m_preR.data(), n, 0.0f);
        std::fill_n(m_postL.data(), n, 0.0f);
        std::fill_n(m_postR.data(), n, 0.0f);
        m_samples.store(0, std::memory_order_release);
    }

    // Copies the pre-FX stage to the post-FX stage.  A BusTrack can replace
    // this stage with the result of its Track FX chain afterwards.
    bool commitDryToPost(uint32_t len) noexcept {
        if (len == 0 || len > kMaxSamples) return false;
        std::memcpy(m_postL.data(), m_preL.data(), static_cast<size_t>(len) * sizeof(float));
        std::memcpy(m_postR.data(), m_preR.data(), static_cast<size_t>(len) * sizeof(float));
        m_samples.store(len, std::memory_order_release);
        return true;
    }

    uint32_t samples() const noexcept { return m_samples.load(std::memory_order_acquire); }
    bool readPre(float* l, float* r, uint32_t len) const noexcept { return read(l, r, len, false); }
    bool readPost(float* l, float* r, uint32_t len) const noexcept { return read(l, r, len, true); }

private:
    bool read(float* l, float* r, uint32_t len, bool post) const noexcept {
        if (!l || !r || len == 0 || len > kMaxSamples) return false;
        const auto& left = post ? m_postL : m_preL;
        const auto& right = post ? m_postR : m_preR;
        std::memcpy(l, left.data(), static_cast<size_t>(len) * sizeof(float));
        std::memcpy(r, right.data(), static_cast<size_t>(len) * sizeof(float));
        return true;
    }

    uint32_t m_id = 0;
    alignas(64) std::array<float, kMaxSamples> m_preL{};
    std::array<float, kMaxSamples> m_preR{};
    alignas(64) std::array<float, kMaxSamples> m_postL{};
    std::array<float, kMaxSamples> m_postR{};
    std::atomic<uint32_t> m_samples{0};
};

class BusSystem {
public:
    BusSystem() {
        for (uint32_t i = 0; i < kMaxBuses; ++i) m_buses[i].setId(i);
    }
    static constexpr uint32_t kMaxBuses = 128;
    static BusSystem& getInstance() { static BusSystem instance; return instance; }

    Bus* registerBus(uint32_t id) noexcept {
        if (id >= kMaxBuses) return nullptr;
        m_buses[id].setId(id);
        m_present[id].store(true, std::memory_order_release);
        return &m_buses[id];
    }

    Bus* getBus(uint32_t id) noexcept {
        if (id >= kMaxBuses || !m_present[id].load(std::memory_order_acquire)) return nullptr;
        return &m_buses[id];
    }

    void process(uint32_t len) noexcept {
        if (len == 0 || len > Bus::kMaxSamples) return;
        // The order is control-thread published and read as a fixed array on
        // the audio thread.  Unlisted buses still get a safe dry commit.
        std::array<bool, kMaxBuses> visited{};
        const uint32_t count = m_orderCount.load(std::memory_order_acquire);
        for (uint32_t i = 0; i < count; ++i) {
            const uint32_t id = m_order[i];
            if (id < kMaxBuses && !visited[id]) {
                visited[id] = true;
                if (auto* bus = getBus(id)) bus->commitDryToPost(len);
            }
        }
        for (uint32_t id = 0; id < kMaxBuses; ++id) {
            if (!visited[id] && m_present[id].load(std::memory_order_acquire)) {
                m_buses[id].commitDryToPost(len);
            }
        }
    }

    // Control-thread API.  Only a bounded copy is published; the audio side
    // never retains or traverses the caller's vector.
    void updateRouting(const std::vector<uint32_t>& newOrder) noexcept {
        const uint32_t count = static_cast<uint32_t>(std::min<size_t>(newOrder.size(), kMaxBuses));
        for (uint32_t i = 0; i < count; ++i) m_order[i] = newOrder[i];
        m_orderCount.store(count, std::memory_order_release);
    }

    void clear(uint32_t len) noexcept {
        for (uint32_t id = 0; id < kMaxBuses; ++id) {
            if (m_present[id].load(std::memory_order_acquire)) m_buses[id].clear(len);
        }
    }

private:
    std::array<Bus, kMaxBuses> m_buses{};
    std::array<std::atomic<bool>, kMaxBuses> m_present{};
    std::array<uint32_t, kMaxBuses> m_order{};
    std::atomic<uint32_t> m_orderCount{0};
};

} // namespace Aura::Core::Engine
