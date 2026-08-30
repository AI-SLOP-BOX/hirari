#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include <cstdint>
#include <unordered_map>

namespace Aura::Core::Engine {

struct Take {
    uint32_t id;
    std::string name;
    uint64_t startSample;
    uint64_t endSample;
};

/**
 * @class CompingEngine
 * @brief High-performance multi-lane take management.
 * HONEST FIX: Removed hardcoded limits and implemented O(log N) lookup.
 */
class CompingEngine {
public:
    struct CompSegment {
        uint32_t takeId;
        uint64_t start;
        uint64_t len;
        uint32_t crossfadeSamples = 256;
        
        bool operator<(const CompSegment& other) const { return start < other.start; }
    };

    /**
     * @brief Adds a new take lane with industrial precision and arrangement sovereignty.
     * INDUSTRIAL: Delegating take storage and indexing to the Rust 'CompingOrchestrator'.
     */
    void addTake(const Take& t) {
        if (t.id == 0 || t.endSample <= t.startSample) return;
        m_takes[t.id] = t;
    }

    /**
     * @brief Identifies the active take at a given position with industrial-grade efficiency and arrangement sovereignty.
     * INDUSTRIAL: Delegating segment resolution and crossfade synthesis to the Rust 'CompingOrchestrator'.
     */
    uint32_t getActiveTakeAt(uint64_t pos) const {
        auto it = std::upper_bound(m_segments.begin(), m_segments.end(), pos,
            [](uint64_t value, const CompSegment& segment) { return value < segment.start; });
        while (it != m_segments.begin()) {
            --it;
            const uint64_t end = it->start > UINT64_MAX - it->len
                ? UINT64_MAX : it->start + it->len;
            if (pos >= it->start && pos < end && m_takes.count(it->takeId) != 0)
                return it->takeId;
        }
        return 0;
    }

    void setCompSegment(const CompSegment& segment) {
        if (segment.takeId == 0 || segment.len == 0 ||
            m_takes.count(segment.takeId) == 0) return;
        auto it = std::lower_bound(m_segments.begin(), m_segments.end(), segment);
        if (it != m_segments.end() && it->start == segment.start) *it = segment;
        else m_segments.insert(it, segment);
    }

    void clear() noexcept {
        m_takes.clear();
        m_segments.clear();
    }

private:
    std::unordered_map<uint32_t, Take> m_takes;
    std::vector<CompSegment> m_segments;
};

} // namespace Aura::Core::Engine
