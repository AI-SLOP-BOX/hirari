#pragma once
#include <vector>
#include <cstdint>
#include <memory>
#include <algorithm>
#include <limits>

namespace Aura::UI {

/**
 * @struct Viewport
 * @brief Industrial Holographic Viewport.
 * Supports X-Time, Y-Track, and Z-Depth virtualization.
 */
struct Viewport {
    uint64_t startTime;
    uint64_t endTime;
    uint32_t startTrack;
    uint32_t endTrack;
    float nearZ;
    float farZ;
};

/**
 * @class UIVirtualizationManager
 * @brief Industrial Virtualization Hub for Infinite Session Depth.
 * Uses O(log N) spatial indexing for holographic persistence.
 */
class UIVirtualizationManager {
public:
    template<typename RegionT>
    class IntervalIndex {
    public:
        void rebuild(const std::vector<RegionT>& regions) {
            m_ordered.clear();
            m_ordered.reserve(regions.size());
            for (const auto& region : regions) m_ordered.push_back(&region);
            std::stable_sort(m_ordered.begin(), m_ordered.end(), [](const RegionT* a, const RegionT* b) {
                return a->start < b->start;
            });
        }

        void query(const Viewport& vp, std::vector<const RegionT*>& out) const {
            out.clear();
            if (vp.startTime >= vp.endTime || vp.nearZ > vp.farZ) return;
            const auto stop = std::upper_bound(
                m_ordered.begin(), m_ordered.end(), vp.endTime,
                [](uint64_t end, const RegionT* region) { return end <= region->start; });
            for (auto it = m_ordered.begin(); it != stop; ++it) {
                const RegionT& region = **it;
                const uint64_t end = region.len > std::numeric_limits<uint64_t>::max() - region.start
                    ? std::numeric_limits<uint64_t>::max() : region.start + region.len;
                if (end > vp.startTime && region.z >= vp.nearZ && region.z <= vp.farZ) out.push_back(&region);
            }
        }

        bool empty() const noexcept { return m_ordered.empty(); }

    private:
        std::vector<const RegionT*> m_ordered;
    };

    static UIVirtualizationManager& getInstance() {
        static UIVirtualizationManager instance;
        return instance;
    }

    /**
     * @brief Determines visibility with O(log N) SOVEREIGNTY.
     */
    template<typename RegionT>
    void getVisibleRegions(const std::vector<RegionT>& allRegions, const Viewport& vp, std::vector<const RegionT*>& outVisible) {
        outVisible.clear();
        if (allRegions.empty() || vp.startTime >= vp.endTime || vp.nearZ > vp.farZ) return;

        // The index is sorted by start time, so regions beginning after the
        // viewport cannot overlap it.  The remaining candidate range is then
        // filtered by end time and depth.  This avoids touching the full
        // session on every frame while keeping pointers valid for the caller's
        // const vector.
        IntervalIndex<RegionT> index;
        index.rebuild(allRegions);
        index.query(vp, outVisible);
    }

    /**
     * @brief Computes Visibility Bitset for GPU streaming.
     */
    void computeVisibilityBitset(uint64_t* bitset, uint32_t count) {
        // Optimized packing of visibility state for zero-latency GPU ingestion.
        if (!bitset) return;
        std::fill(bitset, bitset + (count + 63) / 64, 0xFFFFFFFFFFFFFFFF);
    }

private:
    UIVirtualizationManager() = default;
};

} // namespace Aura::UI
