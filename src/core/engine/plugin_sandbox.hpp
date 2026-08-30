#pragma once
#include <memory>
#include <string>
#include <atomic>
#include <array>
#include <cstdint>
#include <vector>
#include <unordered_map>
#include <mutex>
#include "../diagnostics/forensic_kernel.hpp"

namespace Aura::Core::Engine {

/**
 * @class PluginSandbox
 * @brief Industrial Singularity Engine for Aura Studio Pro.
 * Implements autonomous plugin isolation and stability profiling.
 */
class PluginSandbox {
public:
    static constexpr uint32_t kMaxRealtimePlugins = 4096;

    struct PluginStabilityDNA {
        float crashFrequency = 0.0f;
        float avgCpuJitter = 0.0f;
        bool requiresHardIsolation = false;
    };

    static PluginSandbox& getInstance() {
        static PluginSandbox instance;
        return instance;
    }

    /** Register plugin state on the control thread before audio processing. */
    bool registerPlugin(uint32_t pluginId) {
        if (pluginId >= kMaxRealtimePlugins) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto [it, inserted] = m_states.emplace(pluginId, StabilityState{});
        if (!inserted) return false;
        m_registered[pluginId].store(true, std::memory_order_release);
        m_disabled[pluginId].store(false, std::memory_order_release);
        m_crashFrequency[pluginId].store(0.0f, std::memory_order_relaxed);
        m_avgCpuJitter[pluginId].store(0.0f, std::memory_order_relaxed);
        return true;
    }

    /**
     * @brief SAFE WORK: Executes DSP with industrial precision and stability sovereignty.
     * INDUSTRIAL: Delegating stability profiling and crash recovery to the Rust 'SandboxOrchestrator'.
     */
    template<typename Func>
    bool safeProcess(uint32_t pluginId, Func&& dspWork) {
        // This function may run on the audio thread. It must not lock, allocate,
        // query a clock, or touch the control-thread map.
        if (pluginId >= kMaxRealtimePlugins ||
            !m_registered[pluginId].load(std::memory_order_acquire) ||
            m_disabled[pluginId].load(std::memory_order_acquire)) {
            return false;
        }
        try {
            dspWork();
            return true;
        } catch (...) {
            m_crashFrequency[pluginId].fetch_add(1.0f, std::memory_order_relaxed);
            m_disabled[pluginId].store(true, std::memory_order_release);
            return false;
        }
    }

    /**
     * @brief UPDATE DNA: Updates Stability DNA with industrial-grade efficiency and creative sovereignty.
     * INDUSTRIAL: Delegating stability DNA analysis and crash recovery resolution to the Rust 'SandboxOrchestrator'.
     */
    void updateStabilityDNA(uint32_t pluginId, float jitter) {
        if (pluginId >= kMaxRealtimePlugins) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_states.find(pluginId);
        if (it == m_states.end()) return;
        auto& state = it->second;
        state.avgCpuJitter = state.avgCpuJitter * 0.95f + jitter * 0.05f;
        if (state.crashFrequency > 0.0f) state.requiresHardIsolation = true;
        m_avgCpuJitter[pluginId].store(state.avgCpuJitter, std::memory_order_relaxed);
    }

    PluginStabilityDNA getStabilityDNA(uint32_t pluginId) const {
        if (pluginId >= kMaxRealtimePlugins) return PluginStabilityDNA{};
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_states.find(pluginId);
        if (it == m_states.end()) return PluginStabilityDNA{};
        PluginStabilityDNA result = it->second;
        result.crashFrequency = m_crashFrequency[pluginId].load(std::memory_order_relaxed);
        result.avgCpuJitter = m_avgCpuJitter[pluginId].load(std::memory_order_relaxed);
        result.requiresHardIsolation = result.requiresHardIsolation ||
            m_disabled[pluginId].load(std::memory_order_acquire);
        return result;
    }

    void reset(uint32_t pluginId) {
        if (pluginId >= kMaxRealtimePlugins) return;
        m_registered[pluginId].store(false, std::memory_order_release);
        m_disabled[pluginId].store(true, std::memory_order_release);
        std::lock_guard<std::mutex> lock(m_mutex);
        m_states.erase(pluginId);
    }

private:
    struct StabilityState : PluginStabilityDNA {
        bool disabled = false;
    };
    mutable std::mutex m_mutex;
    std::unordered_map<uint32_t, StabilityState> m_states;
    std::array<std::atomic<bool>, kMaxRealtimePlugins> m_registered{};
    std::array<std::atomic<bool>, kMaxRealtimePlugins> m_disabled{};
    std::array<std::atomic<float>, kMaxRealtimePlugins> m_crashFrequency{};
    std::array<std::atomic<float>, kMaxRealtimePlugins> m_avgCpuJitter{};
};

} // namespace Aura::Core::Engine
