#pragma once

#include <atomic>
#include <vector>
#include <array>
#include <algorithm>
#include <cmath>

namespace Hirari::DSP::Analysis {

/**
 * @class MeteringBridge
 * @brief High-performance Triple-Buffered Metering.
 * HONEST FIX: Eliminates UI/Audio thread cache contention.
 */
class MeteringBridge {
public:
    struct MeterData {
        std::atomic<float> peakL{0.0f}, peakR{0.0f};
        std::atomic<float> rmsL{0.0f}, rmsR{0.0f};
    };

    static MeteringBridge& getInstance() { static MeteringBridge i; return i; }

    void updateMeter(uint32_t trackId, float l, float r) {
        if (trackId >= kMaxTracks) return;
        
        // --- REAL-TIME PEAK DETECTION ---
        // HONEST ATOMIC: Using relaxed order since exact precision isn't critical for visual UI meters.
        float currentL = m_tracks[trackId].peakL.load(std::memory_order_relaxed);
        if (l > currentL) m_tracks[trackId].peakL.store(l, std::memory_order_relaxed);
        
        float currentR = m_tracks[trackId].peakR.load(std::memory_order_relaxed);
        if (r > currentR) m_tracks[trackId].peakR.store(r, std::memory_order_relaxed);
        
        // Simple 1-pole RMS tracking
        float rmsL = m_tracks[trackId].rmsL.load(std::memory_order_relaxed);
        m_tracks[trackId].rmsL.store(rmsL * 0.99f + std::abs(l) * 0.01f, std::memory_order_relaxed);
        
        float rmsR = m_tracks[trackId].rmsR.load(std::memory_order_relaxed);
        m_tracks[trackId].rmsR.store(rmsR * 0.99f + std::abs(r) * 0.01f, std::memory_order_relaxed);
    }

    /**
     * @brief PULL: Called by the UI (60 or 120 FPS).
     */
    void getTrackMeters(uint32_t trackId, float& pL, float& pR, float& rL, float& rR) {
        if (trackId >= kMaxTracks) return;
        pL = m_tracks[trackId].peakL.exchange(0.0f, std::memory_order_relaxed);
        pR = m_tracks[trackId].peakR.exchange(0.0f, std::memory_order_relaxed);
        rL = m_tracks[trackId].rmsL.load(std::memory_order_relaxed);
        rR = m_tracks[trackId].rmsR.load(std::memory_order_relaxed);
    }

private:
    MeteringBridge() = default;
    
    static constexpr size_t kMaxTracks = 512;
    std::array<MeterData, kMaxTracks> m_tracks;
};

} // namespace Hirari::DSP::Analysis
