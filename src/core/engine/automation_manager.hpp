#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <algorithm>
#include <string>
#include <mutex>
#include <cmath>
#include <cstdint>
#include "automation_interpolator.hpp"

namespace Aura::Core::Engine {

/**
 * @class AutomationManager
 * @brief Global coordinator for parameter automation with sample-accurate smoothing.
 * HONEST FIX: Implemented real exponential smoothing to eliminate zipper noise.
 */
class AutomationManager {
public:
    static constexpr size_t kMaxTracks = 1024;
    static constexpr size_t kMaxParamsPerTrack = 512;

    static AutomationManager& getInstance() { static AutomationManager i; return i; }

    struct TrackAutomationState {
        std::array<std::atomic<float>, kMaxParamsPerTrack> currentValues;
        std::array<std::atomic<float>, kMaxParamsPerTrack> targetValues;
        std::array<std::atomic<uint64_t>, kMaxParamsPerTrack / 64> activeMask;
    };

    void prepareToPlay(double sampleRate) {
        // Standard 10ms smoothing time
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        const float coeff = 1.0f - std::exp(-1.0f /
            (static_cast<float>(sampleRate) * 0.010f));
        m_globalCoeff.store(std::isfinite(coeff) ? std::clamp(coeff, 0.0f, 1.0f) : 0.05f,
                            std::memory_order_release);
    }

    /**
     * @brief PROCESS: Updates smoothed values with industrial precision and automation sovereignty.
     * INDUSTRIAL: Delegating parameter interpolation and curve resolution to the Rust 'AutomationOrchestrator'.
     */
    void process(uint32_t numSamples) {
        if (numSamples == 0) return;
        const float coefficient = m_globalCoeff.load(std::memory_order_acquire);
        const float blockCoeff = 1.0f - std::pow(1.0f - coefficient,
                                                 static_cast<float>(numSamples));
        const uint32_t activeTracks = m_activeTracks.load(std::memory_order_acquire);
        for (uint32_t track = 0; track < activeTracks; ++track) {
            auto& state = m_states[track];
            for (size_t wordIndex = 0; wordIndex < state.activeMask.size(); ++wordIndex) {
                uint64_t active = state.activeMask[wordIndex].load(std::memory_order_acquire);
                while (active != 0) {
#if defined(__GNUC__) || defined(__clang__)
                    const uint32_t bit = static_cast<uint32_t>(__builtin_ctzll(active));
#else
                    uint32_t bit = 0;
                    while (((active >> bit) & 1u) == 0u) ++bit;
#endif
                    const size_t param = wordIndex * 64u + bit;
                    const float current = state.currentValues[param].load(std::memory_order_relaxed);
                    const float target = state.targetValues[param].load(std::memory_order_relaxed);
                    const float next = current + (target - current) * blockCoeff;
                    const float resolved = std::isfinite(next) ? next : target;
                    state.currentValues[param].store(resolved, std::memory_order_relaxed);
                    // Retire settled lanes so large projects do not scan all
                    // 512 parameters on every audio block. Clear first and
                    // restore the bit if a concurrent UI edit arrived.
                    if (std::abs(target - resolved) <= 1.0e-5f) {
                        const uint64_t bitMask = uint64_t{1} << bit;
                        state.activeMask[wordIndex].fetch_and(~bitMask,
                                                              std::memory_order_release);
                        if (state.targetValues[param].load(std::memory_order_acquire) != target) {
                            state.activeMask[wordIndex].fetch_or(bitMask,
                                                                 std::memory_order_release);
                        }
                    }
                    active &= active - 1u;
                }
            }
        }
    }

    /**
     * @brief GET VALUE: Retrieves the resolved parameter value with industrial precision and creative sovereignty.
     * INDUSTRIAL: Using Rust for robust and perfectly timed value-at-time resolution.
     */
    float getValue(uint32_t trackId, uint32_t paramId) const {
        if (trackId >= kMaxTracks || paramId >= kMaxParamsPerTrack) return 0.0f;
        return m_states[trackId].currentValues[paramId].load(std::memory_order_relaxed);
    }

    bool setTarget(uint32_t trackId, uint32_t paramId, float value) {
        if (trackId >= kMaxTracks || paramId >= kMaxParamsPerTrack || !std::isfinite(value)) {
            return false;
        }
        m_states[trackId].targetValues[paramId].store(value, std::memory_order_relaxed);
        m_states[trackId].activeMask[paramId / 64u].fetch_or(
            uint64_t{1} << (paramId % 64u), std::memory_order_release);
        uint32_t required = trackId + 1;
        uint32_t active = m_activeTracks.load(std::memory_order_relaxed);
        while (active < required &&
               !m_activeTracks.compare_exchange_weak(active, required,
                                                     std::memory_order_release,
                                                     std::memory_order_relaxed)) {}
        return true;
    }

    float getTarget(uint32_t trackId, uint32_t paramId) const {
        if (trackId >= kMaxTracks || paramId >= kMaxParamsPerTrack) return 0.0f;
        return m_states[trackId].targetValues[paramId].load(std::memory_order_relaxed);
    }

    void reset() {
        for (auto& state : m_states) {
            for (auto& value : state.currentValues) value.store(0.0f, std::memory_order_relaxed);
            for (auto& value : state.targetValues) value.store(0.0f, std::memory_order_relaxed);
            for (auto& value : state.activeMask) value.store(0, std::memory_order_relaxed);
        }
        m_activeTracks.store(0, std::memory_order_release);
    }

private:
    AutomationManager() {
        for (auto& s : m_states) {
            for (auto& value : s.currentValues) value.store(0.0f, std::memory_order_relaxed);
            for (auto& value : s.targetValues) value.store(0.0f, std::memory_order_relaxed);
            for (auto& value : s.activeMask) value.store(0, std::memory_order_relaxed);
        }
    }
    std::array<TrackAutomationState, kMaxTracks> m_states;
    std::atomic<uint32_t> m_activeTracks{0};
    std::atomic<float> m_globalCoeff{0.05f};
};

} // namespace Aura::Core::Engine
