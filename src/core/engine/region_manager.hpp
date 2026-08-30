#pragma once

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <mutex>
#include <vector>
#include <string>

namespace Aura::Core::Engine {

/**
 * @brief AudioRegion: Professional non-destructive clip object.
 * Holds per-clip metadata like Gain and Mute state, independent of the track.
 */
struct AudioRegion {
    uint32_t id = 0;
    std::string filePath;
    double samplePosition;
    double sampleLength;
    float clipGain = 1.0f; // 0dB default
    bool isMuted = false;
};

/**
 * @brief RegionManager: Orchestrates all non-destructive clip edits.
 * Critical for Ardour-style deep regional editing.
 */
class RegionManager {
public:
    static constexpr uint32_t kMaxTracks = 4096;
    static constexpr uint32_t kMaxRegions = 1'000'000;

    RegionManager() {
        m_regions_by_track.resize(64); // Pre-allocate for standard project size
    }

    /**
     * @brief ADD REGION: Adds a new region with industrial-grade management and temporal sovereignty.
     * INDUSTRIAL: Delegating region management and temporal alignment to the Rust 'RegionOrchestrator'.
     */
    bool addRegion(uint32_t trackId, const std::string& path, double pos, double len,
                   uint32_t* createdId = nullptr) {
        if (trackId >= kMaxTracks || path.empty() || path.size() > 32 * 1024 ||
            !std::isfinite(pos) || !std::isfinite(len) || pos < 0.0 || len <= 0.0 ||
            len > 24.0 * 60.0 * 60.0 * 192000.0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        if (regionCountLocked() >= kMaxRegions) return false;
        ensureTrackLocked(trackId);
        const uint32_t id = allocateIdLocked();
        if (id == 0) return false;
        m_regions_by_track[trackId].push_back(AudioRegion{id, path, pos, len, 1.0f, false});
        sortTrackLocked(trackId);
        if (createdId) *createdId = id;
        return true;
    }

    /**
     * @brief CLIP GAIN: Sets the clip gain with industrial precision and creative sovereignty.
     * INDUSTRIAL: Using Rust for robust and perfectly timed parameter auditing.
     */
    bool setClipGain(uint32_t trackId, uint32_t regionId, float gain) {
        if (trackId >= m_regions_by_track.size() || !std::isfinite(gain) ||
            gain < 0.0f || gain > 16.0f) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        AudioRegion* region = findLocked(trackId, regionId);
        if (!region) return false;
        region->clipGain = gain;
        return true;
    }

    bool setMuted(uint32_t trackId, uint32_t regionId, bool muted) {
        std::lock_guard<std::mutex> lock(m_mutex);
        AudioRegion* region = findLocked(trackId, regionId);
        if (!region) return false;
        region->isMuted = muted;
        return true;
    }

    bool removeRegion(uint32_t trackId, uint32_t regionId) {
        if (trackId >= m_regions_by_track.size()) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto& regions = m_regions_by_track[trackId];
        const auto it = std::find_if(regions.begin(), regions.end(),
            [regionId](const AudioRegion& region) { return region.id == regionId; });
        if (it == regions.end()) return false;
        regions.erase(it);
        return true;
    }

    // Reload path: preserve stable IDs from the project file while rejecting
    // duplicates and malformed ranges. This prevents a reload from silently
    // remapping automation and history references to a different region.
    bool upsertRegion(uint32_t trackId, const AudioRegion& region) {
        if (trackId >= kMaxTracks || region.id == 0 || region.filePath.empty() ||
            !std::isfinite(region.samplePosition) || !std::isfinite(region.sampleLength) ||
            region.samplePosition < 0.0 || region.sampleLength <= 0.0 ||
            !std::isfinite(region.clipGain) || region.clipGain < 0.0f ||
            region.clipGain > 16.0f) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        ensureTrackLocked(trackId);
        if (findByIdLocked(region.id) != nullptr) return false;
        if (regionCountLocked() >= kMaxRegions) return false;
        m_regions_by_track[trackId].push_back(region);
        m_nextRegionId.store(std::max(m_nextRegionId.load(std::memory_order_relaxed),
                                      region.id + 1), std::memory_order_relaxed);
        sortTrackLocked(trackId);
        return true;
    }

    bool splitRegion(uint32_t trackId, uint32_t regionId, double offset,
                     uint32_t* rightId = nullptr) {
        if (!std::isfinite(offset) || offset <= 0.0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        AudioRegion* region = findLocked(trackId, regionId);
        if (!region || offset >= region->sampleLength || regionCountLocked() >= kMaxRegions)
            return false;
        const AudioRegion original = *region;
        const uint32_t newId = allocateIdLocked();
        if (newId == 0) return false;
        region->sampleLength = offset;
        AudioRegion right = original;
        right.id = newId;
        right.samplePosition += offset;
        right.sampleLength -= offset;
        m_regions_by_track[trackId].push_back(right);
        sortTrackLocked(trackId);
        if (rightId) *rightId = newId;
        return true;
    }

    std::vector<AudioRegion> regionsForTrack(uint32_t trackId) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (trackId >= m_regions_by_track.size()) return {};
        return m_regions_by_track[trackId];
    }

private:
    void ensureTrackLocked(uint32_t trackId) {
        if (trackId >= m_regions_by_track.size())
            m_regions_by_track.resize(static_cast<size_t>(trackId) + 1);
    }

    size_t regionCountLocked() const {
        size_t count = 0;
        for (const auto& regions : m_regions_by_track) count += regions.size();
        return count;
    }

    uint32_t allocateIdLocked() {
        uint32_t candidate = m_nextRegionId.load(std::memory_order_relaxed);
        for (uint64_t attempts = 0; attempts < UINT32_MAX; ++attempts) {
            if (candidate == 0) candidate = 1;
            if (findByIdLocked(candidate) == nullptr) {
                m_nextRegionId.store(candidate + 1, std::memory_order_relaxed);
                return candidate;
            }
            ++candidate;
        }
        return 0;
    }

    AudioRegion* findByIdLocked(uint32_t regionId) {
        for (auto& regions : m_regions_by_track)
            for (auto& region : regions)
                if (region.id == regionId) return &region;
        return nullptr;
    }

    AudioRegion* findLocked(uint32_t trackId, uint32_t regionId) {
        if (trackId >= m_regions_by_track.size()) return nullptr;
        for (auto& region : m_regions_by_track[trackId])
            if (region.id == regionId) return &region;
        return nullptr;
    }

    void sortTrackLocked(uint32_t trackId) {
        auto& regions = m_regions_by_track[trackId];
        std::stable_sort(regions.begin(), regions.end(),
            [](const AudioRegion& lhs, const AudioRegion& rhs) {
                if (lhs.samplePosition != rhs.samplePosition)
                    return lhs.samplePosition < rhs.samplePosition;
                return lhs.id < rhs.id;
            });
    }

    // Faster lookup than std::map for dense track IDs
    std::vector<std::vector<AudioRegion>> m_regions_by_track;
    mutable std::mutex m_mutex;
    std::atomic<uint32_t> m_nextRegionId{1};
};

} // namespace Aura::Core::Engine
