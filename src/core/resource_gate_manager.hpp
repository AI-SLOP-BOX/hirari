#pragma once
#include <vector>
#include <atomic>
#include <cmath>
#include <array>
#include <memory>
#include <mutex>
#include "../dsp/analysis/psychoacoustic_model.hpp"
#include "../dsp/simd/simd_kernel.hpp"

namespace Aura::Core::Engine {

/**
 * @class ResourceGateManager
 * @brief Manages track processing suspension (gating) based on audio levels.
 * HONEST FIX: Resolved critical data race by using atomic status management.
 */
class ResourceGateManager {
public:
    static constexpr size_t kMaxSupportedTracks = 2048;

    static ResourceGateManager& getInstance() { static ResourceGateManager i; return i; }

    void prepareToPlay(double sr) {
        m_sampleRate = sr;
        m_tailThresholdSamples = static_cast<uint64_t>(2.0 * sr);
    }

    /**
     * @brief Analyzes signal and updates gating status with thread-safety.
     */
    void update(uint32_t trackID, const float* l, const float* r, uint32_t numSamples, float mixRMS) {
        if (trackID >= kMaxSupportedTracks) return;

        float sum = (l && r) ? SIMD::SIMDKernel::sumSquares(l, r, numSamples) : 0;
        float rms = std::sqrt(sum / (numSamples * 2 + 1e-6f));
        float db = 20.0f * std::log10(std::max(rms, 1e-6f));
        float mixDb = 20.0f * std::log10(std::max(mixRMS, 1e-6f));

        float importance = m_psyModel.getPerceptualImportance(db, mixDb);
        
        auto& status = m_statuses[trackID];
        if (importance < 0.1f) {
            uint64_t current = status.silenceSamples.fetch_add(numSamples, std::memory_order_relaxed) + numSamples;
            if (current > m_tailThresholdSamples) { 
                status.gated.store(true, std::memory_order_release);
            }
        } else {
            status.silenceSamples.store(0, std::memory_order_relaxed);
            status.gated.store(false, std::memory_order_release);
        }
    }

    bool isGated(uint32_t trackID) const {
        if (trackID >= kMaxSupportedTracks) return false;
        return m_statuses[trackID].gated.load(std::memory_order_acquire);
    }

private:
    ResourceGateManager() : m_sampleRate(44100.0), m_tailThresholdSamples(88200) {
        for (size_t i = 0; i < kMaxSupportedTracks; ++i) {
            m_statuses[i].gated.store(false);
            m_statuses[i].silenceSamples.store(0);
        }
    }
    
    struct GateStatus {
        std::atomic<bool> gated;
        std::atomic<uint64_t> silenceSamples;
    };

    // Pre-allocated for thread-safety and RT-performance
    std::array<GateStatus, kMaxSupportedTracks> m_statuses;
    
    DSP::Analysis::PsychoacousticModel m_psyModel;
    double m_sampleRate;
    uint64_t m_tailThresholdSamples;
};

} // namespace Aura::Core::Engine
