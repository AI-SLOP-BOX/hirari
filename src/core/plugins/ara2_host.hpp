#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include <mutex>
#include <unordered_map>
#include <cstdint>
#include <cmath>
#include <limits>
#include <functional>
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
    struct ExternalProvider {
        std::function<bool(uint32_t, double, uint32_t, uint32_t, float*)> readInterleaved;
        std::function<bool(uint32_t, uint32_t)> requestAnalysis;
        std::function<void(uint32_t, uint32_t)> cancelAnalysis;
        std::function<bool(uint32_t, uint32_t)> bindRegion;
        std::function<void(uint32_t, uint32_t)> unbindRegion;
        std::function<bool(uint32_t, const ::aura::editing::AudioNoteSegment&)> setNoteSegment;
    };
    static ARA2Host& getInstance() { static ARA2Host i; return i; }

    void bindExternalProvider(ExternalProvider provider) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_externalProvider = std::move(provider);
    }

    void clearExternalProvider() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_externalProvider = {};
    }

    bool hasExternalProvider() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return static_cast<bool>(m_externalProvider.readInterleaved);
    }

    bool registerRegionWithPlugin(uint32_t pluginId, uint32_t regionId) {
        if (pluginId == 0 || regionId == 0) return false;
        std::function<bool(uint32_t, uint32_t)> bind;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_syncedRegions.find(regionId);
            if (it == m_syncedRegions.end() || it->second.expired()) return false;
            bind = m_externalProvider.bindRegion;
        }
        return bind ? bind(pluginId, regionId) : false;
    }

    bool syncRegionToPlugin(uint32_t pluginId, const std::shared_ptr<AudioRegion>& region) {
        if (!region || pluginId == 0) return false;
        registerRegion(region);
        return registerRegionWithPlugin(pluginId, region->getId());
    }

    bool unregisterRegionFromPlugin(uint32_t pluginId, uint32_t regionId) {
        if (pluginId == 0 || regionId == 0) return false;
        std::function<void(uint32_t, uint32_t)> unbind;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            unbind = m_externalProvider.unbindRegion;
        }
        if (!unbind) return false;
        unbind(pluginId, regionId);
        return true;
    }

    /**
     * @brief SYNC: Exchanges timeline metadata and audio handles with the plugin.
     */
    void registerRegion(const std::shared_ptr<AudioRegion>& region) {
        if (!region) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        // ARA document controllers may resend the same region after a
        // timeline edit. Keep one weak handle per region ID so random access
        // never observes duplicate providers.
        const uint32_t regionId = region->getId();
        const auto existing = m_syncedRegions.find(regionId);
        if (existing != m_syncedRegions.end()) {
            // A same-ID replacement is a new audio document from ARA's point
            // of view. Never expose the prior region's completed analysis.
            m_analysisState.erase(regionId);
        }
        m_syncedRegions[regionId] = region;
    }

    void unregisterRegion(uint32_t regionId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_syncedRegions.erase(regionId);
        m_analysisState.erase(regionId);
    }

    void clearDocument() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_syncedRegions.clear();
        m_analysisState.clear();
    }

    uint32_t registeredRegionCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return static_cast<uint32_t>(m_syncedRegions.size());
    }

    uint32_t pruneExpiredRegions() {
        std::lock_guard<std::mutex> lock(m_mutex);
        uint32_t removed = 0;
        for (auto it = m_syncedRegions.begin(); it != m_syncedRegions.end();) {
            if (it->second.expired()) {
                m_analysisState.erase(it->first);
                it = m_syncedRegions.erase(it);
                ++removed;
            } else {
                ++it;
            }
        }
        return removed;
    }

    /**
     * @brief ANALYZE: Allows the plugin to perform non-realtime pre-analysis.
     */
    bool requestAnalysis(uint32_t pluginId, uint32_t regionId = 0) {
        if (pluginId == 0) return false;
        std::function<bool(uint32_t, uint32_t)> request;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            if (regionId != 0) {
                const auto it = m_syncedRegions.find(regionId);
                if (it == m_syncedRegions.end() || it->second.expired()) return false;
            }
            request = m_externalProvider.requestAnalysis;
        }
        if (request && !request(pluginId, regionId)) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_analysisState[regionId] = AnalysisState{pluginId, false};
        return true;
    }

    bool markAnalysisReady(uint32_t pluginId, uint32_t regionId = 0) {
        if (pluginId == 0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_analysisState.find(regionId);
        if (it == m_analysisState.end() || it->second.pluginId != pluginId) return false;
        it->second.ready = true;
        return true;
    }

    bool cancelAnalysis(uint32_t pluginId, uint32_t regionId = 0) {
        if (pluginId == 0) return false;
        std::function<void(uint32_t, uint32_t)> cancel;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_analysisState.find(regionId);
            if (it == m_analysisState.end() || it->second.pluginId != pluginId) return false;
            cancel = m_externalProvider.cancelAnalysis;
            m_analysisState.erase(it);
        }
        if (cancel) cancel(pluginId, regionId);
        return true;
    }

    uint32_t analysisPluginId(uint32_t regionId = 0) const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_analysisState.find(regionId);
        return it == m_analysisState.end() ? 0u : it->second.pluginId;
    }

    bool analysisReady(uint32_t regionId = 0) const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (regionId != 0) {
            const auto region = m_syncedRegions.find(regionId);
            if (region == m_syncedRegions.end() || region->second.expired()) return false;
        }
        const auto it = m_analysisState.find(regionId);
        return it != m_analysisState.end() && it->second.ready;
    }

    bool setNoteSegment(uint32_t regionId, ::aura::editing::AudioNoteSegment segment) {
        if (!segment.valid()) return false;
        std::shared_ptr<AudioRegion> region;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_syncedRegions.find(regionId);
            if (it == m_syncedRegions.end()) return false;
            region = it->second.lock();
        }
        if (!region || !region->upsertAudioNoteSegment(segment)) return false;
        std::function<bool(uint32_t, const ::aura::editing::AudioNoteSegment&)> submit;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            submit = m_externalProvider.setNoteSegment;
        }
        return !submit || submit(regionId, segment);
    }

    bool clearNoteSegments(uint32_t regionId) {
        std::shared_ptr<AudioRegion> region;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_syncedRegions.find(regionId);
            if (it == m_syncedRegions.end()) return false;
            region = it->second.lock();
        }
        if (!region) return false;
        region->clearAudioNoteSegments();
        return true;
    }

    std::vector<::aura::editing::AudioNoteSegment> noteSegments(uint32_t regionId) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_syncedRegions.find(regionId);
        if (it == m_syncedRegions.end()) return {};
        const auto region = it->second.lock();
        return region ? region->getAudioNoteSegmentsSnapshot() : std::vector<::aura::editing::AudioNoteSegment>{};
    }

    /**
     * @brief Reads sample data from the timeline at an arbitrary random-access offset.
     * This is the core of ARA2's timeline-wide random access API, enabling plugins like Melodyne
     * to query audio data anywhere without waiting for linear playback.
     */
    bool readAudioSamples(uint32_t regionId, double sampleOffset, uint32_t numSamples, float* outBuffer) {
        if (!outBuffer || numSamples == 0 || !std::isfinite(sampleOffset) || sampleOffset < 0.0 ||
            sampleOffset > static_cast<double>(std::numeric_limits<uint64_t>::max())) return false;
        std::shared_ptr<AudioRegion> region;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_syncedRegions.find(regionId);
            if (it == m_syncedRegions.end()) return false;
            region = it->second.lock();
        }
        if (!region) return false;
        const uint64_t readStart = static_cast<uint64_t>(sampleOffset);
        const uint64_t totalSamples = region->getSampleLength();
        if (readStart >= totalSamples) return false;
        const uint32_t limit = static_cast<uint32_t>(std::min<uint64_t>(numSamples, totalSamples - readStart));
        for (uint32_t i = 0; i < limit; ++i) {
            const float value = region->getInterpolatedSample(0, sampleOffset + static_cast<double>(i));
            outBuffer[i] = std::isfinite(value) ? std::clamp(value, -16.0f, 16.0f) : 0.0f;
        }
        if (limit < numSamples) std::fill(outBuffer + limit, outBuffer + numSamples, 0.0f);
        return true;
    }

    // Interleaved random access used by ARA clients that request the complete
    // source channel set in one call.  The region source remains immutable;
    // this method only exposes a bounded read view and pads the tail.
    bool readAudioInterleaved(uint32_t regionId, double sampleOffset,
                              uint32_t numFrames, uint32_t channelCount,
                              float* outBuffer) {
        if (!outBuffer || numFrames == 0 || channelCount == 0 ||
            channelCount > 32 || !std::isfinite(sampleOffset) || sampleOffset < 0.0 ||
            sampleOffset > static_cast<double>(std::numeric_limits<uint64_t>::max())) return false;
        std::function<bool(uint32_t, double, uint32_t, uint32_t, float*)> externalRead;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            externalRead = m_externalProvider.readInterleaved;
        }
        if (externalRead)
            return externalRead(regionId, sampleOffset, numFrames, channelCount, outBuffer);
        std::shared_ptr<AudioRegion> region;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_syncedRegions.find(regionId);
            if (it == m_syncedRegions.end()) return false;
            region = it->second.lock();
        }
        const auto source = region ? region->getSource() : nullptr;
        if (!source || channelCount > source->getNumChannels()) return false;
        const uint64_t start = static_cast<uint64_t>(sampleOffset);
        const uint64_t total = region->getSampleLength();
        if (start >= total) return false;
        const uint32_t frames = static_cast<uint32_t>(std::min<uint64_t>(numFrames, total - start));
        for (uint32_t frame = 0; frame < frames; ++frame) {
            for (uint32_t channel = 0; channel < channelCount; ++channel) {
                const float value = region->getInterpolatedSample(
                    channel, sampleOffset + static_cast<double>(frame));
                outBuffer[static_cast<size_t>(frame) * channelCount + channel] =
                    std::isfinite(value) ? std::clamp(value, -16.0f, 16.0f) : 0.0f;
            }
        }
        std::fill(outBuffer + static_cast<size_t>(frames) * channelCount,
                  outBuffer + static_cast<size_t>(numFrames) * channelCount, 0.0f);
        return true;
    }

private:
    ARA2Host() = default;
    struct AnalysisState { uint32_t pluginId = 0; bool ready = false; };
    mutable std::mutex m_mutex;
    std::unordered_map<uint32_t, std::weak_ptr<AudioRegion>> m_syncedRegions;
    std::unordered_map<uint32_t, AnalysisState> m_analysisState;
    ExternalProvider m_externalProvider;
};

} // namespace Aura::Core::Plugins
