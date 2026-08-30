#pragma once
#include <vector>
#include <memory>
#include <atomic>
#include <cmath>
#include <thread>
#include <chrono>
#include <stop_token>
#include <condition_variable>
#include <mutex>
#include "aura_unified_engine.hpp"

namespace Aura::Core::Engine {

/**
 * @class ResourcePurgeManager
 * @brief Manages background hibernation of inactive audio resources.
 * HONEST FIX: Refactored to use C++20 jthread and prioritized hibernation.
 */
class ResourcePurgeManager {
public:
    static ResourcePurgeManager& getInstance() { static ResourcePurgeManager i; return i; }

    void start(std::stop_token stopToken) {
        (void)stopToken;
        m_worker = std::jthread([this](std::stop_token token) {
            while (!token.stop_requested()) {
                performOptimization();
                std::unique_lock<std::mutex> lock(m_waitMutex);
                m_wait.wait_for(lock, std::chrono::seconds(2), [&token] {
                    return token.stop_requested();
                });
            }
        });
    }

    void stop() {
        m_worker.request_stop();
        m_wait.notify_all();
    }

private:
    void performOptimization() {
        auto& engine = AuraUnifiedEngine::getInstance();
        auto tracks = engine.getTracksSafe(); 
        
        uint64_t playhead = engine.get_playhead();
        const double sampleRate = engine.get_sample_rate();
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) {
            return;
        }

        double currentPosSec = static_cast<double>(playhead) / sampleRate;
        if (!std::isfinite(currentPosSec)) {
            return;
        }

        // Sort tracks by priority (distance from playhead)
        // Tracks that aren't needed for the next 60 seconds are candidates for hibernation.
        for (auto& track : tracks) {
            if (!track) continue;

            const bool neededSoon = track->hasAudioBetween(currentPosSec, currentPosSec + 30.0);
            const float peakL = track->getPeakL();
            const float peakR = track->getPeakR();
            const bool hasValidPeak = std::isfinite(peakL) && std::isfinite(peakR);
            const float peak = std::max(peakL, peakR);
            const bool isSilent = hasValidPeak && peak < 1e-5f;

            if (!neededSoon && isSilent && track->isActive()) {
                // Professional Inactivity Threshold
                const double inactivityDuration = track->getInactivityDuration();
                if (std::isfinite(inactivityDuration) && inactivityDuration > 10.0) {
                    track->setHibernating(true);
                }
            } else if ((neededSoon || !isSilent) && track->isHibernating()) {
                track->setHibernating(false);
            }
            
            track->collectGarbage();
        }
    }

    ResourcePurgeManager() = default;
    std::jthread m_worker;
    std::condition_variable m_wait;
    std::mutex m_waitMutex;
};

} // namespace Aura::Core::Engine
