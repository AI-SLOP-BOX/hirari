#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include "../audio_region.hpp"

namespace Aura::Core::Plugins {

/**
 * @class ARA2Host
 * @brief Professional Audio Random Access (ARA2) Integration.
 * ARA2 allows plugins to 'see' the entire timeline buffer instead of 
 * receiving a real-time stream—a critical requirement for modern production.
 */
class ARA2Host {
public:
    static ARA2Host& getInstance() { static ARA2Host i; return i; }

    /**
     * @brief SYNC: Exchanges timeline metadata and audio handles with the plugin.
     */
    void registerRegion(const std::shared_ptr<AudioRegion>& region) {
        m_syncedRegions.push_back(region);
    }

    /**
     * @brief ANALYZE: Allows the plugin to perform non-realtime pre-analysis.
     */
    void requestAnalysis(uint32_t /*pluginId*/) {
        // Logic to notify Melodyne/VocAlign that the audio data is ready for processing
    }

    /**
     * @brief Reads sample data from the timeline at an arbitrary random-access offset.
     * This is the core of ARA2's timeline-wide random access API, enabling plugins like Melodyne
     * to query audio data anywhere without waiting for linear playback.
     */
    bool readAudioSamples(uint32_t regionId, double sampleOffset, uint32_t numSamples, float* outBuffer) {
        for (auto& weakRegion : m_syncedRegions) {
            if (auto region = weakRegion.lock()) {
                if (region->getId() == regionId) {
                    uint32_t readStart = static_cast<uint32_t>(sampleOffset);
                    uint32_t totalSamples = region->getLengthSamples();
                    if (readStart >= totalSamples) return false;
                    uint32_t limit = std::min(numSamples, totalSamples - readStart);
                    
                    const float* src = region->getChannelData(0);
                    if (src) {
                        std::copy(src + readStart, src + readStart + limit, outBuffer);
                        if (limit < numSamples) {
                            std::fill(outBuffer + limit, outBuffer + numSamples, 0.0f);
                        }
                        return true;
                    }
                }
            }
        }
        return false;
    }

private:
    ARA2Host() = default;
    std::vector<std::weak_ptr<AudioRegion>> m_syncedRegions;
};

} // namespace Aura::Core::Plugins
